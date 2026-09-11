// The JavaScript semantics the reference relies on, spelled out.
//
// Java is the closest edition to the reference in the one place that usually costs the most — a `String` here
// is UTF-16, exactly as a JavaScript string is, so there is no decision to make about how to represent the
// reference's strings. That makes the four places below more dangerous rather than less, because they are the
// ones where a reader stops expecting a difference:
//
// | The reference calls | Java's near-equivalent | Why it is not the same |
// |---|---|---|
// | `trim()` | `String.trim()` / `String.strip()` | `trim()` removes anything at or below U+0020 and nothing above it, so U+00A0 and U+FEFF survive. `strip()` uses `Character.isWhitespace`, which excludes U+00A0 and U+FEFF *and* includes U+001C-001F. ECMAScript's set is neither |
// | `toUpperCase()` | `String.toUpperCase()` | The mappings and the sigma rule are right, and the *Unicode version* is the JDK's. An edition using it diverges on the characters added between that version and the engine's, in a build nobody changed |
// | `${value}` | `Double.toString` | It writes `1.0E-7`, a `.0` on every integral value, `-0.0`, and `4.9E-324` where the shortest round-trip is `5e-324` |
// | `Number(text)` | `Double.parseDouble` | `Number("0x1f")` is 31 and carries no sign; `Number("Infinity")` needs that capital; `Number("")` is zero; and `parseDouble` accepts `1d`, `0x1p3` and a trailing `f` |

package skynet.os;

import java.util.Locale;

public final class Js {
    private Js() {}

    public static final String UNICODE_VERSION = UnicodeUpper.UNICODE_VERSION;

    // ── Whitespace ───────────────────────────────────────────────────────────

    /**
     * ECMAScript's WhiteSpace and LineTerminator, which is what {@code trim()} removes.
     *
     * <p>Written out as the <i>language's</i> set rather than derived from a Unicode property, because that is
     * what it is: U+FEFF is whitespace to JavaScript and is not {@code White_Space} in Unicode, and U+0085 is
     * the other way round.
     */
    public static boolean isSpace(int code) {
        return switch (code) {
            case 0x0009, 0x000A, 0x000B, 0x000C, 0x000D, 0x0020, 0x00A0, 0x1680,
                    0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF -> true;
            // U+2000 to U+200A, the en quad through the hair space.
            default -> code >= 0x2000 && code <= 0x200A;
        };
    }

    public static String trim(String text) {
        int start = 0;
        while (start < text.length()) {
            int code = text.codePointAt(start);
            if (!isSpace(code)) break;
            start += Character.charCount(code);
        }
        int end = text.length();
        while (end > start) {
            int code = text.codePointBefore(end);
            if (!isSpace(code)) break;
            end -= Character.charCount(code);
        }
        return text.substring(start, end);
    }

    // ── Case ─────────────────────────────────────────────────────────────────

    private static boolean inRanges(int[][] ranges, int code) {
        int low = 0;
        int high = ranges.length;
        while (low < high) {
            int middle = low + (high - low) / 2;
            if (code < ranges[middle][0]) high = middle;
            else if (code > ranges[middle][1]) low = middle + 1;
            else return true;
        }
        return false;
    }

    private static boolean isCased(int code) {
        return inRanges(UnicodeRanges.casedRanges, code);
    }

    private static boolean isCaseIgnorable(int code) {
        return inRanges(UnicodeRanges.caseIgnorableRanges, code);
    }

    /**
     * The mapping for {@code code}, or null when it maps to itself.
     *
     * <p>{@code from} is ascending, so this is a binary search; a table that lost its order would still answer
     * most lookups, which is why the self-check asserts the order rather than trusting it.
     */
    private static int[] lookup(int[] from, int[][] to, int code) {
        int low = 0;
        int high = from.length;
        while (low < high) {
            int middle = low + (high - low) / 2;
            if (code < from[middle]) high = middle;
            else if (code > from[middle]) low = middle + 1;
            else return to[middle];
        }
        return null;
    }

    /** True when both mapping tables are ascending and the ranges do not overlap. */
    public static boolean tablesSorted() {
        for (int at = 1; at < UnicodeUpper.upperFrom.length; at++) {
            if (UnicodeUpper.upperFrom[at - 1] >= UnicodeUpper.upperFrom[at]) return false;
        }
        for (int at = 1; at < UnicodeLower.lowerFrom.length; at++) {
            if (UnicodeLower.lowerFrom[at - 1] >= UnicodeLower.lowerFrom[at]) return false;
        }
        for (int at = 1; at < UnicodeRanges.casedRanges.length; at++) {
            if (UnicodeRanges.casedRanges[at - 1][1] >= UnicodeRanges.casedRanges[at][0]) return false;
        }
        for (int at = 1; at < UnicodeRanges.caseIgnorableRanges.length; at++) {
            if (UnicodeRanges.caseIgnorableRanges[at - 1][1] >= UnicodeRanges.caseIgnorableRanges[at][0]) {
                return false;
            }
        }
        return true;
    }

    public static String upper(String text) {
        StringBuilder out = new StringBuilder();
        int at = 0;
        while (at < text.length()) {
            int code = text.codePointAt(at);
            at += Character.charCount(code);
            int[] mapping = lookup(UnicodeUpper.upperFrom, UnicodeUpper.upperTo, code);
            if (mapping == null) out.appendCodePoint(code);
            else for (int point : mapping) out.appendCodePoint(point);
        }
        return out.toString();
    }

    private static final int CAPITAL_SIGMA = 0x03A3;
    private static final int SMALL_SIGMA = 0x03C3;
    private static final int FINAL_SIGMA = 0x03C2;

    /**
     * Unicode's Final_Sigma condition, which is the one context rule {@code toLowerCase} applies outside the
     * Turkish and Lithuanian tailorings.
     *
     * <p>Before: a cased character, skipping case-ignorable ones. After: <i>not</i> a cased character, skipping
     * case-ignorable ones. A character that is both cased and case-ignorable is <b>skipped</b>, because the
     * condition skips Case_Ignorable first — and that is the class {@code tools/unicode-case.ts} picks
     * representatives for, so the vectors are what say whether this reading is the engine's.
     */
    private static boolean scanCased(String text, int from, boolean forward) {
        if (forward) {
            int at = from;
            while (at < text.length()) {
                int code = text.codePointAt(at);
                at += Character.charCount(code);
                if (isCaseIgnorable(code)) continue;
                return isCased(code);
            }
            return false;
        }
        int at = from;
        while (at > 0) {
            int code = text.codePointBefore(at);
            at -= Character.charCount(code);
            if (isCaseIgnorable(code)) continue;
            return isCased(code);
        }
        return false;
    }

    public static String lower(String text) {
        StringBuilder out = new StringBuilder();
        int at = 0;
        while (at < text.length()) {
            int start = at;
            int code = text.codePointAt(at);
            at += Character.charCount(code);
            if (code == CAPITAL_SIGMA) {
                boolean before = scanCased(text, start, false);
                boolean after = scanCased(text, at, true);
                out.appendCodePoint(before && !after ? FINAL_SIGMA : SMALL_SIGMA);
                continue;
            }
            int[] mapping = lookup(UnicodeLower.lowerFrom, UnicodeLower.lowerTo, code);
            if (mapping == null) out.appendCodePoint(code);
            else for (int point : mapping) out.appendCodePoint(point);
        }
        return out.toString();
    }

    // ── Numbers and ToString ─────────────────────────────────────────────────

    /**
     * ECMAScript's Number::toString at radix 10 — {@code String(value)}, <b>not</b> the canonical writer and
     * <b>not</b> {@code Double.toString}.
     *
     * <p>Stated over three integers: {@code s} with {@code k} digits and no trailing zero, and {@code n} where
     * {@code s * 10^(n-k)} is the value. The shortest {@code %e} that reads back as this double gives all three
     * at once — its digits are {@code s}, their count is {@code k}, and its exponent is {@code n - 1}.
     */
    public static String numberToString(double number) {
        if (Double.isNaN(number)) return "NaN";
        if (number == 0.0) return "0";
        if (Double.isInfinite(number)) return number < 0 ? "-Infinity" : "Infinity";

        double magnitude = Math.abs(number);
        String rendered = String.format(Locale.ROOT, "%.16e", magnitude);
        for (int precision = 1; precision <= 17; precision++) {
            String candidate = String.format(Locale.ROOT, "%." + (precision - 1) + "e", magnitude);
            if (Double.parseDouble(candidate) == magnitude) {
                rendered = candidate;
                break;
            }
        }

        int marker = rendered.indexOf('e');
        String digits = rendered.substring(0, marker).replace(".", "");
        // ECMAScript's `s` carries no trailing zero, and the rules below count digits.
        int k = digits.length();
        while (k > 1 && digits.charAt(k - 1) == '0') k -= 1;
        digits = digits.substring(0, k);
        int n = Integer.parseInt(rendered.substring(marker + 1)) + 1;

        StringBuilder out = new StringBuilder();
        if (number < 0) out.append('-');
        if (k <= n && n <= 21) {
            out.append(digits).append("0".repeat(n - k));
        } else if (0 < n && n <= 21) {
            out.append(digits, 0, n).append('.').append(digits.substring(n));
        } else if (-6 < n && n <= 0) {
            out.append("0.").append("0".repeat(-n)).append(digits);
        } else {
            // Exponential — which is the whole reason the canonical writer does not use this function.
            out.append(digits.charAt(0));
            if (k > 1) out.append('.').append(digits.substring(1));
            out.append('e').append(n - 1 >= 0 ? "+" : "-").append(Math.abs(n - 1));
        }
        return out.toString();
    }

    /**
     * ECMAScript's ToString over a parsed JSON value — what a template literal does to whatever it is handed.
     *
     * <p>{@code present} is false for a field that was absent, which is JavaScript's {@code undefined} and
     * renders as the word — a different rendering from a field that was present and null, exactly as rule 5
     * says they are different facts.
     */
    public static String toStringOf(Value value, boolean present) {
        if (!present || value == null) return "undefined";
        return switch (value.kind()) {
            case NULL -> "null";
            case BOOL -> Value.asBool(value, false) ? "true" : "false";
            case NUMBER -> numberToString(Value.asNumber(value, 0));
            case STRING -> Value.asString(value, "");
            case ARRAY -> {
                // Array.prototype.join: a null or undefined element is the empty string rather than "null",
                // which is the one place ToString's answer for null is not the word.
                StringBuilder out = new StringBuilder();
                for (int at = 0; at < Value.length(value); at++) {
                    if (at > 0) out.append(',');
                    Value element = Value.at(value, at);
                    if (Value.nullish(element)) continue;
                    out.append(toStringOf(element, true));
                }
                yield out.toString();
            }
            case OBJECT -> "[object Object]";
        };
    }

    /**
     * ECMAScript's ToNumber over a string — {@code Number(text)}, which is not {@code parseDouble}.
     *
     * <p>Three differences decide corpus cases. {@code Number("0x1f")} is 31, because a string may hold a
     * non-decimal integer literal — and it carries <b>no sign</b> in the grammar, so {@code -0x10} is NaN where
     * {@code -16} is -16. {@code Number("Infinity")} is infinite, spelled with that capital and no other. And
     * {@code Number("")} is <i>zero</i>, which is why the reference's {@code toNumber} guards with its own blank
     * test before converting.
     */
    public static double stringToNumber(String text) {
        String trimmed = trim(text);
        if (trimmed.isEmpty()) return 0;

        if (trimmed.length() > 2 && trimmed.charAt(0) == '0') {
            int radix = switch (trimmed.charAt(1)) {
                case 'x', 'X' -> 16;
                case 'o', 'O' -> 8;
                case 'b', 'B' -> 2;
                default -> 0;
            };
            if (radix != 0) {
                double out = 0;
                for (int at = 2; at < trimmed.length(); at++) {
                    char ch = trimmed.charAt(at);
                    int digit;
                    if (ch >= '0' && ch <= '9') digit = ch - '0';
                    else if (ch >= 'a' && ch <= 'z') digit = ch - 'a' + 10;
                    else if (ch >= 'A' && ch <= 'Z') digit = ch - 'A' + 10;
                    else return Double.NaN;
                    if (digit >= radix) return Double.NaN;
                    out = out * radix + digit;
                }
                return out;
            }
        }

        int at = 0;
        double sign = 1;
        if (trimmed.charAt(at) == '+' || trimmed.charAt(at) == '-') {
            sign = trimmed.charAt(at) == '-' ? -1 : 1;
            at += 1;
        }
        if (trimmed.substring(at).equals("Infinity")) return sign * Double.POSITIVE_INFINITY;

        // StrUnsignedDecimalLiteral, validated here and converted by `parseDouble` — the validation is what
        // keeps `parseDouble`'s wider grammar out.
        int scan = at;
        int integerDigits = 0;
        while (scan < trimmed.length() && trimmed.charAt(scan) >= '0' && trimmed.charAt(scan) <= '9') {
            scan += 1;
            integerDigits += 1;
        }
        int fractionDigits = 0;
        if (scan < trimmed.length() && trimmed.charAt(scan) == '.') {
            scan += 1;
            while (scan < trimmed.length() && trimmed.charAt(scan) >= '0' && trimmed.charAt(scan) <= '9') {
                scan += 1;
                fractionDigits += 1;
            }
        }
        if (integerDigits == 0 && fractionDigits == 0) return Double.NaN;
        if (scan < trimmed.length() && (trimmed.charAt(scan) == 'e' || trimmed.charAt(scan) == 'E')) {
            scan += 1;
            if (scan < trimmed.length() && (trimmed.charAt(scan) == '+' || trimmed.charAt(scan) == '-')) scan += 1;
            int exponentDigits = 0;
            while (scan < trimmed.length() && trimmed.charAt(scan) >= '0' && trimmed.charAt(scan) <= '9') {
                scan += 1;
                exponentDigits += 1;
            }
            if (exponentDigits == 0) return Double.NaN;
        }
        if (scan != trimmed.length()) return Double.NaN;
        return Double.parseDouble(trimmed);
    }
}
