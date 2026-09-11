// The canonical writer.
//
// corpus/README.md's five rules, applied here and nowhere else in this edition. Two of them are where Java's
// standard library looks right and is not.
//
// # Rule 3 is not `Double.toString`, and the counter-example is in the corpus's own domain
//
// `Double.toString` produces shortest-round-trip *digits* for almost everything, which is what makes this
// worth writing down: an edition built on it would pass thousands of cases and fail on the ones below.
//
// * **It is not always shortest.** `Double.toString(Double.MIN_VALUE)` is `4.9E-324`. `5E-324` parses back to
//   the same double, so the shortest decimal that round-trips has **one** digit and Java prints two. Measured,
//   not assumed: `Double.parseDouble("5E-324") == Double.MIN_VALUE` is true.
// * **It is exponential wherever it feels like it** — `1.0E-7`, `1.0E21`, `9.007199254740992E15` — and rule 3
//   is positional always.
// * **It writes a `.0` on every integral value**, so `1` comes out `1.0`.
// * **It writes `-0.0`**, and rule 3 writes `0`.
//
// So the search the reference runs is run here too: the shortest precision whose rendering reads back as the
// same double, then converted to a positional form. `String.format("%.*e")` and `Double.parseDouble` are the
// two halves, and both are correctly rounded.
//
// # Rule 2 is not `String.compareTo`
//
// Keys sort by their UTF-8 **bytes**; `compareTo` compares UTF-16 code units. The two agree for every key in
// the basic multilingual plane and disagree above it, because a surrogate pair sorts below U+E000…U+FFFF as
// code units and above it as UTF-8. Comparing the encoded bytes is the rule as written.

package skynet.os;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

public final class Canonical {
    private Canonical() {}

    /** Thrown for a non-finite number, which is the only way writing a value can fail. */
    public static final class NonFinite extends RuntimeException {
        private static final long serialVersionUID = 1L;

        NonFinite(double value) {
            super("cannot serialise a non-finite number: " + value);
        }
    }

    /** 2^53, the point above which an integral double is no longer written as an integer. */
    private static final double INT_SAFE = 9007199254740992.0;

    /**
     * Turn {@code -4.4e-01} into {@code -0.44}.
     *
     * <p>The reference's {@code exponentialToPositional}, over the one shape {@code %e} produces. The exponent
     * is read as a number, so Java's two-digit minimum and JavaScript's one-digit rendering are the same input
     * to this.
     */
    private static String exponentialToPositional(String rendered) {
        int marker = rendered.indexOf('e');
        if (marker < 0) return rendered;

        String mantissa = rendered.substring(0, marker);
        int exponent = Integer.parseInt(rendered.substring(marker + 1));
        boolean negative = mantissa.startsWith("-");
        if (negative) mantissa = mantissa.substring(1);
        String digits = mantissa.replace(".", "");

        StringBuilder out = new StringBuilder();
        if (negative) out.append('-');
        // The first digit carries weight 10^exponent, so the point falls after `exponent + 1` digits.
        int point = exponent + 1;
        if (point <= 0) {
            out.append("0.").append("0".repeat(-point)).append(digits);
        } else if (point >= digits.length()) {
            out.append(digits).append("0".repeat(point - digits.length()));
        } else {
            out.append(digits, 0, point).append('.').append(digits.substring(point));
        }
        return out.toString();
    }

    /** Format a number in canonical form. Throws on a non-finite value rather than emitting {@code null}. */
    public static String formatNumber(double value) {
        if (Double.isNaN(value) || Double.isInfinite(value)) throw new NonFinite(value);
        // Covers -0.0, which rule 3 writes as `0`. Comparing against 0.0 rather than testing the sign bit is
        // deliberate: -0.0 == 0.0 is true, and that is the whole rule.
        if (value == 0.0) return "0";
        if (value == Math.floor(value) && Math.abs(value) < INT_SAFE) {
            // An integral double below 2^53 is exactly representable, and `%.0f` renders it without an
            // exponent — which is what rule 3 asks for and what `Double.toString` does not do.
            return String.format(Locale.ROOT, "%.0f", value);
        }
        for (int precision = 1; precision <= 17; precision++) {
            String rendered = String.format(Locale.ROOT, "%." + (precision - 1) + "e", value);
            if (Double.parseDouble(rendered) == value) return exponentialToPositional(rendered);
        }
        return exponentialToPositional(String.format(Locale.ROOT, "%.16e", value));
    }

    /**
     * Rule 4: escape the quote, the backslash and the C0 controls, and nothing else.
     *
     * <p>Everything above U+001F is written as itself, and the output is encoded UTF-8 at the end. An edition
     * that escaped non-ASCII would produce a document that differs from the corpus on every case carrying a
     * degree sign.
     *
     * <p>Iterated by {@code char} rather than by code point on purpose: a surrogate pair is two chars and
     * neither is a control, so each is appended unchanged and the pair survives — which is both simpler and
     * what a lone surrogate needs, since a JavaScript string can hold one and the reference can therefore
     * produce one.
     */
    private static void writeString(String text, StringBuilder out) {
        out.append('"');
        for (int at = 0; at < text.length(); at++) {
            char ch = text.charAt(at);
            switch (ch) {
                case '"' -> out.append("\\\"");
                case '\\' -> out.append("\\\\");
                case '\b' -> out.append("\\b");
                case '\f' -> out.append("\\f");
                case '\n' -> out.append("\\n");
                case '\r' -> out.append("\\r");
                case '\t' -> out.append("\\t");
                default -> {
                    if (ch < 0x20) {
                        out.append(String.format(Locale.ROOT, "\\u%04x", (int) ch));
                    } else {
                        out.append(ch);
                    }
                }
            }
        }
        out.append('"');
    }

    /**
     * Rule 2: keys ascending by their UTF-8 byte sequence.
     *
     * <p>Compared as encoded bytes rather than with {@code String.compareTo}, which compares UTF-16 code units
     * and disagrees for any key above the basic multilingual plane.
     */
    private static int compareUtf8(String left, String right) {
        byte[] a = left.getBytes(StandardCharsets.UTF_8);
        byte[] b = right.getBytes(StandardCharsets.UTF_8);
        int shared = Math.min(a.length, b.length);
        for (int at = 0; at < shared; at++) {
            // Unsigned, because a byte above 0x7F is negative in Java and would sort below an ASCII one.
            int difference = (a[at] & 0xFF) - (b[at] & 0xFF);
            if (difference != 0) return difference;
        }
        return a.length - b.length;
    }

    private static void writeValue(Value value, StringBuilder out) {
        if (value == null || value.kind() == Value.Kind.NULL) {
            out.append("null");
            return;
        }
        switch (value.kind()) {
            case BOOL -> out.append(value == Value.TRUE ? "true" : "false");
            case NUMBER -> out.append(formatNumber(value.number()));
            case STRING -> writeString(value.text(), out);
            case ARRAY -> {
                out.append('[');
                List<Value> items = value.items();
                for (int at = 0; at < items.size(); at++) {
                    if (at > 0) out.append(',');
                    writeValue(items.get(at), out);
                }
                out.append(']');
            }
            case OBJECT -> {
                List<Value.Member> sorted = new ArrayList<>(value.memberList());
                sorted.sort((left, right) -> compareUtf8(left.key(), right.key()));
                out.append('{');
                for (int at = 0; at < sorted.size(); at++) {
                    if (at > 0) out.append(',');
                    writeString(sorted.get(at).key(), out);
                    out.append(':');
                    writeValue(sorted.get(at).value(), out);
                }
                out.append('}');
            }
            default -> out.append("null");
        }
    }

    /** Serialise a value in canonical form. */
    public static String encode(Value value) {
        StringBuilder out = new StringBuilder();
        writeValue(value, out);
        return out.toString();
    }
}
