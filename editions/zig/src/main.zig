//! Run the conformance corpus against the Zig edition.
//!
//!     skynet-corpus-zig [--corpus-root corpus] [--verbose] [--json] [--min-passed N]
//!
//! Exit code 0 only when every case in every corpus **that has a handler** passed, and the summary always
//! says how many cases are not implemented — split into the corpora with no handler at all and the
//! individual files a handler has not been written for. Those facts together are the honest sentence
//! about this edition, and none of them is useful without the others.
//!
//! `--min-passed` is the ratchet, and it exists because everything above is a **floor of zero**. An
//! edition whose handler was deleted answers nothing, fails nothing, and exits 0: the cases move from
//! *passed* into *not implemented*, which is the bucket that reads as honest progress. That is the one
//! regression this runner's own design cannot see, so the number a build passes here is the number this
//! edition has reached, and lowering it is a decision somebody makes in a diff.

const std = @import("std");
const corpus = @import("corpus.zig");
const value = @import("value.zig");
const dispatch = @import("contracts/dispatch.zig");

fn usage() u8 {
    std.debug.print("usage: skynet-corpus-zig [--corpus-root DIR] [--verbose] [--json] [--min-passed N]\n", .{});
    return 2;
}

fn printList(heading: []const u8, values: []const []const u8) void {
    if (values.len == 0) return;
    std.debug.print("{s} ", .{heading});
    for (values, 0..) |held, index| {
        std.debug.print("{s}{s}", .{ if (index == 0) "" else ", ", held });
    }
    std.debug.print("\n", .{});
}

fn strings(a: std.mem.Allocator, values: []const []const u8) !*const value.Value {
    const out = try value.array(a);
    for (values) |held| try value.push(a, out, try value.string(a, held));
    return out;
}

/// Zig 0.16 hands `main` the process arena, an `Io` and the argument vector, so none of the three
/// programs here builds its own. Taking them rather than constructing them is also what makes the
/// `Io` the one the standard library selected for this target instead of one this edition chose.
pub fn main(init: std.process.Init) !u8 {
    const a = init.arena.allocator();
    const io = init.io;

    var root_argument: ?[]const u8 = null;
    var verbose = false;
    var as_json = false;
    var min_passed: i64 = -1;

    var arguments = init.minimal.args.iterate();
    _ = arguments.skip();
    while (arguments.next()) |argument| {
        if (std.mem.eql(u8, argument, "--corpus-root")) {
            root_argument = arguments.next() orelse return usage();
        } else if (std.mem.eql(u8, argument, "--verbose")) {
            verbose = true;
        } else if (std.mem.eql(u8, argument, "--json")) {
            as_json = true;
        } else if (std.mem.eql(u8, argument, "--min-passed")) {
            min_passed = try std.fmt.parseInt(i64, arguments.next() orelse return usage(), 10);
        } else {
            return usage();
        }
    }

    const corpus_root = (try corpus.root(a, io, init.environ_map, root_argument)) orelse {
        std.debug.print("no corpus found: pass --corpus-root, or set SKYNET_CORPUS\n", .{});
        return 2;
    };
    const files = try corpus.load(a, io, corpus_root);
    if (files.len == 0) {
        std.debug.print("no corpus files under {s}\n", .{corpus_root});
        return 2;
    }

    const report = try corpus.run(a, files, &dispatch.entries);

    if (as_json) {
        // Deliberately the same fields the human report prints, and no derived ones: a summary that
        // computed a percentage here and a page that computed it differently would be two answers to one
        // question.
        const out = try value.object(a);
        try value.set(a, out, "passed", try value.number(a, @floatFromInt(report.passed)));
        try value.set(a, out, "failed", try value.number(a, @floatFromInt(report.failed)));
        try value.set(a, out, "unimplemented", try value.number(a, @floatFromInt(report.unanswered)));
        try value.set(a, out, "total", try value.number(a, @floatFromInt(report.total())));
        try value.set(a, out, "complete", try strings(a, report.complete));
        try value.set(a, out, "partial", try strings(a, report.partial));
        try value.set(a, out, "missing_handlers", try strings(a, report.missing_handlers));
        try value.set(a, out, "declined_files", try strings(a, report.declined_files));
        const encoded = try value.encode(a, out);
        // stdout, because this is the output a tool parses; everything else here goes to stderr.
        var buffer: [64]u8 = undefined;
        var stdout = std.Io.File.stdout().writer(io, &buffer);
        try stdout.interface.print("{s}\n", .{encoded});
        try stdout.interface.flush();
        return if (report.failed == 0) 0 else 1;
    }

    std.debug.print("corpus     {s}\n", .{corpus_root});
    std.debug.print("result     {d} passed, {d} failed, {d} not implemented, of {d}\n", .{
        report.passed, report.failed, report.unanswered, report.total(),
    });
    printList("complete  ", report.complete);
    printList("no handler", report.missing_handlers);
    printList("declined  ", report.declined_files);
    // Named separately from `complete` on purpose: these are corpora this edition answers part of, and a
    // reader who saw only the passing files would take them for finished ones.
    printList("partial   ", report.partial);

    if (report.failures.len > 0) {
        std.debug.print("\n", .{});
        const shown = if (verbose) report.failures.len else @min(report.failures.len, 10);
        for (report.failures[0..shown]) |failure| {
            std.debug.print("{s}\n\n", .{try corpus.describe(a, failure)});
        }
        if (shown < report.failures.len) {
            std.debug.print("... and {d} more (pass --verbose)\n", .{report.failures.len - shown});
        }
    }

    if (min_passed >= 0 and report.passed < min_passed) {
        std.debug.print(
            \\
            \\regression: {d} passed, and this edition had reached {d}. Cases that stop being answered
            \\are counted as *not implemented* rather than as failures, so nothing above this line went
            \\red — which is why the floor is checked separately from the failures.
            \\
        , .{ report.passed, min_passed });
        return 1;
    }
    return if (report.failed == 0) 0 else 1;
}
