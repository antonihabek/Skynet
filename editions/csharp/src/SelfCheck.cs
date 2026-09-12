// The C# edition checking itself, over the things the corpus cannot reach.
//
//   dotnet run --project editions/csharp -c Release -- selfcheck
//
// Six groups, and each exists because a corpus run would be green without it:
//
// 1. **Rule 3 at its edges**, including the two ways `double.ToString("R")` is not rule 3 even though its
//    digits are the right ones. See `Numbers()`.
// 2. **Rules 2, 4 and 5 as facts rather than consequences** — including rule 2 over a key above the basic
//    multilingual plane, which is where UTF-8 byte order and UTF-16 code-unit order part company and where
//    `string.CompareOrdinal` would answer differently.
// 3. **What the parser refuses.** A parser that accepted these would read documents the oracle cannot write,
//    and no corpus file would say so.
// 4. **The runner's own accounting**, over synthetic corpora with known answers.
// 5. **The case tables**, against 219 whole strings the reference's engine converted, and beside the
//    framework's own casing — which is the *simple* mapping and disagrees with the engine.
// 6. **No fused multiply-add**, which on this platform is a source scan with a positive control rather than a
//    compiler flag: see `Fusion()`.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using Skynet.Os.Contracts;

namespace Skynet.Os;

internal static class SelfCheck
{
    private static int checks;
    private static int failures;

    private static void ExpectText(string what, string actual, string wanted)
    {
        checks += 1;
        if (!string.Equals(actual, wanted, StringComparison.Ordinal))
        {
            failures += 1;
            Console.Error.WriteLine("  " + what + ": expected \"" + wanted + "\", got \"" + actual + "\"");
        }
    }

    private static void ExpectTrue(string what, bool truth)
    {
        checks += 1;
        if (!truth)
        {
            failures += 1;
            Console.Error.WriteLine("  " + what + ": expected true");
        }
    }

    private static void ExpectInt(string what, long actual, long wanted)
    {
        checks += 1;
        if (actual != wanted)
        {
            failures += 1;
            Console.Error.WriteLine("  " + what + ": expected "
                + wanted.ToString(CultureInfo.InvariantCulture) + ", got "
                + actual.ToString(CultureInfo.InvariantCulture));
        }
    }

    private static string Round(double value) => value.ToString("R", CultureInfo.InvariantCulture);

    // ── 1. Rule 3 ────────────────────────────────────────────────────────────

    private static void CheckNumber(string what, double number, string wanted) =>
        ExpectText(what, Canonical.FormatNumber(number), wanted);

    private static void Numbers()
    {
        Console.Error.WriteLine("rule 3 — numbers");
        CheckNumber("zero", 0.0, "0");
        // `ToString("R")` writes `-0` here, and rule 3 writes `0`.
        CheckNumber("negative zero", -0.0, "0");
        CheckNumber("one", 1.0, "1");
        CheckNumber("minus one", -1.0, "-1");
        CheckNumber("a hundred", 100.0, "100");
        CheckNumber("2^53 - 2", 9007199254740990.0, "9007199254740990");
        CheckNumber("2^53", 9007199254740992.0, "9007199254740992");
        CheckNumber("a tenth", 0.1, "0.1");
        CheckNumber("a tenth and two", 0.1 + 0.2, "0.30000000000000004");
        // The reference's own example of why `canonical.ts` exists rather than a `JSON.stringify` with a
        // replacer: the engine writes `1e-7`, and rule 3 requires a positional rendering.
        CheckNumber("a ten-millionth", 1e-7, "0.0000001");
        CheckNumber("ten to the twenty-first", 1e21, "1000000000000000000000");
        CheckNumber("a third", 1.0 / 3.0, "0.3333333333333333");
        CheckNumber("a large mixed value", 1234567890.1234567, "1234567890.1234567");

        // **The two ways `ToString("R")` is not rule 3.**
        //
        // Its *digits* are right — .NET Core 3.0 onwards, `"R"` is the shortest decimal that round-trips, which
        // is why this edition does not run the precision loop the C, C++ and Java editions run. (Java's
        // `Double.toString` gives `4.9E-324` for the smallest subnormal where `5E-324` round-trips, so that
        // edition cannot use its library at all.) Its *shape* is wrong twice: it is exponential wherever the
        // exponent is outside a narrow band, and it writes a sign on negative zero. Both are asserted here, so a
        // reader who wonders why `FormatNumber` reshapes a string it already trusts has the answer in a test.
        ExpectText("`R` gives the shortest round-trip digits", Round(double.Epsilon), "5E-324");
        ExpectTrue("and that string is the smallest subnormal",
            double.Parse("5E-324", NumberStyles.Float, CultureInfo.InvariantCulture) == double.Epsilon);
        ExpectText("rule 3 renders it positionally", Canonical.FormatNumber(double.Epsilon),
            "0." + new string('0', 323) + "5");
        ExpectText("`R` is exponential at a ten-millionth", Round(1e-7), "1E-07");
        ExpectText("and at ten to the twentieth", Round(1e20), "1E+20");
        ExpectText("and it signs negative zero", Round(-0.0), "-0");

        checks += 1;
        try
        {
            Canonical.FormatNumber(double.NaN);
            failures += 1;
            Console.Error.WriteLine("  NaN: was formatted rather than refused. A document that wrote a"
                + " non-finite value as `null` would claim a measurement was absent when it was not.");
        }
        catch (NonFiniteNumberException)
        {
            // The only way writing a value can fail, and it fails as itself.
        }
        checks += 1;
        try
        {
            Canonical.FormatNumber(double.PositiveInfinity);
            failures += 1;
            Console.Error.WriteLine("  infinity: was formatted rather than refused");
        }
        catch (NonFiniteNumberException)
        {
            // As above.
        }
    }

    // ── 2. Rules 2, 4 and 5 ──────────────────────────────────────────────────

    private static void Writer()
    {
        Console.Error.WriteLine("rules 2, 4 and 5 — keys, strings and absence");

        // Rule 2 over a key that is a prefix of another: `a` sorts before `ab`, which a comparison over the
        // shared bytes alone cannot decide.
        ExpectText("a key that is a prefix of another", Canonical.Encode(Value.Object()
            .Set("ab", Value.Of(2.0)).Set("a", Value.Of(1.0)).Set("abc", Value.Of(3.0))),
            "{\"a\":1,\"ab\":2,\"abc\":3}");

        // Ascending by UTF-8 bytes, which is not case-insensitive: `Z` (0x5A) sorts before `a` (0x61).
        ExpectText("uppercase before lowercase", Canonical.Encode(Value.Object()
            .Set("a", Value.Of(1.0)).Set("Z", Value.Of(2.0))), "{\"Z\":2,\"a\":1}");

        // **Rule 2 where UTF-8 and UTF-16 disagree.** A character above the basic multilingual plane is a
        // surrogate pair in UTF-16, and a surrogate code unit (0xD800…) sorts *below* U+E000…U+FFFF — while its
        // UTF-8 encoding (0xF0…) sorts *above*. So `string.CompareOrdinal` puts the astral key first and rule 2
        // puts it last. This is the case that says the comparison is over encoded bytes.
        string astral = char.ConvertFromUtf32(0x1D11E);
        const string Bmp = "\uFFFD";
        ExpectTrue("UTF-16 order puts the astral key first", string.CompareOrdinal(astral, Bmp) < 0);
        ExpectText("rule 2 puts it last", Canonical.Encode(Value.Object()
            .Set(astral, Value.Of(1.0)).Set(Bmp, Value.Of(2.0))),
            "{\"" + Bmp + "\":2,\"" + astral + "\":1}");

        // Rule 5: an absent field is omitted and a null one is written, and they are different facts.
        ExpectText("absent beside null", Canonical.Encode(Value.Object()
            .Set("declared", Value.Null).SetOptional("undeclared", null)), "{\"declared\":null}");

        // A C# null is neither of those, and `Set` refuses it so the third state cannot exist. The cast is what
        // makes the call compile at all under `Nullable enable`, which is the gate doing its job — this check is
        // for the caller who reaches the same place through an unannotated boundary.
        checks += 1;
        try
        {
            Value.Object().Set("held", null!);
            failures += 1;
            Console.Error.WriteLine("  a C# null was accepted as a field value");
        }
        catch (ArgumentNullException)
        {
            // Rule 5 has two states and a third would make it undecidable.
        }

        // Rule 4: the quote, the backslash and the C0 controls, and nothing else. A tab is an escape; a degree
        // sign is itself.
        ExpectText("rule 4", Canonical.Encode(Value.Object()
            .Set("quote", Value.Of("a\"b"))
            .Set("backslash", Value.Of("a\\b"))
            .Set("tab", Value.Of("a\tb"))
            .Set("unit separator", Value.Of("a\u001Fb"))
            .Set("degree", Value.Of("20\u00B0"))),
            "{\"backslash\":\"a\\\\b\",\"degree\":\"20\u00B0\",\"quote\":\"a\\\"b\","
                + "\"tab\":\"a\\tb\",\"unit separator\":\"a\\u001fb\"}");

        ExpectText("a string holding U+0000", Canonical.Encode(Value.Object()
            .Set("nul", Value.Of("a\u0000b"))), "{\"nul\":\"a\\u0000b\"}");

        ExpectText("an empty object", Canonical.Encode(Value.Object()), "{}");
        ExpectText("an empty array", Canonical.Encode(Value.Array()), "[]");

        // Assigning a field twice keeps its first position, which is what the reference's object literal does —
        // and matters because the order a contract inserted in is what a failure prints.
        ExpectText("a field assigned twice", Canonical.Encode(Value.Object()
            .Set("b", Value.Of(1.0)).Set("a", Value.Of(2.0)).Set("b", Value.Of(3.0))),
            "{\"a\":2,\"b\":3}");
    }

    // ── 3. What the parser refuses ───────────────────────────────────────────

    private static void Refuses(string what, string document)
    {
        checks += 1;
        try
        {
            Parse.Document(document);
            failures += 1;
            Console.Error.WriteLine("  " + what + ": `" + document + "` was accepted. A document the oracle"
                + " cannot write is one this edition must not read.");
        }
        catch (ParseException)
        {
            // As intended.
        }
    }

    private static void Accepts(string what, string document, string canonical)
    {
        try
        {
            ExpectText(what, Canonical.Encode(Parse.Document(document)), canonical);
        }
        catch (ParseException error)
        {
            checks += 1;
            failures += 1;
            Console.Error.WriteLine("  " + what + ": `" + document + "` was refused: " + error.Message);
        }
    }

    private static void Parser()
    {
        Console.Error.WriteLine("the parser");
        Refuses("a trailing comma in an array", "[1,2,]");
        Refuses("a trailing comma in an object", "{\"a\":1,}");
        Refuses("NaN", "{\"a\":NaN}");
        Refuses("infinity", "{\"a\":Infinity}");
        Refuses("a leading zero", "{\"a\":01}");
        Refuses("a leading plus", "{\"a\":+1}");
        Refuses("a bare word", "{\"a\":undefined}");
        Refuses("a single-quoted string", "{'a':1}");
        Refuses("an unterminated object", "{\"a\":1");
        Refuses("two documents", "{} {}");
        Refuses("an unescaped newline in a string", "{\"a\":\"one\ntwo\"}");
        Refuses("a comment", "{\"a\":1} // a comment");
        // `double.Parse` with `NumberStyles.Float` accepts a leading sign, whitespace and a thousands-free
        // decimal with a currency-free exponent — and JSON accepts none of the shapes below, which is why the
        // parser validates the run of bytes before handing it over.
        Refuses("a trailing d", "{\"a\":1d}");
        Refuses("a hexadecimal float", "{\"a\":0x1p3}");
        // `char.IsDigit` accepts a fullwidth digit, which is not a hex digit in JSON.
        Refuses("a fullwidth digit in an escape", "[\"\\u\uFF10\uFF10\uFF10\uFF10\"]");

        // The corpus is written by `JSON.stringify`, so what the parser must accept is that shape: whitespace,
        // unsorted keys, and an escape for every character above the BMP.
        Accepts("whitespace and unsorted keys", "{ \"b\" : 1 ,\n  \"a\" : 2 }", "{\"a\":2,\"b\":1}");
        Accepts("a surrogate pair", "[\"\\ud834\\udd1e\"]", "[\"\uD834\uDD1E\"]");
        Accepts("an escaped solidus", "[\"a\\/b\"]", "[\"a/b\"]");
        Accepts("an escaped BMP character", "[\"\\u00b0\"]", "[\"\u00B0\"]");
        Accepts("an exponent", "[1e3]", "[1000]");
        Accepts("a negative exponent", "[1e-7]", "[0.0000001]");
        Accepts("nesting", "[[[[1]]]]", "[[[[1]]]]");
    }

    // ── 4. The runner's accounting ───────────────────────────────────────────

    /// <summary>One file of <c>count</c> cases whose input is <c>n</c> and whose expectation is <c>n + 1</c>.</summary>
    private static CorpusFile Synthetic(string corpusName, string name, int count)
    {
        List<CorpusCase> cases = new();
        for (int at = 0; at < count; at++)
        {
            cases.Add(new CorpusCase("case", "a synthetic case", Value.Of((double)at),
                (at + 1).ToString(CultureInfo.InvariantCulture)));
        }
        return new CorpusFile(name, corpusName, name, cases);
    }

    private static bool SameList(List<string> actual, params string[] wanted)
    {
        if (actual.Count != wanted.Length)
        {
            return false;
        }
        for (int at = 0; at < wanted.Length; at++)
        {
            if (!string.Equals(actual[at], wanted[at], StringComparison.Ordinal))
            {
                return false;
            }
        }
        return true;
    }

    private static void Accounting()
    {
        Console.Error.WriteLine("the runner's accounting");

        List<CorpusFile> files = new()
        {
            Synthetic("answered", "whole", 3),
            Synthetic("partly", "handled", 2),
            Synthetic("partly", "declined", 4),
            Synthetic("absent", "nothing", 5),
        };

        static Value AnswersOne(Value? input) => Value.Of(Value.AsNumber(input, 0) + 1);
        Dictionary<string, Corpus.Handler> dispatch = new(StringComparer.Ordinal)
        {
            ["answered/whole"] = AnswersOne,
            ["partly/handled"] = AnswersOne,
        };
        Report report = Corpus.Run(files, dispatch);

        ExpectInt("passed", report.Passed, 5);
        ExpectInt("failed", report.Failed, 0);
        // The four declined cases and the five with no handler at all are both *not implemented*, and a runner
        // that let either read as a pass would report a finished edition.
        ExpectInt("not implemented", report.Unanswered, 9);
        ExpectInt("total", report.Total, 14);
        ExpectTrue("`answered` is complete", SameList(report.Complete, "answered"));
        ExpectTrue("`partly` is partial", SameList(report.Partial, "partly"));
        ExpectTrue("`absent` has no handler", SameList(report.MissingHandlers, "absent"));
        // A corpus with no handler at all is named once rather than once per file, so the declined list holds
        // only the file of the corpus that is partly answered.
        ExpectTrue("the declined file is named", SameList(report.DeclinedFiles, "partly/declined"));

        // A handler that answers wrongly is a failure, and its corpus is partial rather than complete even
        // though every file has a handler.
        List<CorpusFile> one = new() { files[0] };
        Report red = Corpus.Run(one, new Dictionary<string, Corpus.Handler>(StringComparer.Ordinal)
        {
            ["answered/whole"] = _ => Value.Of(-1.0),
        });
        ExpectInt("a wrong answer fails", red.Failed, 3);
        ExpectInt("and passes nothing", red.Passed, 0);
        ExpectInt("and its corpus is not complete", red.Complete.Count, 0);
        ExpectInt("its corpus is partial", red.Partial.Count, 1);

        // A handler that throws is a *failure*, not an unanswered case: the file has a handler, so silence there
        // would be a contract quietly opting out of its own cases — and a runner that crashed here would hide
        // the failure it was describing (finding 164).
        Report refused = Corpus.Run(one, new Dictionary<string, Corpus.Handler>(StringComparer.Ordinal)
        {
            ["answered/whole"] = _ => throw new InvalidOperationException("this case is not answered"),
        });
        ExpectInt("a thrown case fails", refused.Failed, 3);
        ExpectInt("and is not counted as unimplemented", refused.Unanswered, 0);
    }

    // ── 5. The case tables ───────────────────────────────────────────────────

    private static void CaseTables()
    {
        Console.Error.WriteLine("the case tables");

        // Two files generated under different Unicode versions would disagree about characters neither of them
        // is wrong about, and the disagreement would read as hundreds of defects.
        ExpectText("the tables and the vectors are one Unicode version", Js.UnicodeVersion,
            CaseVectors.VectorsUnicodeVersion);

        // The mapping tables are binary-searched, so their order is load-bearing.
        ExpectTrue("the case tables are ascending", Js.TablesSorted());

        // 219 whole strings the engine converted. The tables are single code points and cannot be wrong about a
        // string on their own; the code that walks a string with them, and applies the sigma rule, can — and
        // these are what would say so (finding 163).
        int wrong = 0;
        foreach (string[] vector in CaseVectors.Vectors)
        {
            bool upper = string.Equals(vector[0], "1", StringComparison.Ordinal);
            string got = upper ? Js.Upper(vector[1]) : Js.Lower(vector[1]);
            if (!string.Equals(got, vector[2], StringComparison.Ordinal))
            {
                wrong += 1;
                if (wrong <= 5)
                {
                    Console.Error.WriteLine("  a " + (upper ? "upper" : "lower")
                        + " vector disagrees: expected \"" + vector[2] + "\", got \"" + got + "\"");
                }
            }
        }
        checks += 1;
        if (wrong != 0)
        {
            failures += 1;
            Console.Error.WriteLine("  " + wrong.ToString(CultureInfo.InvariantCulture) + " of "
                + CaseVectors.Vectors.Length.ToString(CultureInfo.InvariantCulture)
                + " case vectors disagree with the engine");
        }

        // **And the framework's own casing, asserted rather than assumed.** `ToUpperInvariant` is the *simple*
        // mapping: one code point in, one out, no context. So the sharp s is unchanged where the engine writes
        // two letters, and there is no final sigma at all. This is the whole reason the tables exist.
        const string SharpS = "\u00DF";
        ExpectText("a sharp s upper-cases to two letters", Js.Upper(SharpS), "SS");
        ExpectText("and ToUpperInvariant leaves it alone", SharpS.ToUpperInvariant(), SharpS);
        ExpectText("the Kelvin sign lower-cases to an ASCII k", Js.Lower("\u212A"), "k");
        ExpectText("a final sigma", Js.Lower("\u039F\u03A3"), "\u03BF\u03C2");
        ExpectText("and ToLowerInvariant does not apply the rule", "\u039F\u03A3".ToLowerInvariant(),
            "\u03BF\u03C3");
        ExpectText("and a medial one", Js.Lower("\u039F\u03A3\u039F"), "\u03BF\u03C3\u03BF");

        // A lone surrogate is a string a JavaScript engine can hold and therefore one a corpus document can
        // carry. `char.ConvertToUtf32` throws on it, so every walk in `Js` reads code points its own way — and
        // this is the check that says so rather than a comment claiming it.
        const string LoneHigh = "a\uD834b";
        ExpectText("a lone surrogate survives upper-casing", Js.Upper(LoneHigh), "A\uD834B");
        ExpectText("and lower-casing", Js.Lower("A\uD834B"), LoneHigh);
        ExpectText("and trimming", Js.Trim(" \uD834 "), "\uD834");

        // `Trim` removes ECMAScript's whitespace, which is **the exact inverse of `string.Trim` on both of the
        // characters that distinguish them**: U+FEFF is ECMAScript whitespace and not `char.IsWhiteSpace`, and
        // U+0085 is `char.IsWhiteSpace` and not ECMAScript whitespace. A predicate corpus case turns on the
        // second. Both halves are asserted, so the difference is a fact rather than a claim.
        const string Wrapped = "\uFEFF 1.2.3 \uFEFF";
        ExpectText("trim removes a zero-width no-break space", Js.Trim(Wrapped), "1.2.3");
        ExpectTrue("string.Trim does not",
            !string.Equals(Wrapped.Trim(), "1.2.3", StringComparison.Ordinal));
        ExpectText("trim leaves a next line", Js.Trim("\u0085"), "\u0085");
        ExpectTrue("string.Trim removes it", "\u0085".Trim().Length == 0);
        ExpectText("trim leaves an interior space", Js.Trim("  a b  "), "a b");
        ExpectText("trim of nothing but space", Js.Trim("   "), string.Empty);

        // Number::toString is not the canonical writer, and the two differ in both directions.
        ExpectText("a ten-millionth is exponential here", Js.NumberToString(1e-7), "1e-7");
        ExpectText("and so is ten to the twenty-first", Js.NumberToString(1e21), "1e+21");
        ExpectText("a millionth is not", Js.NumberToString(1e-6), "0.000001");
        ExpectText("ten to the twentieth is not", Js.NumberToString(1e20), "100000000000000000000");
        ExpectText("an integer", Js.NumberToString(100.0), "100");
        ExpectText("a negative", Js.NumberToString(-1.5), "-1.5");
        ExpectText("negative zero is zero here too", Js.NumberToString(-0.0), "0");
        ExpectText("NaN is a word", Js.NumberToString(double.NaN), "NaN");
        ExpectText("and so is infinity", Js.NumberToString(double.PositiveInfinity), "Infinity");
        // Both shapes the framework would have written instead, at the two exponents where it differs.
        ExpectText("the framework disagrees at a millionth", Round(1e-6), "1E-06");
        ExpectText("and at ten to the twentieth", Round(1e20), "1E+20");

        // `Number(text)` is not `double.Parse`.
        ExpectTrue("a hex string is a number", Js.StringToNumber("0x1f") == 31);
        ExpectTrue("`Infinity` with that capital", double.IsInfinity(Js.StringToNumber("Infinity")));
        ExpectTrue("and not with another", double.IsNaN(Js.StringToNumber("inf")));
        ExpectTrue("a signed hex literal is not a number", double.IsNaN(Js.StringToNumber("-0x10")));
        ExpectTrue("a trailing d is not a number", double.IsNaN(Js.StringToNumber("1d")));
        ExpectTrue("an empty string is zero", Js.StringToNumber(string.Empty) == 0);
        ExpectTrue("and so is a blank one", Js.StringToNumber("   ") == 0);
        ExpectTrue("an exponent", Js.StringToNumber("1e3") == 1000);
        ExpectTrue("a trailing unit is not a number", double.IsNaN(Js.StringToNumber("21.5 c")));

        // ToString over a value, which is what a template literal does.
        ExpectText("an absent field", Js.ToStringOf(null, false), "undefined");
        ExpectText("a null field", Js.ToStringOf(Value.Null, true), "null");
        ExpectText("an array joins with commas and writes null as nothing", Js.ToStringOf(Value.Array()
            .Push(Value.Of(1.0)).Push(Value.Null).Push(Value.Of("a")), true), "1,,a");
        ExpectText("an object", Js.ToStringOf(Value.Object(), true), "[object Object]");
    }

    // ── 6. No fused multiply-add ─────────────────────────────────────────────

    /// <summary>
    /// The source tree this assembly was built from, found by walking up from the assembly's own directory to
    /// the project file beside it. Null when the assembly was moved away from its sources, which the caller
    /// reports as a failed check rather than a skipped one.
    /// </summary>
    private static string? SourceRoot()
    {
        string? here = AppContext.BaseDirectory;
        for (int levels = 0; levels < 8 && here is not null; levels++)
        {
            if (File.Exists(Path.Combine(here, "SkynetOs.csproj")))
            {
                string src = Path.Combine(here, "src");
                return Directory.Exists(src) ? src : null;
            }
            here = Path.GetDirectoryName(here);
        }
        return null;
    }

    /// <summary>
    /// §31.2 pins the kernel's arithmetic as a fixed sequence of correctly rounded operations, and a fused
    /// multiply-add is a different sequence: it rounds once where the specification rounds twice.
    /// <para>
    /// The C, C++ and Zig editions switch contraction off at the compiler and then scan the object code, because
    /// those compilers may fuse on their own. The CLR may not — its <c>mul</c> and <c>add</c> on float64 are
    /// separate IEEE-754 operations — so there is no flag here and nothing to find in the IL. What there is, is
    /// <c>Math.FusedMultiplyAdd</c>, which fuses when a caller asks for it. So the rule is that nothing asks,
    /// and this scans the sources to say so.
    /// </para>
    /// <para>
    /// <b>With a positive control</b>, which is finding 171: a scan that finds nothing proves nothing unless
    /// something would have been found. Here the control is arithmetic rather than a compiled probe — a triple
    /// where the fused result and the stepwise one differ, which is what makes the ban worth having. If a future
    /// runtime stopped fusing that call, this check would fail rather than quietly passing for the wrong reason.
    /// </para>
    /// </summary>
    private static void Fusion()
    {
        Console.Error.WriteLine("no fused multiply-add");

        double a = 0.1;
        double b = 0.2;
        // The rounded product, negated: the stepwise sequence therefore cancels to exactly zero, and the fused
        // one keeps the bits rounding away.
        double c = -(a * b);
        double fused = Math.FusedMultiplyAdd(a, b, c);
        double stepwise = (a * b) + c;
        ExpectText("the stepwise sequence cancels", Canonical.FormatNumber(stepwise), "0");
        ExpectTrue("and the fused one does not — so fusing is observable here", fused != stepwise);

        string? root = SourceRoot();
        checks += 1;
        if (root is null)
        {
            failures += 1;
            Console.Error.WriteLine("  the sources could not be found next to this assembly, so the scan"
                + " below would have passed by finding nothing to read");
            return;
        }

        // The call this file makes is the control above, so it is the one file exempted — and named, rather than
        // matched by a pattern that a second file could accidentally satisfy.
        string mine = Path.GetFileName(root) + Path.DirectorySeparatorChar + "SelfCheck.cs";
        int found = 0;
        foreach (string path in Directory.GetFiles(root, "*.cs", SearchOption.AllDirectories))
        {
            if (path.EndsWith(mine, StringComparison.Ordinal))
            {
                continue;
            }
            string text = File.ReadAllText(path);
            if (text.Contains("FusedMultiplyAdd", StringComparison.Ordinal))
            {
                found += 1;
                Console.Error.WriteLine("  " + Path.GetFileName(path) + " names FusedMultiplyAdd. §31.2 pins"
                    + " the arithmetic as a fixed sequence of correctly rounded operations, and a fused"
                    + " operation rounds once where that sequence rounds twice.");
            }
        }
        if (found != 0)
        {
            failures += 1;
        }
    }

    internal static int Run()
    {
        Numbers();
        Writer();
        Parser();
        Accounting();
        CaseTables();
        Fusion();

        Console.Error.WriteLine("self check  " + checks.ToString(CultureInfo.InvariantCulture) + " checks, "
            + failures.ToString(CultureInfo.InvariantCulture) + " failed");
        return failures == 0 ? 0 : 1;
    }
}
