//! The Zig edition checking itself, over the things the corpus cannot reach.
//!
//!     skynet-selfcheck-zig [--corpus-root corpus]
//!
//! Six groups, and each exists because a corpus run would be green without it:
//!
//! 1. **Rule 3 at its edges.** The corpus holds the numbers the reference happened to produce. It does
//!    not hold 2^53, the smallest subnormal, or negative zero — and negative zero is the one place this
//!    edition departs from `std.fmt.float.render`, so it is the one an upgrade of the standard library
//!    could silently take back.
//! 2. **Rules 2, 4 and 5 as facts rather than consequences.** A key that is a prefix of another, an
//!    absent field beside a null one, a string holding U+0000.
//! 3. **What the parser refuses.** A parser that accepted these would read documents the oracle cannot
//!    write, and no corpus file would say so.
//! 4. **The runner's own accounting**, over synthetic corpora with known answers. The four buckets are
//!    what every other number in this edition is reported through, and a runner that counted a declined
//!    file as a pass would report a finished edition.
//! 5. **The case tables**, against the whole strings the reference's engine converted. The tables are
//!    single code points and cannot be wrong about a string on their own; the code that walks a string
//!    with them, and applies the sigma rule, can.
//! 6. **The closed vocabularies' table**, over the two things the corpus checks nothing about: how many
//!    sets this edition believes it holds, and whether a name nobody declared is distinguishable from a
//!    set with no members.

const std = @import("std");
const Allocator = std.mem.Allocator;
const corpus = @import("corpus.zig");
const parse = @import("parse.zig");
const value = @import("value.zig");
const js = @import("js.zig");
const vectors = @import("case_vectors.zig");
const opstate = @import("contracts/opstate.zig");
const vocabulary = @import("contracts/vocabulary.zig");

var checks: u32 = 0;
var failures: u32 = 0;

fn expectText(what: []const u8, actual: []const u8, wanted: []const u8) void {
    checks += 1;
    if (!std.mem.eql(u8, actual, wanted)) {
        failures += 1;
        std.debug.print("  {s}: expected \"{s}\", got \"{s}\"\n", .{ what, wanted, actual });
    }
}

fn expectTrue(what: []const u8, truth: bool) void {
    checks += 1;
    if (!truth) {
        failures += 1;
        std.debug.print("  {s}: expected true\n", .{what});
    }
}

fn expectInt(what: []const u8, actual: u32, wanted: u32) void {
    checks += 1;
    if (actual != wanted) {
        failures += 1;
        std.debug.print("  {s}: expected {d}, got {d}\n", .{ what, wanted, actual });
    }
}

// ── 1. Rule 3 ────────────────────────────────────────────────────────────────

var sink: f64 = 0;

fn checkNumber(what: []const u8, number: f64, wanted: []const u8) void {
    var buf: [value.number_buffer_size]u8 = undefined;
    const rendered = value.formatNumber(&buf, number) catch {
        checks += 1;
        failures += 1;
        std.debug.print("  {s}: refused a finite number\n", .{what});
        return;
    };
    expectText(what, rendered, wanted);
}

fn numbers() void {
    std.debug.print("rule 3 — numbers\n", .{});
    // Every value goes through a runtime `f64` rather than a literal expression, because Zig evaluates
    // comptime float arithmetic in **f128**: `0.1 + 0.2` written as a literal is a different value from
    // the one a program computes, and a test written the obvious way would assert the wrong thing.
    sink = 0;
    checkNumber("zero", sink, "0");
    sink = -0.0;
    // The one departure from `std.fmt.float.render`, which writes `-0`.
    checkNumber("negative zero", sink, "0");
    sink = 1;
    checkNumber("one", sink, "1");
    sink = -1;
    checkNumber("minus one", sink, "-1");
    sink = 9007199254740990;
    checkNumber("2^53 - 2", sink, "9007199254740990");
    sink = 9007199254740992;
    checkNumber("2^53", sink, "9007199254740992");
    var tenth: f64 = 0.1;
    var fifth: f64 = 0.2;
    sink = tenth;
    checkNumber("a tenth", sink, "0.1");
    sink = tenth + fifth;
    checkNumber("a tenth and two", sink, "0.30000000000000004");
    sink = 1e-7;
    // The reference's own example of why `canonical.ts` exists rather than a `JSON.stringify` with a
    // replacer: the engine writes `1e-7`, and rule 3 requires a positional rendering.
    checkNumber("a ten-millionth", sink, "0.0000001");
    sink = 1e21;
    checkNumber("ten to the twenty-first", sink, "1000000000000000000000");
    var one: f64 = 1;
    var three: f64 = 3;
    sink = one / three;
    checkNumber("a third", sink, "0.3333333333333333");
    sink = 1234567890.1234567;
    checkNumber("a large mixed value", sink, "1234567890.1234567");
    _ = &tenth;
    _ = &fifth;
    _ = &one;
    _ = &three;

    var buf: [value.number_buffer_size]u8 = undefined;
    sink = std.math.nan(f64);
    checks += 1;
    if (value.formatNumber(&buf, sink)) |_| {
        failures += 1;
        std.debug.print("  NaN: was formatted rather than refused. A document that wrote a non-finite " ++
            "value as `null` would claim a measurement was absent when it was not.\n", .{});
    } else |_| {}
    sink = std.math.inf(f64);
    checks += 1;
    if (value.formatNumber(&buf, sink)) |_| {
        failures += 1;
        std.debug.print("  infinity: was formatted rather than refused\n", .{});
    } else |_| {}
}

// ── 2. Rules 2, 4 and 5 ──────────────────────────────────────────────────────

fn encoded(a: Allocator, held: ?*const value.Value) []const u8 {
    return value.encode(a, held) catch "<unencodable>";
}

fn writer(a: Allocator) !void {
    std.debug.print("rules 2, 4 and 5 — keys, strings and absence\n", .{});

    // Rule 2 over a key that is a prefix of another: `a` sorts before `ab`, which a comparison over the
    // shared bytes alone cannot decide.
    const prefixes = try value.object(a);
    try value.set(a, prefixes, "ab", try value.number(a, 2));
    try value.set(a, prefixes, "a", try value.number(a, 1));
    try value.set(a, prefixes, "abc", try value.number(a, 3));
    expectText("a key that is a prefix of another", encoded(a, prefixes), "{\"a\":1,\"ab\":2,\"abc\":3}");

    // Ascending by UTF-8 bytes, which is not case-insensitive: `Z` (0x5A) sorts before `a` (0x61).
    const bytes = try value.object(a);
    try value.set(a, bytes, "a", try value.number(a, 1));
    try value.set(a, bytes, "Z", try value.number(a, 2));
    expectText("uppercase before lowercase", encoded(a, bytes), "{\"Z\":2,\"a\":1}");

    // Rule 5: an absent field is omitted and a null one is written, and they are different facts.
    const absence = try value.object(a);
    try value.set(a, absence, "declared", value.nul());
    try value.setOptional(a, absence, "undeclared", null);
    expectText("absent beside null", encoded(a, absence), "{\"declared\":null}");

    // Rule 4: the quote, the backslash and the C0 controls, and nothing else. A tab is `\t` and not
    // `\u0009`; a degree sign is its UTF-8 bytes and not an escape.
    const escapes = try value.object(a);
    try value.set(a, escapes, "quote", try value.string(a, "a\"b"));
    try value.set(a, escapes, "backslash", try value.string(a, "a\\b"));
    try value.set(a, escapes, "tab", try value.string(a, "a\tb"));
    try value.set(a, escapes, "unit separator", try value.string(a, "a\x1fb"));
    try value.set(a, escapes, "degree", try value.string(a, "20\u{00B0}"));
    expectText("rule 4", encoded(a, escapes), "{\"backslash\":\"a\\\\b\",\"degree\":\"20\u{00B0}\"," ++
        "\"quote\":\"a\\\"b\",\"tab\":\"a\\tb\",\"unit separator\":\"a\\u001fb\"}");

    // A string carrying U+0000 keeps its length. In Zig a slice always does, which is why this costs
    // nothing here and was a design decision in the C edition.
    const embedded = try value.object(a);
    try value.set(a, embedded, "nul", try value.string(a, "a\x00b"));
    expectText("a string holding U+0000", encoded(a, embedded), "{\"nul\":\"a\\u0000b\"}");

    // A non-finite number is where writing fails, and it fails as itself.
    const broken = try value.object(a);
    sink = std.math.nan(f64);
    try value.set(a, broken, "measured", try value.number(a, sink));
    checks += 1;
    if (value.encode(a, broken)) |_| {
        failures += 1;
        std.debug.print("  a document holding NaN was serialised\n", .{});
    } else |_| {}

    expectText("an empty object", encoded(a, try value.object(a)), "{}");
    expectText("an empty array", encoded(a, try value.array(a)), "[]");

    // Assigning a field twice keeps its first position, which is what the reference's object literal
    // does — and matters because the order a contract inserted in is what a failure prints.
    const reassigned = try value.object(a);
    try value.set(a, reassigned, "b", try value.number(a, 1));
    try value.set(a, reassigned, "a", try value.number(a, 2));
    try value.set(a, reassigned, "b", try value.number(a, 3));
    expectText("a field assigned twice", encoded(a, reassigned), "{\"a\":2,\"b\":3}");
}

// ── 3. What the parser refuses ───────────────────────────────────────────────

fn refuses(a: Allocator, what: []const u8, document: []const u8) void {
    var diagnostic: parse.Diagnostic = .{};
    checks += 1;
    if (parse.parse(a, document, &diagnostic)) |_| {
        failures += 1;
        std.debug.print("  {s}: `{s}` was accepted. A document the oracle cannot write is one this " ++
            "edition must not read.\n", .{ what, document });
    } else |_| {}
}

fn accepts(a: Allocator, what: []const u8, document: []const u8, canonical: []const u8) void {
    var diagnostic: parse.Diagnostic = .{};
    const parsed = parse.parse(a, document, &diagnostic) catch {
        checks += 1;
        failures += 1;
        std.debug.print("  {s}: `{s}` was refused\n", .{ what, document });
        return;
    };
    expectText(what, encoded(a, parsed), canonical);
}

fn parser(a: Allocator) void {
    std.debug.print("the parser\n", .{});
    refuses(a, "a trailing comma in an array", "[1,2,]");
    refuses(a, "a trailing comma in an object", "{\"a\":1,}");
    refuses(a, "NaN", "{\"a\":NaN}");
    refuses(a, "infinity", "{\"a\":Infinity}");
    refuses(a, "a leading zero", "{\"a\":01}");
    refuses(a, "a leading plus", "{\"a\":+1}");
    refuses(a, "a bare word", "{\"a\":undefined}");
    refuses(a, "a single-quoted string", "{'a':1}");
    refuses(a, "an unterminated object", "{\"a\":1");
    refuses(a, "two documents", "{} {}");
    refuses(a, "an unescaped newline in a string", "{\"a\":\"one\ntwo\"}");
    refuses(a, "a comment", "{\"a\":1} // a comment");

    // The corpus is written by `JSON.stringify`, so what the parser must accept is that shape:
    // whitespace, unsorted keys, and an escape for every character above the BMP.
    accepts(a, "whitespace and unsorted keys", "{ \"b\" : 1 ,\n  \"a\" : 2 }", "{\"a\":2,\"b\":1}");
    accepts(a, "a surrogate pair", "[\"\\ud834\\udd1e\"]", "[\"\u{1D11E}\"]");
    accepts(a, "an escaped solidus", "[\"a\\/b\"]", "[\"a/b\"]");
    accepts(a, "an escaped BMP character", "[\"\\u00b0\"]", "[\"\u{00B0}\"]");
    accepts(a, "an exponent", "[1e3]", "[1000]");
    accepts(a, "a negative exponent", "[1e-7]", "[0.0000001]");
    accepts(a, "nesting", "[[[[1]]]]", "[[[[1]]]]");
}

// ── 4. The runner's accounting ───────────────────────────────────────────────

fn answersOne(a: Allocator, input: ?*const value.Value) anyerror!*const value.Value {
    return value.number(a, value.asNumber(input, 0) + 1);
}

fn alwaysWrong(a: Allocator, input: ?*const value.Value) anyerror!*const value.Value {
    _ = input;
    return value.number(a, -1);
}

fn declines(a: Allocator, input: ?*const value.Value) anyerror!*const value.Value {
    _ = a;
    _ = input;
    return error.ThisCaseIsNotAnswered;
}

/// One file of `count` cases whose input is `n` and whose expectation is `n + 1`.
fn synthetic(a: Allocator, corpus_name: []const u8, name: []const u8, count: usize) !corpus.File {
    var cases: std.ArrayList(corpus.Case) = .empty;
    for (0..count) |at| {
        try cases.append(a, .{
            .id = "case",
            .note = "a synthetic case",
            .input = try value.number(a, @floatFromInt(at)),
            .expected = try std.fmt.allocPrint(a, "{d}", .{at + 1}),
        });
    }
    return .{
        .path = name,
        .corpus = corpus_name,
        .name = name,
        .key = try std.fmt.allocPrint(a, "{s}/{s}", .{ corpus_name, name }),
        .cases = cases.items,
    };
}

fn accounting(a: Allocator) !void {
    std.debug.print("the runner's accounting\n", .{});

    const files = [_]corpus.File{
        try synthetic(a, "answered", "whole", 3),
        try synthetic(a, "partly", "handled", 2),
        try synthetic(a, "partly", "declined", 4),
        try synthetic(a, "absent", "nothing", 5),
    };

    const dispatch = [_]corpus.DispatchEntry{
        .{ .key = "answered/whole", .handler = answersOne },
        .{ .key = "partly/handled", .handler = answersOne },
    };
    const report = try corpus.run(a, &files, &dispatch);

    expectInt("passed", report.passed, 5);
    expectInt("failed", report.failed, 0);
    // The four declined cases and the five with no handler at all are both *not implemented*, and a
    // runner that let either read as a pass would report a finished edition.
    expectInt("not implemented", report.unanswered, 9);
    expectInt("total", report.total(), 14);
    expectInt("complete corpora", @intCast(report.complete.len), 1);
    expectTrue("`answered` is complete", report.complete.len == 1 and
        std.mem.eql(u8, report.complete[0], "answered"));
    expectInt("partial corpora", @intCast(report.partial.len), 1);
    expectTrue("`partly` is partial", report.partial.len == 1 and
        std.mem.eql(u8, report.partial[0], "partly"));
    expectInt("corpora with no handler", @intCast(report.missing_handlers.len), 1);
    expectTrue("`absent` has no handler", report.missing_handlers.len == 1 and
        std.mem.eql(u8, report.missing_handlers[0], "absent"));
    // A corpus with no handler at all is named once rather than once per file, so the declined list holds
    // only the file of the corpus that is partly answered.
    expectInt("declined files", @intCast(report.declined_files.len), 1);
    expectTrue("the declined file is named", report.declined_files.len == 1 and
        std.mem.eql(u8, report.declined_files[0], "partly/declined"));

    // A handler that answers wrongly is a failure, and its corpus is partial rather than complete even
    // though every file has a handler.
    const wrong = [_]corpus.DispatchEntry{.{ .key = "answered/whole", .handler = alwaysWrong }};
    const red = try corpus.run(a, files[0..1], &wrong);
    expectInt("a wrong answer fails", red.failed, 3);
    expectInt("and passes nothing", red.passed, 0);
    expectInt("and its corpus is not complete", @intCast(red.complete.len), 0);
    expectInt("its corpus is partial", @intCast(red.partial.len), 1);

    // A handler that declines a case it was given is a *failure*, not an unanswered case: the file has a
    // handler, so silence there would be a contract quietly opting out of its own cases.
    const declining = [_]corpus.DispatchEntry{.{ .key = "answered/whole", .handler = declines }};
    const refused = try corpus.run(a, files[0..1], &declining);
    expectInt("a declined case fails", refused.failed, 3);
    expectInt("and is not counted as unimplemented", refused.unanswered, 0);
}

// ── 5. The case tables, against whole strings the engine converted ───────────

fn caseTables(a: Allocator) !void {
    std.debug.print("the case tables\n", .{});

    // Two files generated under different Unicode versions would disagree about characters neither of them
    // is wrong about, and the disagreement would read as hundreds of defects.
    expectText("the tables and the vectors are one Unicode version", js.unicode_version, vectors.vectors_unicode_version);

    // The mapping tables are binary-searched, so their order is load-bearing: a table that lost it would
    // still answer most lookups, which is the failure that passes a corpus run and breaks on one character
    // nobody tested.
    expectTrue("the case tables are ascending", js.tablesSorted());

    // 219 whole strings the engine converted. The tables are single code points and cannot be wrong about a
    // string on their own; the code that walks a string with them, and applies the sigma rule, can — and
    // these are what would say so (finding 163).
    var wrong: u32 = 0;
    for (vectors.case_vectors, 0..) |vector, at| {
        const got = if (vector.upper) try js.upper(a, vector.in) else try js.lower(a, vector.in);
        if (!std.mem.eql(u8, got, vector.out)) {
            wrong += 1;
            if (wrong <= 5) {
                std.debug.print("  vector {d} ({s}): expected \"{s}\", got \"{s}\"\n", .{
                    at, if (vector.upper) "upper" else "lower", vector.out, got,
                });
            }
        }
    }
    checks += 1;
    if (wrong != 0) {
        failures += 1;
        std.debug.print("  {d} of {d} case vectors disagree with the engine\n", .{ wrong, vectors.case_vectors.len });
    }

    // The three mappings the header says a standard library cannot do, named so a reader can see them.
    expectText("a sharp s upper-cases to two letters", try js.upper(a, "\u{00DF}"), "SS");
    expectText("the Kelvin sign lower-cases to an ASCII k", try js.lower(a, "\u{212A}"), "k");
    // A capital sigma at the end of a word is a *different* letter from one in the middle, which is the whole
    // reason the ranges are in the table beside the mappings.
    expectText("a final sigma", try js.lower(a, "\u{039F}\u{03A3}"), "\u{03BF}\u{03C2}");
    expectText("and a medial one", try js.lower(a, "\u{039F}\u{03A3}\u{039F}"), "\u{03BF}\u{03C3}\u{03BF}");

    // `trim` removes ECMAScript's whitespace, which is not ASCII whitespace: the semver corpus holds a range
    // written with a U+FEFF before the version, and the reference trims it.
    expectText("trim removes a zero-width no-break space", js.trim("\u{FEFF} 1.2.3 \u{FEFF}"), "1.2.3");
    expectText("trim leaves an interior space", js.trim("  a b  "), "a b");
    expectText("trim of nothing but space", js.trim("   "), "");
    // U+0085 is *not* ECMAScript whitespace, and a predicate corpus case turns on exactly that.
    expectText("trim leaves a next line", js.trim("\u{0085}"), "\u{0085}");

    // Number::toString is not the canonical writer, and the two differ in both directions.
    sink = 1e-7;
    expectText("a ten-millionth is exponential here", try js.numberToString(a, sink), "1e-7");
    sink = 1e21;
    expectText("and so is ten to the twenty-first", try js.numberToString(a, sink), "1e+21");
    sink = 1e-6;
    expectText("a millionth is not", try js.numberToString(a, sink), "0.000001");
    sink = 1e20;
    expectText("ten to the twentieth is not", try js.numberToString(a, sink), "100000000000000000000");
    sink = 100;
    expectText("an integer", try js.numberToString(a, sink), "100");
    sink = -1.5;
    expectText("a negative", try js.numberToString(a, sink), "-1.5");
    sink = -0.0;
    expectText("negative zero is zero here too", try js.numberToString(a, sink), "0");
    // Where the canonical writer refuses, this renders the word — and the difference matters because these
    // end up inside frozen message strings.
    sink = std.math.nan(f64);
    expectText("NaN is a word", try js.numberToString(a, sink), "NaN");
    sink = std.math.inf(f64);
    expectText("and so is infinity", try js.numberToString(a, sink), "Infinity");

    // `Number(text)` is not `parseFloat`.
    expectTrue("a hex string is a number", js.stringToNumber("0x1f") == 31);
    expectTrue("`Infinity` with that capital", std.math.isInf(js.stringToNumber("Infinity")));
    expectTrue("and not with another", std.math.isNan(js.stringToNumber("inf")));
    expectTrue("a signed hex literal is not a number", std.math.isNan(js.stringToNumber("-0x10")));
    expectTrue("an empty string is zero", js.stringToNumber("") == 0);
    expectTrue("and so is a blank one", js.stringToNumber("   ") == 0);
    expectTrue("an exponent", js.stringToNumber("1e3") == 1000);
    expectTrue("a trailing unit is not a number", std.math.isNan(js.stringToNumber("21.5 c")));

    // ToString over a value, which is what a template literal does.
    expectText("an absent field", try js.toString(a, null, false), "undefined");
    expectText("a null field", try js.toString(a, value.nul(), true), "null");
    const array = try value.array(a);
    try value.push(a, array, try value.number(a, 1));
    try value.push(a, array, value.nul());
    try value.push(a, array, try value.string(a, "a"));
    expectText("an array joins with commas and writes null as nothing", try js.toString(a, array, true), "1,,a");
    expectText("an object", try js.toString(a, try value.object(a), true), "[object Object]");
}

// ── 6. The closed vocabularies ───────────────────────────────────────────────
//
// The corpus checks every set's *members*, and cannot check two things about the table that holds them.
// A duplicate name is the dangerous one: the lookup returns the first match, so a second entry under a
// name already present is dead code that a reader would take for the answer — and the corpus would pass,
// because the first entry is the one it measures.
fn vocabularies(a: Allocator) !void {
    std.debug.print("the closed vocabularies\n", .{});

    var buf: [vocabulary.count][]const u8 = undefined;
    const names = vocabulary.names(&buf);

    // §31.4's sentence about closed vocabularies is about *all* of them, and the corpus names the
    // seventy-four it holds. A set added to the reference and not to this table is a corpus failure; a
    // set removed from this table is a corpus failure too. This is the third case: the table and the
    // corpus agreeing on seventy-four while this edition believes it holds some other number.
    expectInt("seventy-four sets", @intCast(names.len), 74);

    var duplicates: u32 = 0;
    for (names, 0..) |name, at| {
        for (names[at + 1 ..]) |other| {
            if (std.mem.eql(u8, name, other)) {
                duplicates += 1;
                std.debug.print("  `{s}` is declared twice, and the second is unreachable\n", .{name});
            }
        }
    }
    expectTrue("no set is declared twice", duplicates == 0);

    // Every set answers a non-empty array, which is what makes JSON null usable as "no such set": if any
    // set were empty, the two answers would be indistinguishable to a caller.
    var empty: u32 = 0;
    for (names) |name| {
        const ask = try value.object(a);
        try value.set(a, ask, "set", try value.string(a, name));
        const answer = try vocabulary.closedSetsCase(a, ask);
        if (answer.kind != .array or value.len(answer) == 0) {
            empty += 1;
            std.debug.print("  `{s}` answers no members\n", .{name});
        }
    }
    expectTrue("every set has at least one member", empty == 0);

    // **The same seven state names live in two files of this edition**, and nothing in the corpus compares
    // them: `opstate/properties` measures one against the oracle and `vocabulary/closed-sets` measures the
    // other, so two lists that agreed with the oracle at different moments would both still pass while
    // disagreeing with each other. This is the check for that — §27's rule order is load-bearing in one file
    // and frozen output in the other.
    const askStates = try value.object(a);
    try value.set(a, askStates, "set", try value.string(a, "operating_states"));
    const frozen = try vocabulary.closedSetsCase(a, askStates);
    var agree = value.len(frozen) == opstate.states.len;
    for (opstate.states, 0..) |state, at| {
        if (!agree) break;
        const held = value.at(frozen, at) orelse break;
        if (!std.mem.eql(u8, held.text, state)) {
            agree = false;
            std.debug.print("  state {d}: the machine says `{s}`, the vocabulary says `{s}`\n", .{ at, state, held.text });
        }
    }
    expectTrue("the machine's states and the frozen vocabulary are the same list, in the same order", agree);

    // And a name nobody declared is null rather than empty, which is the other half of that.
    const unknown = try value.object(a);
    try value.set(a, unknown, "set", try value.string(a, "a_vocabulary_nobody_declared"));
    expectTrue("an undeclared set answers null", (try vocabulary.closedSetsCase(a, unknown)).isNull());
}

pub fn main(init: std.process.Init) !u8 {
    const a = init.arena.allocator();

    var arguments = init.minimal.args.iterate();
    _ = arguments.skip();
    while (arguments.next()) |argument| {
        if (std.mem.eql(u8, argument, "--corpus-root")) {
            // Accepted and unused: `zig build test` passes the same arguments to both programs, and a
            // self-check that rejected them would make the two lines differ for no reason a reader could
            // see.
            _ = arguments.next();
        } else {
            std.debug.print("usage: skynet-selfcheck-zig [--corpus-root DIR]\n", .{});
            return 2;
        }
    }

    numbers();
    try writer(a);
    parser(a);
    try accounting(a);
    try caseTables(a);
    try vocabularies(a);

    std.debug.print("self check  {d} checks, {d} failed\n", .{ checks, failures });
    return if (failures == 0) 0 else 1;
}
