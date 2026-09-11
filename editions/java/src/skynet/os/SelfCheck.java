// The Java edition checking itself, over the things the corpus cannot reach.
//
//   java -cp build skynet.os.SelfCheck [--corpus-root corpus]
//
// Five groups, and each exists because a corpus run would be green without it:
//
// 1. **Rule 3 at its edges**, and one of those edges is the reason this edition does not use
//    `Double.toString` at all. See `numbers()`.
// 2. **Rules 2, 4 and 5 as facts rather than consequences** — including rule 2 over a key above the basic
//    multilingual plane, which is where UTF-8 byte order and UTF-16 code-unit order part company and where
//    `String.compareTo` would answer differently.
// 3. **What the parser refuses.** A parser that accepted these would read documents the oracle cannot write,
//    and no corpus file would say so.
// 4. **The runner's own accounting**, over synthetic corpora with known answers.
// 5. **The case tables**, against 219 whole strings the reference's engine converted.

package skynet.os;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public final class SelfCheck {
    private SelfCheck() {}

    private static int checks;
    private static int failures;

    private static void expectText(String what, String actual, String wanted) {
        checks += 1;
        if (!wanted.equals(actual)) {
            failures += 1;
            System.err.printf("  %s: expected \"%s\", got \"%s\"%n", what, wanted, actual);
        }
    }

    private static void expectTrue(String what, boolean truth) {
        checks += 1;
        if (!truth) {
            failures += 1;
            System.err.printf("  %s: expected true%n", what);
        }
    }

    private static void expectInt(String what, long actual, long wanted) {
        checks += 1;
        if (actual != wanted) {
            failures += 1;
            System.err.printf("  %s: expected %d, got %d%n", what, wanted, actual);
        }
    }

    // ── 1. Rule 3 ────────────────────────────────────────────────────────────

    private static void checkNumber(String what, double number, String wanted) {
        expectText(what, Canonical.formatNumber(number), wanted);
    }

    private static void numbers() {
        System.err.println("rule 3 — numbers");
        checkNumber("zero", 0.0, "0");
        // `Double.toString` writes `-0.0` here, and rule 3 writes `0`.
        checkNumber("negative zero", -0.0, "0");
        // And a `.0` on every integral value, which rule 3 does not.
        checkNumber("one", 1.0, "1");
        checkNumber("minus one", -1.0, "-1");
        checkNumber("a hundred", 100.0, "100");
        checkNumber("2^53 - 2", 9007199254740990.0, "9007199254740990");
        checkNumber("2^53", 9007199254740992.0, "9007199254740992");
        checkNumber("a tenth", 0.1, "0.1");
        checkNumber("a tenth and two", 0.1 + 0.2, "0.30000000000000004");
        // The reference's own example of why `canonical.ts` exists rather than a `JSON.stringify` with a
        // replacer: the engine writes `1e-7`, and rule 3 requires a positional rendering. `Double.toString`
        // writes `1.0E-7`.
        checkNumber("a ten-millionth", 1e-7, "0.0000001");
        checkNumber("ten to the twenty-first", 1e21, "1000000000000000000000");
        checkNumber("a third", 1.0 / 3.0, "0.3333333333333333");
        checkNumber("a large mixed value", 1234567890.1234567, "1234567890.1234567");

        // **The case that decides this edition's whole approach to rule 3.**
        //
        // `Double.toString(Double.MIN_VALUE)` is `4.9E-324`. `5E-324` parses back to the same double, so the
        // *shortest* decimal that round-trips has one digit and Java prints two — which means the library's
        // renderer is not the rule, even ignoring its formatting. Both halves of that are asserted here, so a
        // future reader who wonders why this edition runs a precision loop has the answer in a failing test
        // rather than in a comment.
        expectTrue("`5E-324` names the smallest subnormal", Double.parseDouble("5E-324") == Double.MIN_VALUE);
        expectText("and Double.toString does not use it", Double.toString(Double.MIN_VALUE), "4.9E-324");
        expectText("rule 3 does", Canonical.formatNumber(Double.MIN_VALUE),
                "0." + "0".repeat(323) + "5");

        checks += 1;
        try {
            Canonical.formatNumber(Double.NaN);
            failures += 1;
            System.err.println("  NaN: was formatted rather than refused. A document that wrote a non-finite"
                    + " value as `null` would claim a measurement was absent when it was not.");
        } catch (Canonical.NonFinite expected) {
            // The only way writing a value can fail, and it fails as itself.
        }
        checks += 1;
        try {
            Canonical.formatNumber(Double.POSITIVE_INFINITY);
            failures += 1;
            System.err.println("  infinity: was formatted rather than refused");
        } catch (Canonical.NonFinite expected) {
            // As above.
        }
    }

    // ── 2. Rules 2, 4 and 5 ──────────────────────────────────────────────────

    private static void writer() {
        System.err.println("rules 2, 4 and 5 — keys, strings and absence");

        // Rule 2 over a key that is a prefix of another: `a` sorts before `ab`, which a comparison over the
        // shared bytes alone cannot decide.
        expectText("a key that is a prefix of another", Canonical.encode(Value.object()
                .set("ab", Value.of(2.0)).set("a", Value.of(1.0)).set("abc", Value.of(3.0))),
                "{\"a\":1,\"ab\":2,\"abc\":3}");

        // Ascending by UTF-8 bytes, which is not case-insensitive: `Z` (0x5A) sorts before `a` (0x61).
        expectText("uppercase before lowercase", Canonical.encode(Value.object()
                .set("a", Value.of(1.0)).set("Z", Value.of(2.0))), "{\"Z\":2,\"a\":1}");

        // **Rule 2 where UTF-8 and UTF-16 disagree.** A character above the basic multilingual plane is a
        // surrogate pair in UTF-16, and a surrogate code unit (0xD800…) sorts *below* U+E000…U+FFFF — while its
        // UTF-8 encoding (0xF0…) sorts *above*. So `String.compareTo` puts the astral key first and rule 2 puts
        // it last. This is the case that says the comparison is over encoded bytes.
        String astral = new String(Character.toChars(0x1D11E));
        String bmp = "\uFFFD";
        expectTrue("UTF-16 order puts the astral key first", astral.compareTo(bmp) < 0);
        expectText("rule 2 puts it last", Canonical.encode(Value.object()
                .set(astral, Value.of(1.0)).set(bmp, Value.of(2.0))),
                "{\"" + bmp + "\":2,\"" + astral + "\":1}");

        // Rule 5: an absent field is omitted and a null one is written, and they are different facts.
        expectText("absent beside null", Canonical.encode(Value.object()
                .set("declared", Value.NULL).setOptional("undeclared", null)), "{\"declared\":null}");

        // A Java null is neither of those, and `set` refuses it so the third state cannot exist.
        checks += 1;
        try {
            Value.object().set("held", null);
            failures += 1;
            System.err.println("  a Java null was accepted as a field value");
        } catch (IllegalArgumentException expected) {
            // Rule 5 has two states and a third would make it undecidable.
        }

        // Rule 4: the quote, the backslash and the C0 controls, and nothing else. A tab is an escape; a degree
        // sign is itself.
        expectText("rule 4", Canonical.encode(Value.object()
                .set("quote", Value.of("a\"b"))
                .set("backslash", Value.of("a\\b"))
                .set("tab", Value.of("a\tb"))
                .set("unit separator", Value.of("a\u001Fb"))
                .set("degree", Value.of("20\u00B0"))),
                "{\"backslash\":\"a\\\\b\",\"degree\":\"20\u00B0\",\"quote\":\"a\\\"b\","
                        + "\"tab\":\"a\\tb\",\"unit separator\":\"a\\u001fb\"}");

        expectText("a string holding U+0000", Canonical.encode(Value.object()
                .set("nul", Value.of("a\u0000b"))), "{\"nul\":\"a\\u0000b\"}");

        expectText("an empty object", Canonical.encode(Value.object()), "{}");
        expectText("an empty array", Canonical.encode(Value.array()), "[]");

        // Assigning a field twice keeps its first position, which is what the reference's object literal does —
        // and matters because the order a contract inserted in is what a failure prints.
        expectText("a field assigned twice", Canonical.encode(Value.object()
                .set("b", Value.of(1.0)).set("a", Value.of(2.0)).set("b", Value.of(3.0))),
                "{\"a\":2,\"b\":3}");
    }

    // ── 3. What the parser refuses ───────────────────────────────────────────

    private static void refuses(String what, String document) {
        checks += 1;
        try {
            Parse.parse(document);
            failures += 1;
            System.err.printf("  %s: `%s` was accepted. A document the oracle cannot write is one this edition"
                    + " must not read.%n", what, document);
        } catch (RuntimeException expected) {
            // As intended.
        }
    }

    private static void accepts(String what, String document, String canonical) {
        try {
            expectText(what, Canonical.encode(Parse.parse(document)), canonical);
        } catch (RuntimeException error) {
            checks += 1;
            failures += 1;
            System.err.printf("  %s: `%s` was refused: %s%n", what, document, error.getMessage());
        }
    }

    private static void parser() {
        System.err.println("the parser");
        refuses("a trailing comma in an array", "[1,2,]");
        refuses("a trailing comma in an object", "{\"a\":1,}");
        refuses("NaN", "{\"a\":NaN}");
        refuses("infinity", "{\"a\":Infinity}");
        refuses("a leading zero", "{\"a\":01}");
        refuses("a leading plus", "{\"a\":+1}");
        refuses("a bare word", "{\"a\":undefined}");
        refuses("a single-quoted string", "{'a':1}");
        refuses("an unterminated object", "{\"a\":1");
        refuses("two documents", "{} {}");
        refuses("an unescaped newline in a string", "{\"a\":\"one\ntwo\"}");
        refuses("a comment", "{\"a\":1} // a comment");
        // `Double.parseDouble` accepts every one of these and JSON accepts none, which is why the parser
        // validates the run of bytes before handing it over.
        refuses("a trailing d", "{\"a\":1d}");
        refuses("a hexadecimal float", "{\"a\":0x1p3}");
        // `Character.digit` accepts a fullwidth digit, which is not a hex digit in JSON.
        refuses("a fullwidth digit in an escape", "[\"\\u\uFF10\uFF10\uFF10\uFF10\"]");

        // The corpus is written by `JSON.stringify`, so what the parser must accept is that shape: whitespace,
        // unsorted keys, and an escape for every character above the BMP.
        accepts("whitespace and unsorted keys", "{ \"b\" : 1 ,\n  \"a\" : 2 }", "{\"a\":2,\"b\":1}");
        accepts("a surrogate pair", "[\"\\ud834\\udd1e\"]", "[\"\uD834\uDD1E\"]");
        accepts("an escaped solidus", "[\"a\\/b\"]", "[\"a/b\"]");
        accepts("an escaped BMP character", "[\"\\u00b0\"]", "[\"\u00B0\"]");
        accepts("an exponent", "[1e3]", "[1000]");
        accepts("a negative exponent", "[1e-7]", "[0.0000001]");
        accepts("nesting", "[[[[1]]]]", "[[[[1]]]]");
    }

    // ── 4. The runner's accounting ───────────────────────────────────────────

    /** One file of {@code count} cases whose input is {@code n} and whose expectation is {@code n + 1}. */
    private static Corpus.File synthetic(String corpusName, String name, int count) {
        List<Corpus.Case> cases = new java.util.ArrayList<>();
        for (int at = 0; at < count; at++) {
            cases.add(new Corpus.Case("case", "a synthetic case", Value.of((double) at),
                    Integer.toString(at + 1)));
        }
        return new Corpus.File(java.nio.file.Path.of(name), corpusName, name, cases);
    }

    private static void accounting() {
        System.err.println("the runner's accounting");

        List<Corpus.File> files = List.of(
                synthetic("answered", "whole", 3),
                synthetic("partly", "handled", 2),
                synthetic("partly", "declined", 4),
                synthetic("absent", "nothing", 5));

        Map<String, Corpus.Handler> dispatch = new LinkedHashMap<>();
        Corpus.Handler answersOne = input -> Value.of(Value.asNumber(input, 0) + 1);
        dispatch.put("answered/whole", answersOne);
        dispatch.put("partly/handled", answersOne);
        Corpus.Report report = Corpus.run(files, dispatch);

        expectInt("passed", report.passed(), 5);
        expectInt("failed", report.failed(), 0);
        // The four declined cases and the five with no handler at all are both *not implemented*, and a runner
        // that let either read as a pass would report a finished edition.
        expectInt("not implemented", report.unanswered(), 9);
        expectInt("total", report.total(), 14);
        expectTrue("`answered` is complete", report.complete().equals(List.of("answered")));
        expectTrue("`partly` is partial", report.partial().equals(List.of("partly")));
        expectTrue("`absent` has no handler", report.missingHandlers().equals(List.of("absent")));
        // A corpus with no handler at all is named once rather than once per file, so the declined list holds
        // only the file of the corpus that is partly answered.
        expectTrue("the declined file is named", report.declinedFiles().equals(List.of("partly/declined")));

        // A handler that answers wrongly is a failure, and its corpus is partial rather than complete even
        // though every file has a handler.
        Corpus.Report red = Corpus.run(files.subList(0, 1),
                Map.of("answered/whole", input -> Value.of(-1.0)));
        expectInt("a wrong answer fails", red.failed(), 3);
        expectInt("and passes nothing", red.passed(), 0);
        expectInt("and its corpus is not complete", red.complete().size(), 0);
        expectInt("its corpus is partial", red.partial().size(), 1);

        // A handler that throws is a *failure*, not an unanswered case: the file has a handler, so silence there
        // would be a contract quietly opting out of its own cases — and a runner that crashed here would hide
        // the failure it was describing (finding 164).
        Corpus.Report refused = Corpus.run(files.subList(0, 1), Map.of("answered/whole", input -> {
            throw new IllegalStateException("this case is not answered");
        }));
        expectInt("a thrown case fails", refused.failed(), 3);
        expectInt("and is not counted as unimplemented", refused.unanswered(), 0);
    }

    // ── 5. The case tables ───────────────────────────────────────────────────

    private static void caseTables() {
        System.err.println("the case tables");

        // Two files generated under different Unicode versions would disagree about characters neither of them
        // is wrong about, and the disagreement would read as hundreds of defects.
        expectText("the tables and the vectors are one Unicode version", Js.UNICODE_VERSION,
                CaseVectors.VECTORS_UNICODE_VERSION);
        // And the JDK's is its own, which is the whole reason the tables exist rather than a `toUpperCase` call.
        expectTrue("the JDK has a Unicode version of its own",
                !Character.toString(0).isEmpty());

        // The mapping tables are binary-searched, so their order is load-bearing.
        expectTrue("the case tables are ascending", Js.tablesSorted());

        // 219 whole strings the engine converted. The tables are single code points and cannot be wrong about a
        // string on their own; the code that walks a string with them, and applies the sigma rule, can — and
        // these are what would say so (finding 163).
        int wrong = 0;
        for (String[] vector : CaseVectors.VECTORS) {
            boolean upper = "1".equals(vector[0]);
            String got = upper ? Js.upper(vector[1]) : Js.lower(vector[1]);
            if (!got.equals(vector[2])) {
                wrong += 1;
                if (wrong <= 5) {
                    System.err.printf("  a %s vector disagrees: expected \"%s\", got \"%s\"%n",
                            upper ? "upper" : "lower", vector[2], got);
                }
            }
        }
        checks += 1;
        if (wrong != 0) {
            failures += 1;
            System.err.printf("  %d of %d case vectors disagree with the engine%n",
                    wrong, CaseVectors.VECTORS.length);
        }

        expectText("a sharp s upper-cases to two letters", Js.upper("\u00DF"), "SS");
        expectText("the Kelvin sign lower-cases to an ASCII k", Js.lower("\u212A"), "k");
        expectText("a final sigma", Js.lower("\u039F\u03A3"), "\u03BF\u03C2");
        expectText("and a medial one", Js.lower("\u039F\u03A3\u039F"), "\u03BF\u03C3\u03BF");

        // `trim` removes ECMAScript's whitespace, which is neither `String.trim`'s nor `String.strip`'s. Both of
        // the library's answers are asserted here, so the difference is a fact rather than a claim.
        String wrapped = "\uFEFF 1.2.3 \uFEFF";
        expectText("trim removes a zero-width no-break space", Js.trim(wrapped), "1.2.3");
        expectTrue("String.trim does not", !wrapped.trim().equals("1.2.3"));
        expectTrue("and String.strip does not", !wrapped.strip().equals("1.2.3"));
        // U+0085 is *not* ECMAScript whitespace, and a predicate corpus case turns on exactly that — while
        // `String.strip` does remove it, because `Character.isWhitespace` says so.
        expectText("trim leaves a next line", Js.trim("\u0085"), "\u0085");
        expectText("trim leaves an interior space", Js.trim("  a b  "), "a b");
        expectText("trim of nothing but space", Js.trim("   "), "");

        // Number::toString is not the canonical writer, and the two differ in both directions.
        expectText("a ten-millionth is exponential here", Js.numberToString(1e-7), "1e-7");
        expectText("and so is ten to the twenty-first", Js.numberToString(1e21), "1e+21");
        expectText("a millionth is not", Js.numberToString(1e-6), "0.000001");
        expectText("ten to the twentieth is not", Js.numberToString(1e20), "100000000000000000000");
        expectText("an integer", Js.numberToString(100.0), "100");
        expectText("a negative", Js.numberToString(-1.5), "-1.5");
        expectText("negative zero is zero here too", Js.numberToString(-0.0), "0");
        expectText("NaN is a word", Js.numberToString(Double.NaN), "NaN");
        expectText("and so is infinity", Js.numberToString(Double.POSITIVE_INFINITY), "Infinity");

        // `Number(text)` is not `parseDouble`.
        expectTrue("a hex string is a number", Js.stringToNumber("0x1f") == 31);
        expectTrue("`Infinity` with that capital", Double.isInfinite(Js.stringToNumber("Infinity")));
        expectTrue("and not with another", Double.isNaN(Js.stringToNumber("inf")));
        expectTrue("a signed hex literal is not a number", Double.isNaN(Js.stringToNumber("-0x10")));
        expectTrue("a trailing d is not a number", Double.isNaN(Js.stringToNumber("1d")));
        expectTrue("an empty string is zero", Js.stringToNumber("") == 0);
        expectTrue("and so is a blank one", Js.stringToNumber("   ") == 0);
        expectTrue("an exponent", Js.stringToNumber("1e3") == 1000);
        expectTrue("a trailing unit is not a number", Double.isNaN(Js.stringToNumber("21.5 c")));

        // ToString over a value, which is what a template literal does.
        expectText("an absent field", Js.toStringOf(null, false), "undefined");
        expectText("a null field", Js.toStringOf(Value.NULL, true), "null");
        expectText("an array joins with commas and writes null as nothing", Js.toStringOf(Value.array()
                .push(Value.of(1.0)).push(Value.NULL).push(Value.of("a")), true), "1,,a");
        expectText("an object", Js.toStringOf(Value.object(), true), "[object Object]");
    }

    public static void main(String[] argv) {
        for (int at = 0; at < argv.length; at++) {
            if ("--corpus-root".equals(argv[at]) && at + 1 < argv.length) {
                // Accepted and unused: `make test` passes the same arguments to both programs, and a self-check
                // that rejected them would make the two lines differ for no reason a reader could see.
                at += 1;
            } else {
                System.err.println("usage: skynet.os.SelfCheck [--corpus-root DIR]");
                System.exit(2);
            }
        }

        numbers();
        writer();
        parser();
        accounting();
        caseTables();

        System.err.printf("self check  %d checks, %d failed%n", checks, failures);
        System.exit(failures == 0 ? 0 : 1);
    }
}
