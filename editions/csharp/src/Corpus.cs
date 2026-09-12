// Reading and running the conformance corpus.
//
// Split from the contracts because two programs read the corpus and only one of them computes anything: the
// `roundtrip` subcommand re-serialises every `expected` string before a single contract exists, and `corpus`
// runs the contracts against it. One reader means the two cannot disagree about which files the corpus has.
//
// # The four buckets, and why none of them is useful alone
//
// §31.4: an edition that has not passed the corpus is not an edition. A runner that printed only failures
// would report an edition with one handler as flawless, so a report here says how many cases *passed*, how
// many *failed*, and how many are *not implemented* — split into the corpora with no handler at all and the
// individual files a handler has not been written for.
//
// The floor those three share is zero, which is the one regression this design cannot see: delete a handler
// and its cases move from passed into not-implemented, nothing goes red, and the runner exits 0.
// `--min-passed` is the ratchet that closes it.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text;

namespace Skynet.Os;

/// <summary>
/// One vector: an input, the bytes the reference produced from it, and the note that says what it is for.
/// <c>Expected</c> is a string and not a tree, because the requirement is byte-identical output and the bytes
/// are what the file holds.
/// </summary>
internal sealed record CorpusCase(string Id, string Note, Value? Input, string Expected);

/// <summary>One corpus file, and the <c>corpus/name</c> key a dispatch is registered under.</summary>
internal sealed record CorpusFile(string Path, string Corpus, string Name, List<CorpusCase> Cases)
{
    internal string Key => Corpus + "/" + Name;
}

internal sealed record Failure(
    string File,
    string Id,
    string Note,
    string? Expected,
    string? Actual,
    string? Error);

internal sealed record Report(
    int Passed,
    int Failed,
    int Unanswered,
    List<string> Complete,
    List<string> Partial,
    List<string> MissingHandlers,
    List<string> DeclinedFiles,
    List<Failure> Failures)
{
    internal int Total => Passed + Failed + Unanswered;
}

internal static class Corpus
{
    /// <summary>
    /// A contract, as the runner sees it. Returns the value the contract produced; the runner encodes it and
    /// compares bytes. A thrown exception is a case this edition could not answer <b>at all</b> — not one it
    /// answered wrongly. The two are counted separately and neither may hide in the other.
    /// </summary>
    internal delegate Value Handler(Value? input);

    /// <summary>
    /// The corpus directory: an explicit path, else <c>$SKYNET_CORPUS</c>, else the nearest <c>corpus/</c> at or
    /// above the working directory. Null when there is none, which the caller reports rather than defaulting to
    /// something plausible — a runner that measured an empty corpus would exit 0 having proved nothing.
    /// </summary>
    internal static string? Root(string? explicitPath)
    {
        if (!string.IsNullOrEmpty(explicitPath))
        {
            return Directory.Exists(explicitPath) ? explicitPath : null;
        }
        string? fromEnvironment = Environment.GetEnvironmentVariable("SKYNET_CORPUS");
        if (!string.IsNullOrEmpty(fromEnvironment))
        {
            return Directory.Exists(fromEnvironment) ? fromEnvironment : null;
        }
        // Walk up looking for a `corpus/README.md`, not merely a directory called `corpus`: a build tree with an
        // empty `corpus/` in it would otherwise be found first and measured as zero cases.
        string? here = Directory.GetCurrentDirectory();
        for (int levels = 0; levels < 64 && here is not null; levels++)
        {
            string candidate = Path.Combine(here, "corpus");
            if (File.Exists(Path.Combine(candidate, "README.md")))
            {
                return candidate;
            }
            here = Path.GetDirectoryName(here);
        }
        return null;
    }

    /// <summary>Every JSON file one directory below <c>corpusRoot</c>, sorted, so a run is reproducible.</summary>
    internal static List<CorpusFile> Load(string corpusRoot)
    {
        List<CorpusFile> files = new();
        List<string> directories = new(Directory.GetDirectories(corpusRoot));
        directories.RemoveAll(path => Path.GetFileName(path).StartsWith('.'));
        // Ordinal, not culture-aware: the order a run reports in must not depend on the machine, and
        // `List.Sort` on strings is `Comparer<string>.Default`, which is culture-aware.
        directories.Sort(StringComparer.Ordinal);
        foreach (string directory in directories)
        {
            List<string> names = new(Directory.GetFiles(directory, "*.json"));
            names.Sort(StringComparer.Ordinal);
            foreach (string path in names)
            {
                files.Add(Read(Path.GetFileName(directory), path));
            }
        }
        return files;
    }

    private static CorpusFile Read(string corpusName, string path)
    {
        // `ReadAllText` with an explicit UTF-8: the default for a file with no byte-order mark is UTF-8 anyway,
        // and saying so is cheaper than a reader wondering.
        string text = File.ReadAllText(path, Encoding.UTF8);
        Value document = Parse.Document(text);
        Value? cases = Value.Get(document, "cases");
        if (cases is null || cases.ValueKind != Kind.Array)
        {
            throw new InvalidDataException(path + " has no `cases` array");
        }
        List<CorpusCase> held = new();
        for (int at = 0; at < Value.Length(cases); at++)
        {
            Value? vector = Value.At(cases, at);
            Value? expected = Value.Get(vector, "expected");
            if (expected is null || expected.ValueKind != Kind.String)
            {
                throw new InvalidDataException(path + " has a case with no `expected` string."
                    + " corpus/README.md's first property is that the bytes are what the file holds.");
            }
            held.Add(new CorpusCase(
                Value.AsString(Value.Get(vector, "id"), string.Empty),
                Value.AsString(Value.Get(vector, "note"), string.Empty),
                Value.Get(vector, "input"),
                Value.AsString(expected, string.Empty)));
        }
        string name = Path.GetFileNameWithoutExtension(path);
        return new CorpusFile(path, corpusName, name, held);
    }

    private sealed class Tally
    {
        internal int Files;
        internal int Handled;
        internal bool Failed;
    }

    /// <summary>Measure <c>dispatch</c> against <c>files</c>.</summary>
    internal static Report Run(List<CorpusFile> files, Dictionary<string, Handler> dispatch)
    {
        int passed = 0;
        int failed = 0;
        int unanswered = 0;
        List<Failure> failures = new();
        List<string> declined = new();
        // Insertion-ordered by construction: a `Dictionary` does not promise an order, so the corpus names are
        // kept in a list beside it and sorted before they are printed.
        Dictionary<string, Tally> tallies = new(StringComparer.Ordinal);
        List<string> seen = new();

        foreach (CorpusFile file in files)
        {
            if (!tallies.TryGetValue(file.Corpus, out Tally? tally))
            {
                tally = new Tally();
                tallies.Add(file.Corpus, tally);
                seen.Add(file.Corpus);
            }
            tally.Files += 1;

            if (!dispatch.TryGetValue(file.Key, out Handler? handler))
            {
                unanswered += file.Cases.Count;
                declined.Add(file.Key);
                continue;
            }
            tally.Handled += 1;

            foreach (CorpusCase vector in file.Cases)
            {
                string encoded;
                try
                {
                    encoded = Canonical.Encode(handler(vector.Input));
                }
#pragma warning disable CA1031 // Do not catch general exception types
                catch (Exception error) when (error is not StackOverflowException)
                {
                    // A handler that throws is caught and recorded as a failure of its case rather than taking
                    // the runner down. This is not leniency about exceptions — every one of them is a bug in
                    // this edition and is reported as a red case — it is finding 164: a runner that crashes
                    // describing a failure hides the failure. The catch is deliberately broad for the same
                    // reason, and the analyser is told so here rather than in a project-wide setting.
                    failed += 1;
                    failures.Add(new Failure(file.Key, vector.Id, vector.Note, null, null,
                        error.GetType().Name + ": " + error.Message));
                    tally.Failed = true;
                    continue;
                }
#pragma warning restore CA1031
                if (!string.Equals(encoded, vector.Expected, StringComparison.Ordinal))
                {
                    failed += 1;
                    failures.Add(new Failure(file.Key, vector.Id, vector.Note, vector.Expected, encoded, null));
                    tally.Failed = true;
                    continue;
                }
                passed += 1;
            }
        }

        List<string> complete = new();
        List<string> partial = new();
        List<string> missing = new();
        foreach (string corpusName in seen)
        {
            Tally tally = tallies[corpusName];
            if (tally.Handled == 0)
            {
                missing.Add(corpusName);
            }
            else if (tally.Handled < tally.Files || tally.Failed)
            {
                partial.Add(corpusName);
            }
            else
            {
                complete.Add(corpusName);
            }
        }

        // A corpus with no handler at all is named once in `MissingHandlers`; `DeclinedFiles` is for the files of
        // corpora that are partly answered, which is the bucket a reader needs in order to see that a corpus is
        // not finished.
        List<string> kept = new();
        foreach (string key in declined)
        {
            string corpusName = key.Substring(0, key.IndexOf('/', StringComparison.Ordinal));
            if (!missing.Contains(corpusName))
            {
                kept.Add(key);
            }
        }

        complete.Sort(StringComparer.Ordinal);
        partial.Sort(StringComparer.Ordinal);
        missing.Sort(StringComparer.Ordinal);
        kept.Sort(StringComparer.Ordinal);
        return new Report(passed, failed, unanswered, complete, partial, missing, kept, failures);
    }

    private static string Window(string text, int at) =>
        text.Substring(Math.Max(0, at - 24), Math.Min(text.Length, at + 24) - Math.Max(0, at - 24));

    /// <summary>
    /// <c>file :: case</c>, the note, and where the two documents part company — a diff of two four-kilobyte
    /// documents is not readable without that last line.
    /// </summary>
    internal static string Describe(Failure failure)
    {
        StringBuilder out_ = new();
        out_.Append(failure.File).Append(" :: ").Append(failure.Id).Append("\n  ").Append(failure.Note);
        if (failure.Error is not null)
        {
            return out_.Append("\n  error: ").Append(failure.Error).ToString();
        }
        string expected = failure.Expected ?? string.Empty;
        string actual = failure.Actual ?? string.Empty;
        out_.Append("\n  expected ").Append(expected).Append("\n  actual   ").Append(actual).Append("\n  ");
        int limit = Math.Min(expected.Length, actual.Length);
        for (int at = 0; at < limit; at++)
        {
            if (expected[at] != actual[at])
            {
                return out_.Append("first difference at byte ")
                    .Append(at.ToString(CultureInfo.InvariantCulture))
                    .Append(": expected \"").Append(Window(expected, at)).Append("\", got \"")
                    .Append(Window(actual, at)).Append('"').ToString();
            }
        }
        if (expected.Length == actual.Length)
        {
            return out_.Append("identical").ToString();
        }
        return out_.Append("identical for ").Append(limit.ToString(CultureInfo.InvariantCulture))
            .Append(" bytes, then one is longer").ToString();
    }
}
