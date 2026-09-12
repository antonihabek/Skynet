//! The canonical writer, proven before anything is built on it.
//!
//!     skynet-roundtrip-zig [--corpus-root corpus]
//!
//! Every `expected` string in the corpus is parsed and re-serialised, and the bytes must be identical.
//! That is a stronger exam than it looks, and it is why this program runs before the corpus runner:
//!
//! * **Rule 3 over every number the reference ever wrote.** This edition's number formatting is
//!   `std.fmt.float.render` in `.decimal` mode plus one departure for negative zero — a *library call*,
//!   which means it changes when the library does. This step is what holds it.
//! * **Rule 2 over keys nobody chose.** The corpus holds documents whose keys collide on their first
//!   bytes and differ in length, which is the comparison a shared-prefix compare gets wrong.
//! * **Rule 4 over strings the generator escaped.** The corpus is written by `JSON.stringify`, which
//!   escapes non-ASCII as `\uXXXX`; this edition emits UTF-8. A file that round-trips has had both
//!   halves — the parser's escape decoding and the writer's pass-through — agree.
//!
//! A round trip that fails is a defect in this edition and never in the corpus, because the corpus is
//! the oracle's output and canonical by construction.

const std = @import("std");
const corpus = @import("corpus.zig");
const parse = @import("parse.zig");
const value = @import("value.zig");

pub fn main(init: std.process.Init) !u8 {
    const a = init.arena.allocator();
    const io = init.io;

    var root_argument: ?[]const u8 = null;
    var arguments = init.minimal.args.iterate();
    _ = arguments.skip();
    while (arguments.next()) |argument| {
        if (std.mem.eql(u8, argument, "--corpus-root")) {
            root_argument = arguments.next() orelse {
                std.debug.print("usage: skynet-roundtrip-zig [--corpus-root DIR]\n", .{});
                return 2;
            };
        } else {
            std.debug.print("usage: skynet-roundtrip-zig [--corpus-root DIR]\n", .{});
            return 2;
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

    var checked: u32 = 0;
    var failed: u32 = 0;
    for (files) |file| {
        for (file.cases) |vector| {
            var scratch = std.heap.ArenaAllocator.init(std.heap.page_allocator);
            defer scratch.deinit();
            const scratch_allocator = scratch.allocator();

            var diagnostic: parse.Diagnostic = .{};
            const parsed = parse.parse(scratch_allocator, vector.expected, &diagnostic) catch {
                failed += 1;
                std.debug.print("{s} :: {s}\n  the expectation is not JSON: {s}\n", .{
                    file.key, vector.id, try diagnostic.render(scratch_allocator),
                });
                continue;
            };
            const encoded = value.encode(scratch_allocator, parsed) catch |err| {
                failed += 1;
                std.debug.print("{s} :: {s}\n  the expectation cannot be re-serialised: {s}\n", .{
                    file.key, vector.id, @errorName(err),
                });
                continue;
            };
            if (!std.mem.eql(u8, encoded, vector.expected)) {
                failed += 1;
                std.debug.print("{s}\n\n", .{try corpus.describe(a, .{
                    .file = file.key,
                    .id = vector.id,
                    .note = "a canonical document did not survive a parse and a write",
                    .expected = vector.expected,
                    .actual = try a.dupe(u8, encoded),
                })});
                continue;
            }
            checked += 1;
        }
    }

    std.debug.print("round trip  {d} of {d} expectations re-serialised identically\n", .{ checked, checked + failed });
    return if (failed == 0) 0 else 1;
}
