// The table that routes a corpus file to a contract.
//
// Every contract in this directory is written from the modules under `reference/typescript` and from nothing
// else — never from `corpus/`, which would agree by construction, and never from the Python, C++, Go, C, Zig
// or Java editions, which would prove only that one hand did both. What holds them honest is the corpus, in
// the one direction that means something: this edition reproducing what the oracle produced, byte for byte.
//
// One class here per module there, named for it, so a reader holding the reference open can find the C#
// beside it. This file is the only one with no counterpart: the reference has no such table, because in
// TypeScript the corpus generator calls the functions directly.

using System.Collections.Generic;

namespace Skynet.Os.Contracts;

internal static class Dispatch
{
    /// <summary>
    /// Every corpus file this edition answers, keyed by <c>corpus/file</c>. A file absent from this table is
    /// counted as <i>not implemented</i> rather than as passing, and the runner prints the two separately —
    /// §31.4's "an edition that has not passed the corpus is not an edition" is not satisfied by a table with
    /// fewer entries in it.
    /// </summary>
    internal static Dictionary<string, Corpus.Handler> Entries() => new(System.StringComparer.Ordinal)
    {
        ["predicate/evaluate"] = Predicate.EvaluateCase,
        ["roles/vocabulary"] = Embodiment.RolesVocabularyCase,
        ["semver/compare"] = Semver.CompareCase,
        ["semver/satisfies"] = Semver.SatisfiesCase,
        ["vocabulary/closed-sets"] = Vocabulary.ClosedSetsCase,
    };
}
