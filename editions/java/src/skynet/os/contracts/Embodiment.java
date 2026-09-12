// The embodiment descriptor and the role vocabulary — `reference/typescript/embodiment.ts`.
//
// The role vocabulary is the part of this module the whole system leans on: a role is how two bodies from two
// vendors are recognised as having the same shoulder, and §6.3's transport plan is built out of the match. The
// rule that keeps it honest is that an unrecognised role never *silently* maps to something plausible —
// `parseRole` reports that it could not read one, and the caller treats that as "this joint can only be matched
// by exact name". A body the vocabulary does not describe is still usable; it just cannot take part in
// cross-body remapping, which is the truthful outcome, because nobody has established what its joints mean.

package skynet.os.contracts;

import skynet.os.Js;
import skynet.os.Value;

public final class Embodiment {
    private Embodiment() {}

    /**
     * Twelve chains, and {@code aux} and {@code tail} are in the list so that a body nobody anticipated — a
     * snake robot's segments, a second mast, a tail-mounted sensor boom — can still declare a role that parses,
     * rather than being pushed into a wrong-but-close chain because the vocabulary had no honest slot for it.
     */
    static final String[] CANONICAL_CHAINS = {
        "left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand",
        "torso", "neck", "head", "base", "tail", "aux",
    };

    /**
     * Axis of motion, when a link has more than one degree of freedom. A three-DoF shoulder is three joints in
     * every real driver, and the axis suffix is what keeps {@code left_arm.shoulder.pitch} from colliding with
     * {@code left_arm.shoulder.roll}.
     */
    static final String[] CANONICAL_AXES = {
        "pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip",
    };

    /**
     * A parsed role.
     *
     * <p>{@code axis} is null when the role named no axis, which two segments always mean: {@code torso.yaw} is
     * a link called {@code yaw}, not an axis. {@code index} is a boxed {@code Double} so that absent and zero
     * are different — {@code base.wheel#0} is a valid role whose index is zero, which is the corpus case
     * {@code ordinal-zero} exists for, and a sentinel would read it as absent.
     */
    public record Role(String chain, String link, String axis, Double index) {}

    private static boolean inList(String[] list, String text) {
        for (String held : list) {
            if (held.equals(text)) return true;
        }
        return false;
    }

    private static boolean lowerOrUnderscore(char ch) {
        return (ch >= 'a' && ch <= 'z') || ch == '_';
    }

    private static boolean linkByte(char ch) {
        return lowerOrUnderscore(ch) || (ch >= '0' && ch <= '9');
    }

    private static boolean lowerByte(char ch) {
        return ch >= 'a' && ch <= 'z';
    }

    private static boolean digitByte(char ch) {
        return ch >= '0' && ch <= '9';
    }

    private interface Member {
        boolean test(char ch);
    }

    private static int runOf(String text, int at, Member member) {
        int end = at;
        while (end < text.length() && member.test(text.charAt(end))) end += 1;
        return end - at;
    }

    public static Role parseRoleText(String role) {
        // `role.trim().toLowerCase()`, in that order and with the engine's full mapping: the corpus holds
        // `necK.link.pitch`, which is a chain only after lower-casing, and a role wrapped in a byte-order mark,
        // which is whitespace only to JavaScript.
        String text = Js.lower(Js.trim(role));

        // The reference's role pattern, anchored at both ends. Every character class in it is disjoint from the
        // separator that follows it, so a greedy run is the only run that can match and there is nothing to
        // backtrack: a chain of lower-case letters and underscores, a point, a link that may also hold digits,
        // an optional point and axis of lower-case letters, and an optional hash and decimal ordinal.
        int at = 0;
        int chainLen = runOf(text, at, Embodiment::lowerOrUnderscore);
        if (chainLen == 0) return null;
        String chain = text.substring(at, at + chainLen);
        at += chainLen;
        if (at >= text.length() || text.charAt(at) != '.') return null;
        at += 1;
        int linkLen = runOf(text, at, Embodiment::linkByte);
        if (linkLen == 0) return null;
        String link = text.substring(at, at + linkLen);
        at += linkLen;

        String axis = null;
        if (at < text.length() && text.charAt(at) == '.') {
            int after = at + 1;
            int axisLen = runOf(text, after, Embodiment::lowerByte);
            // The axis group is optional, but a point with nothing readable after it cannot be consumed by
            // anything else either, so the whole role fails — `a.b.3` and `a.b.` both.
            if (axisLen == 0) return null;
            axis = text.substring(after, after + axisLen);
            at = after + axisLen;
        }

        Double index = null;
        if (at < text.length() && text.charAt(at) == '#') {
            int after = at + 1;
            int indexLen = runOf(text, after, Embodiment::digitByte);
            if (indexLen == 0) return null;
            index = Double.parseDouble(text.substring(after, after + indexLen));
            at = after + indexLen;
        }
        if (at != text.length()) return null;

        if (!inList(CANONICAL_CHAINS, chain)) return null;
        if (axis != null && !inList(CANONICAL_AXES, axis)) return null;
        return new Role(chain, link, axis, index);
    }

    /**
     * {@code if (!role) return null} — a null, an absent field and an empty string are all falsy, and all three
     * mean the same thing here: no role was declared.
     */
    public static Role parseRole(Value role) {
        if (role == null || role.kind() != Value.Kind.STRING) return null;
        String text = Value.asString(role, "");
        if (text.isEmpty()) return null;
        return parseRoleText(text);
    }

    /** The canonical string form, so two parsed roles can be compared as keys. */
    public static String roleKey(Role role) {
        StringBuilder out = new StringBuilder();
        out.append(role.chain()).append('.').append(role.link());
        if (role.axis() != null) out.append('.').append(role.axis());
        if (role.index() != null) out.append('#').append(Js.numberToString(role.index()));
        return out.toString();
    }

    public static Value rolesVocabularyCase(Value input) {
        Role parsed = parseRole(Value.get(input, "role"));
        if (parsed == null) return Value.NULL;
        return Value.object()
                .set("chain", Value.of(parsed.chain()))
                .set("link", Value.of(parsed.link()))
                .set("axis", parsed.axis() == null ? Value.NULL : Value.of(parsed.axis()))
                .set("index", parsed.index() == null ? Value.NULL : Value.of(parsed.index()))
                .set("key", Value.of(roleKey(parsed)));
    }
}
