// A JSON parser, for the two things this edition reads: the corpus files, and the `expected` strings inside
// them that the round trip re-serialises.
//
// Deliberately a parser of JSON rather than of canonical JSON. The corpus files are written by
// `tools/corpus-gen/generate.ts` through `JSON.stringify` with indentation, so they carry whitespace,
// unsorted keys and `\\uXXXX` escapes for anything this writer would emit as UTF-8 — and a parser that only
// accepted the canonical form could not read the corpus it exists to read.
//
// What it refuses is everything outside JSON proper: a trailing comma, a comment, `NaN`, `Infinity`, a bare
// word, a leading `+`, a leading zero, a control character inside a string. Each of those is a file this
// edition cannot have produced. §3.9's rule applies rather than being softened because the input is a
// checked-in file: all input from outside the process is untrusted, and a corpus path is an argument.
//
// No `java.util.Scanner`, no regex, and above all no third-party JSON library. §31.4 requires
// byte-identical output, so the number conversion and the duplicate-key rule have to be ours rather than
// whatever a dependency does this year — which is the same reason `Cargo.toml` has no dependencies.

package skynet.os;

public final class Parse {
    /** Where a parse stopped and why. */
    public static final class Invalid extends RuntimeException {
        private static final long serialVersionUID = 1L;

        public Invalid(String message) {
            super(message);
        }
    }

    /**
     * Bounds the nesting a document may reach. §7.5: bound recursion on untrusted input. Deeper than any
     * corpus document and shallow enough that the recursion cannot reach the stack this edition runs on.
     */
    private static final int MAX_DEPTH = 512;

    private final String bytes;
    private int at;
    private int depth;

    private Parse(String text) {
        this.bytes = text;
    }

    /**
     * Parse {@code text}. Trailing whitespace is allowed; trailing anything else is not, so a file holding
     * two documents fails rather than silently reporting the first.
     */
    public static Value parse(String text) {
        String held = text;
        // A UTF-8 byte-order mark, which an editor on Windows will add to a file it saves. Skipped rather
        // than refused, and only at the start.
        if (held.startsWith("﻿")) held = held.substring(1);
        Parse parser = new Parse(held);
        Value out = parser.value();
        parser.skipSpace();
        if (!parser.atEnd()) throw parser.fail("trailing bytes after the document");
        return out;
    }

    private Invalid fail(String message) {
        int line = 1;
        int column = 1;
        for (int scan = 0; scan < Math.min(at, bytes.length()); scan++) {
            if (bytes.charAt(scan) == '\n') {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        return new Invalid(line + ":" + column + ": " + message);
    }

    private boolean atEnd() {
        return at >= bytes.length();
    }

    private char peek() {
        return atEnd() ? '\0' : bytes.charAt(at);
    }

    private void skipSpace() {
        while (!atEnd()) {
            char ch = bytes.charAt(at);
            if (ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r') at += 1;
            else break;
        }
    }

    private void literal(String word) {
        if (!bytes.startsWith(word, at)) throw fail("not a JSON value");
        at += word.length();
    }

    private int hex4() {
        if (bytes.length() - at < 4) throw fail("a truncated \\u escape");
        int value = 0;
        for (int digit = 0; digit < 4; digit++) {
            char ch = bytes.charAt(at + digit);
            int held = Character.digit(ch, 16);
            // `Character.digit` accepts a fullwidth digit and the Arabic-Indic ones, which are not hex digits
            // in JSON — so the range is checked rather than trusted to the helper.
            if (held < 0 || !((ch >= '0' && ch <= '9') || (ch >= 'a' && ch <= 'f') || (ch >= 'A' && ch <= 'F'))) {
                throw fail("a \\u escape with a non-hex digit");
            }
            value = value * 16 + held;
        }
        at += 4;
        return value;
    }

    private String stringBytes() {
        if (peek() != '"') throw fail("expected a string");
        at += 1;
        StringBuilder out = new StringBuilder();
        while (true) {
            if (atEnd()) throw fail("an unterminated string");
            char ch = bytes.charAt(at);
            if (ch == '"') {
                at += 1;
                break;
            }
            if (ch < 0x20) throw fail("an unescaped control character in a string");
            if (ch != '\\') {
                out.append(ch);
                at += 1;
                continue;
            }
            at += 1;
            if (atEnd()) throw fail("a trailing backslash in a string");
            char escape = bytes.charAt(at);
            at += 1;
            switch (escape) {
                case '"' -> out.append('"');
                case '\\' -> out.append('\\');
                case '/' -> out.append('/');
                case 'b' -> out.append('\b');
                case 'f' -> out.append('\f');
                case 'n' -> out.append('\n');
                case 'r' -> out.append('\r');
                case 't' -> out.append('\t');
                case 'u' ->
                    // One `char` per escape, and a surrogate pair therefore assembles itself: `JSON.stringify`
                    // writes two escapes for a character above the BMP, and appending both chars puts the pair
                    // in the `String` exactly as the reference holds it. A lone surrogate survives for the same
                    // reason, which it must, because a JavaScript string can hold one.
                    out.append((char) hex4());
                default -> throw fail("an unknown escape in a string");
            }
        }
        return out.toString();
    }

    private Value number() {
        int start = at;
        if (peek() == '-') at += 1;
        // JSON forbids a leading zero before another digit, and accepting one would be accepting a document
        // this edition's writer cannot produce.
        if (peek() == '0') {
            at += 1;
            if (peek() >= '0' && peek() <= '9') throw fail("a number with a leading zero");
        } else if (peek() >= '1' && peek() <= '9') {
            while (peek() >= '0' && peek() <= '9') at += 1;
        } else {
            throw fail("a number with no digits");
        }
        if (peek() == '.') {
            at += 1;
            if (!(peek() >= '0' && peek() <= '9')) throw fail("a number with no digits after the point");
            while (peek() >= '0' && peek() <= '9') at += 1;
        }
        if (peek() == 'e' || peek() == 'E') {
            at += 1;
            if (peek() == '+' || peek() == '-') at += 1;
            if (!(peek() >= '0' && peek() <= '9')) throw fail("a number with no digits in its exponent");
            while (peek() >= '0' && peek() <= '9') at += 1;
        }
        // The run has been validated as a JSON number above, so this converts rather than parses — and it is
        // the same correctly rounded conversion the reference's `Number(...)` performs. `parseDouble` would
        // also accept `0x1p3`, `1d`, `Infinity` and a leading space; none of those can reach it from here.
        return Value.of(Double.parseDouble(bytes.substring(start, at)));
    }

    private Value arrayValue() {
        at += 1;
        Value out = Value.array();
        skipSpace();
        if (peek() == ']') {
            at += 1;
            return out;
        }
        while (true) {
            out.push(value());
            skipSpace();
            if (peek() == ',') {
                at += 1;
                skipSpace();
                // A trailing comma is the one JSON5 habit a hand-edited corpus file would pick up, and it is
                // named separately so the message says what is wrong rather than "not a value".
                if (peek() == ']') throw fail("a trailing comma in an array");
                continue;
            }
            if (peek() == ']') {
                at += 1;
                return out;
            }
            throw fail("expected a comma or a closing bracket");
        }
    }

    private Value objectValue() {
        at += 1;
        Value out = Value.object();
        skipSpace();
        if (peek() == '}') {
            at += 1;
            return out;
        }
        while (true) {
            skipSpace();
            String key = stringBytes();
            skipSpace();
            if (peek() != ':') throw fail("expected a colon after a key");
            at += 1;
            // A duplicate key replaces the earlier one in place, which is what the reference's object literal
            // does.
            out.set(key, value());
            skipSpace();
            if (peek() == ',') {
                at += 1;
                skipSpace();
                if (peek() == '}') throw fail("a trailing comma in an object");
                continue;
            }
            if (peek() == '}') {
                at += 1;
                return out;
            }
            throw fail("expected a comma or a closing brace");
        }
    }

    private Value value() {
        if (depth >= MAX_DEPTH) throw fail("a document nested deeper than this parser will go");
        skipSpace();
        if (atEnd()) throw fail("the document ended where a value was expected");
        depth += 1;
        try {
            return switch (peek()) {
                case '{' -> objectValue();
                case '[' -> arrayValue();
                case '"' -> Value.of(stringBytes());
                case 't' -> {
                    literal("true");
                    yield Value.TRUE;
                }
                case 'f' -> {
                    literal("false");
                    yield Value.FALSE;
                }
                case 'n' -> {
                    literal("null");
                    yield Value.NULL;
                }
                default -> number();
            };
        } finally {
            depth -= 1;
        }
    }
}
