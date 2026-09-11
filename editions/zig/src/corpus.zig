//! Reading and running the conformance corpus.
//!
//! Split from the contracts because two programs read the corpus and only one of them computes
//! anything: `roundtrip` re-serialises every `expected` string before a single contract exists, and
//! `corpus` runs the contracts against it. One reader means the two cannot disagree about which files
//! the corpus has.
//!
//! # The four buckets, and why none of them is useful alone
//!
//! §31.4: an edition that has not passed the corpus is not an edition. A runner that printed only
//! failures would report an edition with one handler as flawless, so a report here says how many cases
//! *passed*, how many *failed*, and how many are *not implemented* — split into the corpora with no
//! handler at all and the individual files a handler has not been written for.
//!
//! The floor those three share is zero, which is the one regression this design cannot see: delete a
//! handler and its cases move from passed into not-implemented, nothing goes red, and the runner exits
//! 0. `--min-passed` is the ratchet that closes it.

const std = @import("std");
const Allocator = std.mem.Allocator;
const Io = std.Io;
const value = @import("value.zig");
const parse = @import("parse.zig");
const Value = value.Value;

/// One vector: an input, the bytes the reference produced from it, and the note that says what it is
/// for. `expected` is a string and not a tree, because the requirement is byte-identical output and the
/// bytes are what the file holds.
pub const Case = struct {
    id: []const u8,
    note: []const u8,
    input: ?*const Value,
    expected: []const u8,
};

/// One corpus file, and the `corpus/name` key a dispatch is registered under.
pub const File = struct {
    path: []const u8,
    corpus: []const u8,
    name: []const u8,
    key: []const u8,
    cases: []const Case,
};

/// A contract, as the runner sees it.
///
/// Returns the value the contract produced; the runner encodes it and compares bytes. An error is a
/// case this edition could not answer *at all* — not one it answered wrongly. The two are counted
/// separately and neither may hide in the other.
///
/// The allocator belongs to the case and is released after it, so a handler allocates freely and frees
/// nothing.
pub const Handler = *const fn (Allocator, ?*const Value) anyerror!*const Value;

pub const DispatchEntry = struct {
    key: []const u8,
    handler: Handler,
};

pub const Failure = struct {
    file: []const u8,
    id: []const u8,
    note: []const u8,
    expected: ?[]const u8 = null,
    actual: ?[]const u8 = null,
    failed_with: ?[]const u8 = null,
};

pub const Report = struct {
    passed: u32 = 0,
    failed: u32 = 0,
    unanswered: u32 = 0,
    complete: []const []const u8 = &.{},
    partial: []const []const u8 = &.{},
    missing_handlers: []const []const u8 = &.{},
    declined_files: []const []const u8 = &.{},
    failures: []const Failure = &.{},

    pub fn total(self: Report) u32 {
        return self.passed + self.failed + self.unanswered;
    }
};

/// The corpus directory: an explicit path, else `$SKYNET_CORPUS`, else the nearest `corpus/` at or above
/// the working directory. Null when there is none, which the caller reports rather than defaulting to
/// something plausible — a runner that measured an empty corpus would exit 0 having proved nothing.
pub fn root(
    a: Allocator,
    io: Io,
    /// The process's environment, which Zig 0.16 hands to `main` rather than exposing globally. Passed in
    /// rather than read here, because §3.9's rule is that one place reads the environment.
    environ: *std.process.Environ.Map,
    explicit: ?[]const u8,
) !?[]const u8 {
    if (explicit) |given| {
        if (given.len != 0) return if (isDir(io, given)) given else null;
    }
    if (environ.get("SKYNET_CORPUS")) |from_environment| {
        if (from_environment.len != 0) return if (isDir(io, from_environment)) from_environment else null;
    }

    // Walk up looking for a `corpus/README.md`, not merely a directory called `corpus`: a build tree
    // with an empty `corpus/` in it would otherwise be found first and measured as zero cases.
    var cwd_buffer: [Io.Dir.max_path_bytes]u8 = undefined;
    const cwd_len = try std.process.currentPath(io, &cwd_buffer);
    var here: []const u8 = cwd_buffer[0..cwd_len];
    var levels: usize = 0;
    while (levels < 64) : (levels += 1) {
        const candidate = try std.fs.path.join(a, &.{ here, "corpus" });
        const readme = try std.fs.path.join(a, &.{ candidate, "README.md" });
        if (isFile(io, readme)) return candidate;
        const parent = std.fs.path.dirname(here) orelse return null;
        if (std.mem.eql(u8, parent, here)) return null;
        here = parent;
    }
    return null;
}

fn isDir(io: Io, path: []const u8) bool {
    var dir = Io.Dir.cwd().openDir(io, path, .{}) catch return false;
    dir.close(io);
    return true;
}

fn isFile(io: Io, path: []const u8) bool {
    var file = Io.Dir.cwd().openFile(io, path, .{}) catch return false;
    file.close(io);
    return true;
}

fn compareStrings(_: void, left: []const u8, right: []const u8) bool {
    return std.mem.order(u8, left, right) == .lt;
}

/// Every JSON file one directory below `root`, sorted, so a run is reproducible.
pub fn load(a: Allocator, io: Io, corpus_root: []const u8) ![]const File {
    var dir = try Io.Dir.cwd().openDir(io, corpus_root, .{ .iterate = true });
    defer dir.close(io);

    var corpora: std.ArrayList([]const u8) = .empty;
    var it = dir.iterate();
    while (try it.next(io)) |entry| {
        if (entry.name[0] == '.') continue;
        if (entry.kind != .directory) continue;
        try corpora.append(a, try a.dupe(u8, entry.name));
    }
    std.mem.sort([]const u8, corpora.items, {}, compareStrings);

    var files: std.ArrayList(File) = .empty;
    for (corpora.items) |corpus_name| {
        const corpus_path = try std.fs.path.join(a, &.{ corpus_root, corpus_name });
        var corpus_dir = try Io.Dir.cwd().openDir(io, corpus_path, .{ .iterate = true });
        defer corpus_dir.close(io);

        var names: std.ArrayList([]const u8) = .empty;
        var inner = corpus_dir.iterate();
        while (try inner.next(io)) |entry| {
            if (entry.name[0] == '.') continue;
            if (!std.mem.endsWith(u8, entry.name, ".json")) continue;
            try names.append(a, try a.dupe(u8, entry.name));
        }
        std.mem.sort([]const u8, names.items, {}, compareStrings);

        for (names.items) |file_name| {
            const path = try std.fs.path.join(a, &.{ corpus_path, file_name });
            const bytes = try Io.Dir.cwd().readFileAlloc(io, path, a, .limited(1 << 28));
            var diagnostic: parse.Diagnostic = .{};
            const document = parse.parse(a, bytes, &diagnostic) catch {
                std.debug.print("{s} is not JSON: {s}\n", .{ path, try diagnostic.render(a) });
                return error.CorpusUnreadable;
            };
            const cases = value.get(document, "cases") orelse return error.CorpusUnreadable;
            if (cases.kind != .array) return error.CorpusUnreadable;

            var held: std.ArrayList(Case) = .empty;
            for (cases.items.items) |vector| {
                const expected = value.get(vector, "expected") orelse return error.CorpusUnreadable;
                if (expected.kind != .string) {
                    std.debug.print(
                        "{s} has a case with no `expected` string. corpus/README.md's first property is " ++
                            "that the bytes are what the file holds.\n",
                        .{path},
                    );
                    return error.CorpusUnreadable;
                }
                try held.append(a, .{
                    .id = value.asString(value.get(vector, "id"), ""),
                    .note = value.asString(value.get(vector, "note"), ""),
                    .input = value.get(vector, "input"),
                    .expected = expected.text,
                });
            }
            try files.append(a, .{
                .path = path,
                .corpus = corpus_name,
                .name = file_name[0 .. file_name.len - ".json".len],
                .key = try std.fmt.allocPrint(a, "{s}/{s}", .{ corpus_name, file_name[0 .. file_name.len - ".json".len] }),
                .cases = held.items,
            });
        }
    }
    return files.items;
}

fn lookup(dispatch: []const DispatchEntry, key: []const u8) ?Handler {
    for (dispatch) |entry| {
        if (std.mem.eql(u8, entry.key, key)) return entry.handler;
    }
    return null;
}

const Tally = struct {
    name: []const u8,
    files: usize = 0,
    handled: usize = 0,
    failed: bool = false,
};

/// Measure `dispatch` against `files`.
pub fn run(a: Allocator, files: []const File, dispatch: []const DispatchEntry) !Report {
    var report: Report = .{};
    var failures: std.ArrayList(Failure) = .empty;
    var declined: std.ArrayList([]const u8) = .empty;
    var tallies: std.ArrayList(Tally) = .empty;

    for (files) |file| {
        const tally = blk: {
            for (tallies.items) |*held| {
                if (std.mem.eql(u8, held.name, file.corpus)) break :blk held;
            }
            try tallies.append(a, .{ .name = file.corpus });
            break :blk &tallies.items[tallies.items.len - 1];
        };
        tally.files += 1;

        const handler = lookup(dispatch, file.key) orelse {
            report.unanswered += @intCast(file.cases.len);
            try declined.append(a, file.key);
            continue;
        };
        tally.handled += 1;

        for (file.cases) |vector| {
            // One arena per case, released after it. This is what makes "no contract frees anything"
            // affordable: a run of 1,199 cases holds one case's working set rather than all of them.
            var scratch = std.heap.ArenaAllocator.init(std.heap.page_allocator);
            defer scratch.deinit();
            const scratch_allocator = scratch.allocator();

            const produced = handler(scratch_allocator, vector.input) catch |err| {
                report.failed += 1;
                try failures.append(a, .{
                    .file = file.key,
                    .id = vector.id,
                    .note = vector.note,
                    .failed_with = @errorName(err),
                });
                tally.failed = true;
                continue;
            };
            const encoded = value.encode(scratch_allocator, produced) catch |err| {
                report.failed += 1;
                try failures.append(a, .{
                    .file = file.key,
                    .id = vector.id,
                    .note = vector.note,
                    .failed_with = @errorName(err),
                });
                tally.failed = true;
                continue;
            };
            if (!std.mem.eql(u8, encoded, vector.expected)) {
                report.failed += 1;
                try failures.append(a, .{
                    .file = file.key,
                    .id = vector.id,
                    .note = vector.note,
                    .expected = vector.expected,
                    // Copied into the run's allocator before the case's is released.
                    .actual = try a.dupe(u8, encoded),
                });
                tally.failed = true;
                continue;
            }
            report.passed += 1;
        }
    }

    var complete: std.ArrayList([]const u8) = .empty;
    var partial: std.ArrayList([]const u8) = .empty;
    var missing: std.ArrayList([]const u8) = .empty;
    for (tallies.items) |tally| {
        if (tally.handled == 0) {
            try missing.append(a, tally.name);
        } else if (tally.handled < tally.files or tally.failed) {
            try partial.append(a, tally.name);
        } else {
            try complete.append(a, tally.name);
        }
    }

    // A corpus with no handler at all is named once in `missing_handlers`; `declined_files` is for the
    // files of corpora that are partly answered, which is the bucket a reader needs in order to see
    // that a corpus is not finished.
    var kept: std.ArrayList([]const u8) = .empty;
    for (declined.items) |key| {
        const slash = std.mem.indexOfScalar(u8, key, '/') orelse key.len;
        const corpus_name = key[0..slash];
        var whole_corpus_missing = false;
        for (missing.items) |name| {
            if (std.mem.eql(u8, name, corpus_name)) {
                whole_corpus_missing = true;
                break;
            }
        }
        if (!whole_corpus_missing) try kept.append(a, key);
    }

    std.mem.sort([]const u8, complete.items, {}, compareStrings);
    std.mem.sort([]const u8, partial.items, {}, compareStrings);
    std.mem.sort([]const u8, missing.items, {}, compareStrings);
    std.mem.sort([]const u8, kept.items, {}, compareStrings);

    report.complete = complete.items;
    report.partial = partial.items;
    report.missing_handlers = missing.items;
    report.declined_files = kept.items;
    report.failures = failures.items;
    return report;
}

fn window(text: []const u8, at: usize) []const u8 {
    const start = if (at > 24) at - 24 else 0;
    const end = @min(at + 24, text.len);
    return text[start..end];
}

/// `file :: case`, the note, and where the two documents part company — a diff of two four-kilobyte
/// documents is not readable without that last line.
pub fn describe(a: Allocator, failure: Failure) ![]const u8 {
    if (failure.failed_with) |err| {
        return std.fmt.allocPrint(a, "{s} :: {s}\n  {s}\n  error: {s}", .{
            failure.file, failure.id, failure.note, err,
        });
    }
    const expected = failure.expected orelse "";
    const actual = failure.actual orelse "";
    const limit = @min(expected.len, actual.len);
    var where: []const u8 = undefined;
    var found = false;
    for (0..limit) |at| {
        if (expected[at] != actual[at]) {
            where = try std.fmt.allocPrint(a, "first difference at byte {d}: expected \"{s}\", got \"{s}\"", .{
                at, window(expected, at), window(actual, at),
            });
            found = true;
            break;
        }
    }
    if (!found) {
        where = if (expected.len == actual.len)
            "identical"
        else
            try std.fmt.allocPrint(a, "identical for {d} bytes, then one is longer", .{limit});
    }
    return std.fmt.allocPrint(a, "{s} :: {s}\n  {s}\n  expected {s}\n  actual   {s}\n  {s}", .{
        failure.file, failure.id, failure.note, expected, actual, where,
    });
}
