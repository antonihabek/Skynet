//! The table that routes a corpus file to a contract.
//!
//! Every contract in this directory is written from the modules under `reference/typescript` and from
//! nothing else — never from `corpus/`, which would agree by construction, and never from the Python,
//! C++, Go or C editions, which would prove only that one hand did both. What holds them honest is the
//! corpus, in the one direction that means something: this edition reproducing what the oracle
//! produced, byte for byte.
//!
//! One file here per module there, named the same, so a reader holding the reference open can find the
//! Zig beside it. This file is the only one with no counterpart: the reference has no such table,
//! because in TypeScript the corpus generator calls the functions directly.

const corpus = @import("../corpus.zig");
const embodiment = @import("embodiment.zig");
const predicate = @import("predicate.zig");
const semver = @import("semver.zig");
const vocabulary = @import("vocabulary.zig");

/// Every corpus file this edition answers, keyed by `corpus/file`.
///
/// A file absent from this table is counted as *not implemented* rather than as passing, and the runner
/// prints the two separately — §31.4's "an edition that has not passed the corpus is not an edition" is
/// not satisfied by a table with fewer entries in it.
pub const entries = [_]corpus.DispatchEntry{
    .{ .key = "predicate/evaluate", .handler = predicate.evaluateCase },
    .{ .key = "roles/vocabulary", .handler = embodiment.rolesVocabularyCase },
    .{ .key = "semver/compare", .handler = semver.compareCase },
    .{ .key = "semver/satisfies", .handler = semver.satisfiesCase },
    .{ .key = "vocabulary/closed-sets", .handler = vocabulary.closedSetsCase },
};
