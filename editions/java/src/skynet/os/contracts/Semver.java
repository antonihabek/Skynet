// Minimal semver parsing, comparison and range satisfaction — `reference/typescript/semver.ts`.
//
// It is a contract rather than a utility because §54.2 made `semver_satisfies` an operator the robot evaluates
// before a skill may start: a robot deciding whether its runtime satisfies `^1.2.0` differently from the Hub
// that sold it the skill is the §31.2 failure, *a format with two implementations of its own definition is not
// a format*.
//
// Note what is deliberately **not** shared with it. `package.ts` compares OS versions with its own
// numeric-segment comparator, because `min_os_version` is matched against whatever an image reports, which is
// `2.0` as often as `2.0.0`. Two comparators with two jobs.

package skynet.os.contracts;

import java.util.ArrayList;
import java.util.List;
import skynet.os.Js;
import skynet.os.Value;

public final class Semver {
    private Semver() {}

    /**
     * A parsed version, or null for anything that is not {@code major.minor.patch[-prerelease]}.
     *
     * <p>The three components are {@code double} because the reference's are: {@code Number(match[1])} over a
     * digit run with no length limit, so a version whose major is twenty digits long parses to the double
     * nearest it and is written back out that way. An {@code int} here would answer a different question from
     * the oracle's on exactly that input.
     */
    public record Version(double major, double minor, double patch, String prerelease, String raw) {}

    private static boolean asciiDigit(char byte0) {
        return byte0 >= '0' && byte0 <= '9';
    }

    /** {@code [0-9A-Za-z.-]}, the prerelease character class. */
    private static boolean prereleaseByte(char ch) {
        return asciiDigit(ch) || (ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z') || ch == '.' || ch == '-';
    }

    /**
     * A maximal run of ASCII digits at {@code at}.
     *
     * <p>{@code \\d} in the reference's regex has no {@code u} flag and no Unicode property, so it is ASCII 0-9
     * and nothing else — which is what makes the corpus case written in Arabic-Indic digits unreadable rather
     * than a version. {@code Character.isDigit} would accept those, which is why it is not used.
     */
    private static int digitRun(String text, int at) {
        int end = at;
        while (end < text.length() && asciiDigit(text.charAt(end))) end += 1;
        return end - at;
    }

    /** {@code Number(...)} over a run of ASCII digits: the double nearest the value the digits spell. */
    private static double numberOf(String text) {
        return Double.parseDouble(text);
    }

    public static Version parse(String text) {
        String input = Js.trim(text);

        // `^(\\d+)\\.(\\d+)\\.(\\d+)(?:-([0-9A-Za-z.-]+))?$`, anchored at both ends. Digits and the point are
        // disjoint character classes, so a greedy run followed by a required point needs no backtracking: the
        // maximal run is the only run that can be followed by one.
        int at = 0;
        int majorLen = digitRun(input, at);
        if (majorLen == 0) return null;
        int majorAt = at;
        at += majorLen;
        if (at >= input.length() || input.charAt(at) != '.') return null;
        at += 1;
        int minorLen = digitRun(input, at);
        if (minorLen == 0) return null;
        int minorAt = at;
        at += minorLen;
        if (at >= input.length() || input.charAt(at) != '.') return null;
        at += 1;
        int patchLen = digitRun(input, at);
        if (patchLen == 0) return null;
        int patchAt = at;
        at += patchLen;

        String prerelease = null;
        if (at < input.length()) {
            if (input.charAt(at) != '-') return null;
            at += 1;
            int start = at;
            while (at < input.length() && prereleaseByte(input.charAt(at))) at += 1;
            if (at == start || at != input.length()) return null;
            prerelease = input.substring(start, at);
        }

        return new Version(
                numberOf(input.substring(majorAt, majorAt + majorLen)),
                numberOf(input.substring(minorAt, minorAt + minorLen)),
                numberOf(input.substring(patchAt, patchAt + patchLen)),
                prerelease,
                input);
    }

    public static boolean validVersion(String text) {
        return parse(text) != null;
    }

    /**
     * Is every byte of this identifier an ASCII digit? The reference's {@code /^\\d+$/} over a
     * {@code .}-separated part, which decides whether the part compares numerically or as a string.
     */
    private static boolean allDigits(String part) {
        if (part.isEmpty()) return false;
        for (int at = 0; at < part.length(); at++) {
            if (!asciiDigit(part.charAt(at))) return false;
        }
        return true;
    }

    /**
     * {@code text.split('.')[index]}, with null past the end — the reference's {@code undefined}, which the
     * comparison reads as "this side ran out of identifiers".
     */
    private static String dotPart(String text, int index) {
        int at = 0;
        int which = 0;
        while (true) {
            int start = at;
            while (at < text.length() && text.charAt(at) != '.') at += 1;
            if (which == index) return text.substring(start, at);
            if (at >= text.length()) return null;
            at += 1;
            which += 1;
        }
    }

    private static int dotCount(String text) {
        int parts = 1;
        for (int at = 0; at < text.length(); at++) {
            if (text.charAt(at) == '.') parts += 1;
        }
        return parts;
    }

    /**
     * Prerelease identifiers per semver §11: numeric below alphanumeric, and a longer list wins a tie.
     *
     * <p>The string comparison is JavaScript's {@code <}, which compares UTF-16 code units. Every byte a
     * prerelease may hold is ASCII, so {@code String.compareTo} is the same comparison here — and that is a
     * property of the grammar rather than a convenience, which is why it is written down.
     */
    private static int comparePrerelease(String left, String right) {
        if (left == null && right == null) return 0;
        // A version without a prerelease outranks one with it.
        if (left == null) return 1;
        if (right == null) return -1;
        if (left.equals(right)) return 0;

        int length = Math.max(dotCount(left), dotCount(right));
        for (int index = 0; index < length; index++) {
            String ai = dotPart(left, index);
            String bi = dotPart(right, index);
            if (ai == null) return -1;
            if (bi == null) return 1;
            boolean leftNumeric = allDigits(ai);
            boolean rightNumeric = allDigits(bi);
            if (leftNumeric && rightNumeric) {
                double difference = numberOf(ai) - numberOf(bi);
                if (difference != 0) return difference < 0 ? -1 : 1;
                continue;
            }
            if (leftNumeric) return -1;
            if (rightNumeric) return 1;
            int order = ai.compareTo(bi);
            if (order != 0) return order < 0 ? -1 : 1;
        }
        return 0;
    }

    /**
     * -1 if a &lt; b, 0 if equal, 1 if a &gt; b — and <b>0 for anything either side could not parse</b>, which
     * is the reference's answer and the reason {@code package.ts} keeps its own comparator: a strict parser in
     * that position would silently admit a package onto an older OS.
     */
    public static int compare(Version left, Version right) {
        if (left == null || right == null) return 0;
        if (left.major() != right.major()) return left.major() < right.major() ? -1 : 1;
        if (left.minor() != right.minor()) return left.minor() < right.minor() ? -1 : 1;
        if (left.patch() != right.patch()) return left.patch() < right.patch() ? -1 : 1;
        return comparePrerelease(left.prerelease(), right.prerelease());
    }

    // ── Ranges ───────────────────────────────────────────────────────────────

    private enum Op { EQ, GTE, GT, LTE, LT, CARET, TILDE }

    private record Operator(String spelling, Op op) {}

    /**
     * The alternation order is the reference's and it decides two corpus cases: {@code >=} is tried before
     * {@code >}, so {@code >=1.0.0} is one comparator rather than {@code >} applied to {@code =1.0.0}. A token
     * with no operator is an equality comparator.
     */
    private static final Operator[] OPERATORS = {
        new Operator(">=", Op.GTE), new Operator("<=", Op.LTE), new Operator(">", Op.GT),
        new Operator("<", Op.LT), new Operator("^", Op.CARET), new Operator("~", Op.TILDE),
        new Operator("=", Op.EQ),
    };

    private record Comparator(Op op, Version version) {}

    private static Operator operatorAt(String text, int at) {
        for (Operator candidate : OPERATORS) {
            if (text.startsWith(candidate.spelling(), at)) return candidate;
        }
        return null;
    }

    /**
     * {@code \\s*} — JavaScript's whitespace class, which is the same set {@code trim()} removes and includes
     * U+FEFF. A range written with one before its version is a corpus case.
     */
    private static int skipSpace(String text, int at) {
        int scan = at;
        while (scan < text.length()) {
            int code = text.codePointAt(scan);
            if (!Js.isSpace(code)) break;
            scan += Character.charCount(code);
        }
        return scan;
    }

    /**
     * One token of the reference's tokenizer:
     *
     * <pre>/(?:&gt;=|&lt;=|&gt;|&lt;|\\^|~|=)?\\s*\\d+\\.\\d+\\.\\d+(?:-[0-9A-Za-z.-]+)?/g</pre>
     *
     * <p>-1 when nothing matches at {@code at}, which is when the caller advances one character and tries
     * again — the way a global regex walks a string. Tokenizing on the operator rather than on whitespace is
     * what makes {@code >= 1.2.0} and {@code >=1.2.0} the same range, and {@code >=1.0.0<2.0.0} two comparators.
     */
    private static int tokenAt(String text, int at) {
        int scan = at;
        Operator found = operatorAt(text, scan);
        if (found != null) scan += found.spelling().length();
        scan = skipSpace(text, scan);
        int major = digitRun(text, scan);
        if (major == 0) return -1;
        scan += major;
        if (scan >= text.length() || text.charAt(scan) != '.') return -1;
        scan += 1;
        int minor = digitRun(text, scan);
        if (minor == 0) return -1;
        scan += minor;
        if (scan >= text.length() || text.charAt(scan) != '.') return -1;
        scan += 1;
        int patch = digitRun(text, scan);
        if (patch == 0) return -1;
        scan += patch;
        if (scan < text.length() && text.charAt(scan) == '-') {
            int start = scan + 1;
            int run = start;
            while (run < text.length() && prereleaseByte(text.charAt(run))) run += 1;
            // The prerelease group is optional, so a lone `-` with nothing readable after it simply is not part
            // of the token rather than failing it.
            if (run > start) scan = run;
        }
        return scan;
    }

    private static Comparator parseComparator(String token) {
        String trimmed = Js.trim(token);
        int at = 0;
        Op op = Op.EQ;
        Operator found = operatorAt(trimmed, at);
        if (found != null) {
            op = found.op();
            at += found.spelling().length();
        }
        at = skipSpace(trimmed, at);
        // `(.+)` requires at least one character, and `parse` trims again.
        if (at >= trimmed.length()) return null;
        Version version = parse(trimmed.substring(at));
        if (version == null) return null;
        return new Comparator(op, version);
    }

    private static boolean satisfiesComparator(Version target, Comparator against) {
        int order = compare(target, against.version());
        return switch (against.op()) {
            case GTE -> order >= 0;
            case GT -> order > 0;
            case LTE -> order <= 0;
            case LT -> order < 0;
            case EQ -> order == 0;
            case CARET -> {
                // Caret: compatible within the leftmost non-zero component.
                if (order < 0) yield false;
                Version base = against.version();
                if (base.major() > 0) yield target.major() == base.major();
                if (base.minor() > 0) yield target.major() == 0 && target.minor() == base.minor();
                yield target.major() == 0 && target.minor() == 0 && target.patch() == base.patch();
            }
            // Tilde: patch-level changes within the same minor.
            case TILDE -> order >= 0 && target.major() == against.version().major()
                    && target.minor() == against.version().minor();
        };
    }

    /**
     * A group of AND comparators. {@code readable} is false when a token the tokenizer produced was refused by
     * the comparator parser, which makes the whole group unreadable — exactly as
     * {@code comparators.some(c => c === null)} does.
     */
    private record Group(boolean readable, List<Comparator> comparators) {}

    private record ParsedRange(boolean wildcard, List<Group> groups) {}

    /**
     * A range, parsed into its {@code ||} groups.
     *
     * <p>A group that does not parse is marked unreadable rather than failing the whole range, which is what
     * keeps one unreadable alternative from invalidating the readable ones — and {@code validRange} is the
     * caller that wants the stricter answer, which it gets by asking about every group.
     */
    private static ParsedRange parseRange(String range) {
        String trimmed = Js.trim(range);
        if (trimmed.isEmpty() || trimmed.equals("*")) return new ParsedRange(true, List.of());
        if (Js.lower(trimmed).equals("any")) return new ParsedRange(true, List.of());

        List<Group> groups = new ArrayList<>();
        // `split('||')`, which yields one more piece than there are separators and can yield empty ones.
        // `String.split` with a limit of -1 keeps the trailing empties, which the default drops.
        for (String raw : trimmed.split("\\|\\|", -1)) {
            String piece = Js.trim(raw);
            List<Comparator> comparators = new ArrayList<>();
            boolean unreadable = false;
            // The global tokenizer: at each position, match or advance one character.
            int scan = 0;
            while (scan < piece.length()) {
                int end = tokenAt(piece, scan);
                if (end < 0) {
                    scan += 1;
                    continue;
                }
                Comparator held = parseComparator(piece.substring(scan, end));
                if (held == null) {
                    unreadable = true;
                    break;
                }
                comparators.add(held);
                scan = end;
            }
            groups.add(new Group(!unreadable && !comparators.isEmpty(), comparators));
        }
        return new ParsedRange(false, groups);
    }

    public static boolean satisfies(String version, String range) {
        Version target = parse(version);
        if (target == null) return false;
        ParsedRange groups = parseRange(range);
        if (groups.wildcard()) return true;
        for (Group group : groups.groups()) {
            if (!group.readable()) continue;
            boolean all = true;
            for (Comparator held : group.comparators()) {
                if (!satisfiesComparator(target, held)) {
                    all = false;
                    break;
                }
            }
            if (all) return true;
        }
        return false;
    }

    /**
     * Is {@code range} a range this edition can read at all?
     *
     * <p>{@code satisfies} answers false both for "this version is outside that window" and for "that is not a
     * window", and §54.2 needs those apart: a skill whose precondition names an unreadable range is a skill
     * nobody can ever run here, and its publisher is the one who has to hear about it.
     */
    public static boolean validRange(String range) {
        ParsedRange groups = parseRange(range);
        if (groups.wildcard()) return true;
        for (Group group : groups.groups()) {
            if (!group.readable()) return false;
        }
        return true;
    }

    // ── The shapes, from `reference/typescript/shape.ts` ──────────────────────

    private static Value shapeVersion(Version version) {
        if (version == null) return Value.NULL;
        return Value.object()
                .set("major", Value.of(version.major()))
                .set("minor", Value.of(version.minor()))
                .set("patch", Value.of(version.patch()))
                .set("prerelease", version.prerelease() == null ? Value.NULL : Value.of(version.prerelease()))
                .set("raw", Value.of(version.raw()));
    }

    public static Value satisfiesCase(Value input) {
        String version = Value.asString(Value.get(input, "version"), "");
        String range = Value.asString(Value.get(input, "range"), "");
        return Value.object()
                .set("version", Value.of(version))
                .set("range", Value.of(range))
                .set("satisfies", Value.of(satisfies(version, range)))
                // Validity beside satisfaction, because the two are different facts: pinning only the boolean
                // would let an edition agree about every satisfaction and disagree about which ranges it could
                // read at all — the difference between refusing a skill and telling its publisher why.
                .set("valid_version", Value.of(validVersion(version)))
                .set("valid_range", Value.of(validRange(range)));
    }

    public static Value compareCase(Value input) {
        Version left = parse(Value.asString(Value.get(input, "a"), ""));
        Version right = parse(Value.asString(Value.get(input, "b"), ""));
        return Value.object()
                .set("compare", Value.of((double) compare(left, right)))
                .set("a", shapeVersion(left))
                .set("b", shapeVersion(right));
    }
}
