// The canonical JSON value model — this edition's half of `reference/typescript/canonical.ts`.
//
// §31.4 requires byte-identical output, and corpus/README.md states the form: no insignificant whitespace,
// object keys sorted by their UTF-8 bytes, numbers finite and positional, only the quote, the backslash and
// the C0 controls escaped, absent fields omitted and `null` written.
//
// # Java is the closest edition to the reference, and that is a trap
//
// A `String` here is UTF-16, exactly as a JavaScript string is. Every other edition has had to decide how to
// represent the reference's strings; this one does not, and the temptation is to assume the rest of the
// language agrees too. It does not, in four measured places (see `Js`) — and rule 2 is one of them: keys sort
// by their **UTF-8 bytes**, and `String.compareTo` compares UTF-16 code units. The two orders agree for every
// key in the basic multilingual plane and disagree above it, because a surrogate pair (0xD800…) sorts below
// U+E000…U+FFFF as code units and above it as UTF-8.
//
// # Absent and null are different facts
//
// A field never set is the reference's `undefined`, which rule 5 omits; a field set to `Value.NULL` is JSON
// null, which rule 5 writes. `set` refuses a Java `null` outright so the third state cannot exist — the
// whole system distinguishes "nobody declared a safety envelope" from "the envelope is null".

package skynet.os;

import java.util.ArrayList;
import java.util.List;

public final class Value {
    public enum Kind { NULL, BOOL, NUMBER, STRING, ARRAY, OBJECT }

    /** One member of an object, in the order it was inserted. */
    public record Member(String key, Value value) {}

    public static final Value NULL = new Value(Kind.NULL);
    public static final Value TRUE = new Value(Kind.BOOL);
    public static final Value FALSE = new Value(Kind.BOOL);

    private final Kind kind;
    private double number;
    private String text;
    private List<Value> items;
    private List<Member> members;

    private Value(Kind kind) {
        this.kind = kind;
    }

    public static Value of(boolean truth) {
        return truth ? TRUE : FALSE;
    }

    public static Value of(double number) {
        Value out = new Value(Kind.NUMBER);
        out.number = number;
        return out;
    }

    public static Value of(String text) {
        Value out = new Value(Kind.STRING);
        out.text = text;
        return out;
    }

    public static Value array() {
        Value out = new Value(Kind.ARRAY);
        out.items = new ArrayList<>();
        return out;
    }

    public static Value object() {
        Value out = new Value(Kind.OBJECT);
        out.members = new ArrayList<>();
        return out;
    }

    public Kind kind() {
        return kind;
    }

    public Value push(Value item) {
        if (item == null) {
            throw new IllegalArgumentException("push of a Java null; Value.NULL is how JSON null is spelled");
        }
        items.add(item);
        return this;
    }

    /**
     * Append {@code key}, replacing a key already present <b>in place</b> so it keeps its original position —
     * which is what assigning to a field twice does in the reference, and matters because the order a
     * contract inserted in is what a failure prints.
     */
    public Value set(String key, Value value) {
        if (value == null) {
            throw new IllegalArgumentException(
                    "set of a Java null for `" + key + "`; Value.NULL is JSON null and omitting the call is"
                            + " the reference's undefined. A third state would make rule 5 undecidable.");
        }
        for (int at = 0; at < members.size(); at++) {
            if (members.get(at).key().equals(key)) {
                members.set(at, new Member(key, value));
                return this;
            }
        }
        members.add(new Member(key, value));
        return this;
    }

    /**
     * {@code set} unless {@code value} is null — the reference's {@code undefined}, which rule 5 omits.
     * Spelled differently from {@code set} so that omitting a field is a decision at the call site rather
     * than a value that happened to be absent.
     */
    public Value setOptional(String key, Value value) {
        return value == null ? this : set(key, value);
    }

    // ── Reading ──────────────────────────────────────────────────────────────
    //
    // Every accessor is total: a kind mismatch answers the fallback rather than throwing, because a contract
    // reading a document reads whatever the document holds — exactly as the reference does when a field's
    // declared type and its actual value disagree.

    /**
     * True when the field is absent (a Java null) or present and JSON null — the reference's {@code x == null},
     * which is the loose comparison it uses throughout and which is true of both.
     */
    public static boolean nullish(Value value) {
        return value == null || value.kind == Kind.NULL;
    }

    public static boolean asBool(Value value, boolean fallback) {
        if (value == null || value.kind != Kind.BOOL) return fallback;
        return value == TRUE;
    }

    public static double asNumber(Value value, double fallback) {
        if (value == null || value.kind != Kind.NUMBER) return fallback;
        return value.number;
    }

    public static String asString(Value value, String fallback) {
        if (value == null || value.kind != Kind.STRING) return fallback;
        return value.text;
    }

    public static boolean stringIs(Value value, String literal) {
        return value != null && value.kind == Kind.STRING && value.text.equals(literal);
    }

    public static int length(Value value) {
        if (value == null) return 0;
        if (value.kind == Kind.ARRAY) return value.items.size();
        if (value.kind == Kind.OBJECT) return value.members.size();
        return 0;
    }

    /** Element {@code index}, or null past the end — which {@code nullish} reads as the reference's undefined. */
    public static Value at(Value value, int index) {
        if (value == null || value.kind != Kind.ARRAY || index < 0 || index >= value.items.size()) return null;
        return value.items.get(index);
    }

    /** Member {@code key}, or null when absent. */
    public static Value get(Value value, String key) {
        if (value == null || value.kind != Kind.OBJECT) return null;
        for (Member member : value.members) {
            if (member.key().equals(key)) return member.value();
        }
        return null;
    }

    /** The members in insertion order, for the contracts that iterate one. Empty for anything else. */
    public static List<Member> members(Value value) {
        if (value == null || value.kind != Kind.OBJECT) return List.of();
        return value.members;
    }

    /**
     * True when {@code value} is an object with a member {@code key}, present even if null — the reference's
     * {@code key in object}, which is a different question from {@code object[key] != null}.
     */
    public static boolean has(Value value, String key) {
        if (value == null || value.kind != Kind.OBJECT) return false;
        for (Member member : value.members) {
            if (member.key().equals(key)) return true;
        }
        return false;
    }

    double number() {
        return number;
    }

    String text() {
        return text;
    }

    List<Value> items() {
        return items;
    }

    List<Member> memberList() {
        return members;
    }
}
