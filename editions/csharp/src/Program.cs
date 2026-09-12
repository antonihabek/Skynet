// The C# edition's entry point, and its three subcommands.
//
//   dotnet run --project editions/csharp -c Release -- roundtrip [--corpus-root corpus]
//   dotnet run --project editions/csharp -c Release -- selfcheck
//   dotnet run --project editions/csharp -c Release -- corpus    [--verbose] [--json] [--min-passed N]
//
// **roundtrip** — the canonical writer, proven before anything is built on it. Every `expected` string in the
// corpus is parsed and re-serialised, and the bytes must be identical. That is a stronger exam than it looks,
// and it is why this runs before the corpus runner:
//
// * **Rule 3 over every number the reference ever wrote.** `double.ToString("R")` has the right digits and the
//   wrong shape, so this edition reshapes every one of them; this step is what says the reshaping is right.
// * **Rule 2 over keys nobody chose.** The corpus holds documents whose keys collide on their first bytes and
//   differ in length, which is the comparison a shared-prefix compare gets wrong; and keys are compared as
//   UTF-8 bytes, which is not `string.CompareOrdinal`.
// * **Rule 4 over strings the generator escaped.** The corpus is written by `JSON.stringify`, which escapes
//   non-ASCII; this edition emits it as itself. A file that round-trips has had both halves agree.
//
// A round trip that fails is a defect in this edition and never in the corpus, because the corpus is the
// oracle's output and canonical by construction.
//
// **corpus** — exit code 0 only when every case in every corpus *that has a handler* passed, and the summary
// always says how many cases are not implemented, split into the corpora with no handler at all and the
// individual files a handler has not been written for. Those facts together are the honest sentence about this
// edition, and none of them is useful without the others.
//
// `--min-passed` is the ratchet, and it exists because everything above is a **floor of zero**. An edition
// whose handler was deleted answers nothing, fails nothing, and exits 0: the cases move from *passed* into
// *not implemented*, which is the bucket that reads as honest progress. That is the one regression this
// runner's own design cannot see, so the number a build passes here is the number this edition has reached.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;
using Skynet.Os.Contracts;

namespace Skynet.Os;

internal static class Program
{
    private const string Usage =
        "usage: skynet-corpus-csharp (roundtrip | selfcheck | corpus) [--corpus-root DIR] [--verbose]"
            + " [--json] [--min-passed N]";

    private static void PrintList(string heading, List<string> values)
    {
        if (values.Count == 0)
        {
            return;
        }
        Console.Error.WriteLine(heading + " " + string.Join(", ", values));
    }

    private static Value Strings(List<string> values)
    {
        Value out_ = Value.Array();
        foreach (string held in values)
        {
            out_.Push(Value.Of(held));
        }
        return out_;
    }

    /// <summary>
    /// The corpus, or null after printing why there is none. Both callers need the same three sentences, and a
    /// runner that measured an empty corpus would exit 0 having proved nothing.
    /// </summary>
    private static List<CorpusFile>? Corpora(string? rootArgument)
    {
        string? corpusRoot = Corpus.Root(rootArgument);
        if (corpusRoot is null)
        {
            Console.Error.WriteLine("no corpus found: pass --corpus-root, or set SKYNET_CORPUS");
            return null;
        }
        List<CorpusFile> files = Corpus.Load(corpusRoot);
        if (files.Count == 0)
        {
            Console.Error.WriteLine("no corpus files under " + corpusRoot);
            return null;
        }
        Console.Error.WriteLine("corpus     " + corpusRoot);
        return files;
    }

    private static int Roundtrip(string? rootArgument)
    {
        List<CorpusFile>? files = Corpora(rootArgument);
        if (files is null)
        {
            return 2;
        }

        int checked_ = 0;
        int failed = 0;
        foreach (CorpusFile file in files)
        {
            foreach (CorpusCase vector in file.Cases)
            {
                string encoded;
                try
                {
                    encoded = Canonical.Encode(Parse.Document(vector.Expected));
                }
                catch (ParseException error)
                {
                    failed += 1;
                    Console.Error.WriteLine(file.Key + " :: " + vector.Id
                        + "\n  the expectation could not be re-serialised: " + error.Message);
                    continue;
                }
                catch (NonFiniteNumberException error)
                {
                    failed += 1;
                    Console.Error.WriteLine(file.Key + " :: " + vector.Id
                        + "\n  the expectation could not be re-serialised: " + error.Message);
                    continue;
                }
                if (!string.Equals(encoded, vector.Expected, StringComparison.Ordinal))
                {
                    failed += 1;
                    Console.Error.WriteLine(Corpus.Describe(new Failure(file.Key, vector.Id,
                        "a canonical document did not survive a parse and a write",
                        vector.Expected, encoded, null)) + "\n");
                    continue;
                }
                checked_ += 1;
            }
        }

        Console.Error.WriteLine("round trip  " + checked_.ToString(CultureInfo.InvariantCulture) + " of "
            + (checked_ + failed).ToString(CultureInfo.InvariantCulture)
            + " expectations re-serialised identically");
        return failed == 0 ? 0 : 1;
    }

    private static int RunCorpus(string? rootArgument, bool verbose, bool asJson, long minPassed)
    {
        List<CorpusFile>? files = Corpora(rootArgument);
        if (files is null)
        {
            return 2;
        }

        Report report = Corpus.Run(files, Dispatch.Entries());

        if (asJson)
        {
            // Deliberately the same fields the human report prints, and no derived ones: a summary that computed
            // a percentage here and a page that computed it differently would be two answers to one question.
            Value out_ = Value.Object();
            out_.Set("passed", Value.Of((double)report.Passed));
            out_.Set("failed", Value.Of((double)report.Failed));
            out_.Set("unimplemented", Value.Of((double)report.Unanswered));
            out_.Set("total", Value.Of((double)report.Total));
            out_.Set("complete", Strings(report.Complete));
            out_.Set("partial", Strings(report.Partial));
            out_.Set("missing_handlers", Strings(report.MissingHandlers));
            out_.Set("declined_files", Strings(report.DeclinedFiles));
            // stdout, because this is the output a tool parses; everything else here goes to stderr.
            Console.Out.WriteLine(Canonical.Encode(out_));
            return report.Failed == 0 ? 0 : 1;
        }

        Console.Error.WriteLine("result     " + report.Passed.ToString(CultureInfo.InvariantCulture)
            + " passed, " + report.Failed.ToString(CultureInfo.InvariantCulture) + " failed, "
            + report.Unanswered.ToString(CultureInfo.InvariantCulture) + " not implemented, of "
            + report.Total.ToString(CultureInfo.InvariantCulture));
        PrintList("complete  ", report.Complete);
        PrintList("no handler", report.MissingHandlers);
        PrintList("declined  ", report.DeclinedFiles);
        // Named separately from `complete` on purpose: these are corpora this edition answers part of, and a
        // reader who saw only the passing files would take them for finished ones.
        PrintList("partial   ", report.Partial);

        if (report.Failures.Count != 0)
        {
            Console.Error.WriteLine();
            int shown = verbose ? report.Failures.Count : Math.Min(report.Failures.Count, 10);
            for (int at = 0; at < shown; at++)
            {
                Console.Error.WriteLine(Corpus.Describe(report.Failures[at]) + "\n");
            }
            if (shown < report.Failures.Count)
            {
                Console.Error.WriteLine("... and "
                    + (report.Failures.Count - shown).ToString(CultureInfo.InvariantCulture)
                    + " more (pass --verbose)");
            }
        }

        if (minPassed >= 0 && report.Passed < minPassed)
        {
            Console.Error.WriteLine("\nregression: " + report.Passed.ToString(CultureInfo.InvariantCulture)
                + " passed, and this edition had reached " + minPassed.ToString(CultureInfo.InvariantCulture)
                + ". Cases that stop being answered are counted as *not implemented* rather than as failures,"
                + " so nothing above this line went red — which is why the floor is checked separately from the"
                + " failures.");
            return 1;
        }
        return report.Failed == 0 ? 0 : 1;
    }

    internal static int Main(string[] argv)
    {
        // UTF-8 on both streams, set here rather than left to the terminal. A Windows console is code page 437
        // or 1252 by default, and a failure report whose expected and actual differ only in a character the
        // console cannot render is a report that reads as identical — which is the one thing a diff must never
        // do. (`Console.OutputEncoding` is also what makes `--json` pipe correctly into a file.)
        Console.OutputEncoding = new UTF8Encoding(false);
        Console.InputEncoding = new UTF8Encoding(false);

        if (argv.Length == 0)
        {
            Console.Error.WriteLine(Usage);
            return 2;
        }

        string command = argv[0];
        string? rootArgument = null;
        bool verbose = false;
        bool asJson = false;
        long minPassed = -1;

        for (int at = 1; at < argv.Length; at++)
        {
            switch (argv[at])
            {
                case "--corpus-root":
                    if (at + 1 >= argv.Length)
                    {
                        Console.Error.WriteLine(Usage);
                        return 2;
                    }
                    rootArgument = argv[++at];
                    break;
                case "--verbose":
                    verbose = true;
                    break;
                case "--json":
                    asJson = true;
                    break;
                case "--min-passed":
                    if (at + 1 >= argv.Length
                        || !long.TryParse(argv[at + 1], NumberStyles.Integer, CultureInfo.InvariantCulture,
                            out minPassed))
                    {
                        Console.Error.WriteLine(Usage);
                        return 2;
                    }
                    at += 1;
                    break;
                default:
                    Console.Error.WriteLine(Usage);
                    return 2;
            }
        }

        switch (command)
        {
            case "roundtrip":
                return Roundtrip(rootArgument);
            case "selfcheck":
                // `--corpus-root` is accepted and unused here: the Makefile passes the same arguments to all
                // three, and a self-check that rejected them would make the lines differ for no reason a reader
                // could see.
                return SelfCheck.Run();
            case "corpus":
                return RunCorpus(rootArgument, verbose, asJson, minPassed);
            default:
                Console.Error.WriteLine(Usage);
                return 2;
        }
    }
}
