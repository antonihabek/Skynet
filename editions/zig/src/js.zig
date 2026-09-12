//! The JavaScript semantics the reference relies on, spelled out.
//!
//! The reference is TypeScript, and a transcription of it is only as faithful as its account of what the
//! engine does. Four of those are not what Zig's standard library does, and the near-equivalent is the
//! trap: an edition built on it agrees on every input anybody thought to test and diverges on one nobody
//! did.
//!
//! | The reference calls | Zig's near-equivalent | Why it is not the same |
//! |---|---|---|
//! | `trim()` | `std.mem.trim` with `&std.ascii.whitespace` | ECMAScript's whitespace set includes U+FEFF and U+00A0 and excludes U+0085. ASCII whitespace is none of those |
//! | `toUpperCase()` | `std.ascii.upperString` | ASCII only, and one-to-one. A sharp s upper-cases to *two* letters and the Kelvin sign lower-cases to an ASCII `k` |
//! | `${value}` | `{d}` on an `f64` | Number::toString writes `1e-7` and `1e+21` where canonical form writes them positionally, and writes `NaN` where canonical form refuses it |
//! | `Number(text)` | `std.fmt.parseFloat` | `Number("0x1f")` is 31, `Number("Infinity")` is infinite with that capital and no other, and `Number("")` is **zero** |
//!
//! Every string here is UTF-8, which is what the value model holds and what the corpus is written in.

const std = @import("std");
const Allocator = std.mem.Allocator;
const value = @import("value.zig");
const tables = @import("unicode_case.zig");

pub const unicode_version = tables.unicode_version;

// ── UTF-8 ────────────────────────────────────────────────────────────────────

/// Decode the code point at `at`, advancing `at` past it.
///
/// A byte that is not part of a well-formed sequence is returned as U+FFFD and consumed one byte at a
/// time, which is what a JavaScript string holding lone surrogates becomes when it is encoded. No corpus
/// document contains one; the behaviour is defined rather than left to run off the end.
pub fn next(text: []const u8, at: *usize) u21 {
    const width = std.unicode.utf8ByteSequenceLength(text[at.*]) catch {
        at.* += 1;
        return 0xFFFD;
    };
    if (at.* + width > text.len) {
        at.* += 1;
        return 0xFFFD;
    }
    const code = std.unicode.utf8Decode(text[at.*..][0..width]) catch {
        at.* += 1;
        return 0xFFFD;
    };
    at.* += width;
    return code;
}

// ── Whitespace ───────────────────────────────────────────────────────────────

/// ECMAScript's WhiteSpace and LineTerminator, which is what `trim()` removes.
///
/// Written out as the *language's* set rather than derived from a Unicode property, because that is what
/// it is: U+FEFF is whitespace to JavaScript and is not `White_Space` in Unicode, and U+0085 is the other
/// way round.
pub fn isSpace(code: u21) bool {
    return switch (code) {
        0x0009, // tab
        0x000A, // line feed
        0x000B, // vertical tab
        0x000C, // form feed
        0x000D, // carriage return
        0x0020, // space
        0x00A0, // no-break space
        0x1680, // ogham space mark
        0x2000...0x200A, // en quad through hair space
        0x2028, // line separator
        0x2029, // paragraph separator
        0x202F, // narrow no-break space
        0x205F, // medium mathematical space
        0x3000, // ideographic space
        0xFEFF, // zero width no-break space
        => true,
        else => false,
    };
}

pub fn trim(text: []const u8) []const u8 {
    var start: usize = 0;
    while (start < text.len) {
        var after = start;
        if (!isSpace(next(text, &after))) break;
        start = after;
    }
    // The end is found by walking forward from `start` and remembering the last position that was not the
    // beginning of a run of whitespace. Walking backwards through UTF-8 needs its own rules about
    // continuation bytes, and a second decoder is a second chance to disagree with the first.
    var end = start;
    var at = start;
    while (at < text.len) {
        var after = at;
        const code = next(text, &after);
        if (!isSpace(code)) end = after;
        at = after;
    }
    return text[start..end];
}

// ── Case ─────────────────────────────────────────────────────────────────────

fn inRanges(ranges: []const tables.CodeRange, code: u21) bool {
    var low: usize = 0;
    var high: usize = ranges.len;
    while (low < high) {
        const middle = low + (high - low) / 2;
        if (code < ranges[middle].first) {
            high = middle;
        } else if (code > ranges[middle].last) {
            low = middle + 1;
        } else return true;
    }
    return false;
}

fn isCased(code: u21) bool {
    return inRanges(&tables.cased_ranges, code);
}

fn isCaseIgnorable(code: u21) bool {
    return inRanges(&tables.case_ignorable_ranges, code);
}

/// The mapping for `code`, or null when it maps to itself. `from` is ascending, so this is a binary
/// search; a table that lost its order would still answer most lookups, which is why the self-check
/// asserts the order rather than trusting it.
fn lookup(from: []const u21, to: []const []const u21, code: u21) ?[]const u21 {
    var low: usize = 0;
    var high: usize = from.len;
    while (low < high) {
        const middle = low + (high - low) / 2;
        if (code < from[middle]) {
            high = middle;
        } else if (code > from[middle]) {
            low = middle + 1;
        } else return to[middle];
    }
    return null;
}

/// True when both mapping tables are ascending and the ranges do not overlap. Asserted by the self-check
/// rather than trusted.
pub fn tablesSorted() bool {
    for (1..tables.upper_from.len) |at| {
        if (tables.upper_from[at - 1] >= tables.upper_from[at]) return false;
    }
    for (1..tables.lower_from.len) |at| {
        if (tables.lower_from[at - 1] >= tables.lower_from[at]) return false;
    }
    for (1..tables.cased_ranges.len) |at| {
        if (tables.cased_ranges[at - 1].last >= tables.cased_ranges[at].first) return false;
    }
    for (1..tables.case_ignorable_ranges.len) |at| {
        if (tables.case_ignorable_ranges[at - 1].last >= tables.case_ignorable_ranges[at].first) return false;
    }
    return true;
}

fn appendCode(a: Allocator, out: *std.ArrayList(u8), code: u21) !void {
    var buf: [4]u8 = undefined;
    const width = std.unicode.utf8Encode(code, &buf) catch {
        // A lone surrogate cannot be encoded by `utf8Encode`; assembled here so a string carrying one
        // survives a case conversion rather than stopping it.
        try out.append(a, @intCast(0xE0 | (code >> 12)));
        try out.append(a, @intCast(0x80 | ((code >> 6) & 0x3F)));
        try out.append(a, @intCast(0x80 | (code & 0x3F)));
        return;
    };
    try out.appendSlice(a, buf[0..width]);
}

pub fn upper(a: Allocator, text: []const u8) ![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    var at: usize = 0;
    while (at < text.len) {
        const code = next(text, &at);
        if (lookup(&tables.upper_from, &tables.upper_to, code)) |mapping| {
            for (mapping) |point| try appendCode(a, &out, point);
        } else try appendCode(a, &out, code);
    }
    return out.items;
}

const capital_sigma: u21 = 0x03A3;
const small_sigma: u21 = 0x03C3;
const final_sigma: u21 = 0x03C2;

/// Unicode's Final_Sigma condition, which is the one context rule `toLowerCase` applies outside the
/// Turkish and Lithuanian tailorings.
///
/// Before: a cased character, skipping case-ignorable ones. After: *not* a cased character, skipping
/// case-ignorable ones. A character that is both cased and case-ignorable is **skipped**, because the
/// condition skips Case_Ignorable first — and that is the class `tools/unicode-case.ts` picks
/// representatives for, so the vectors are what say whether this reading is the engine's.
fn scanCased(text: []const u8, from: usize, forward: bool) bool {
    if (forward) {
        var at = from;
        while (at < text.len) {
            const code = next(text, &at);
            if (isCaseIgnorable(code)) continue;
            return isCased(code);
        }
        return false;
    }
    // Backwards, without a backwards decoder: walk forward from the start recording the last
    // non-ignorable code point before `from`. Same reason `trim` walks forward — one decoder, one set of
    // rules about continuation bytes.
    var found = false;
    var cased = false;
    var at: usize = 0;
    while (at < from and at < text.len) {
        const code = next(text, &at);
        if (at > from) break;
        if (isCaseIgnorable(code)) continue;
        found = true;
        cased = isCased(code);
    }
    return found and cased;
}

pub fn lower(a: Allocator, text: []const u8) ![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    var at: usize = 0;
    while (at < text.len) {
        const start = at;
        const code = next(text, &at);
        if (code == capital_sigma) {
            const before = scanCased(text, start, false);
            const after = scanCased(text, at, true);
            try appendCode(a, &out, if (before and !after) final_sigma else small_sigma);
            continue;
        }
        if (lookup(&tables.lower_from, &tables.lower_to, code)) |mapping| {
            for (mapping) |point| try appendCode(a, &out, point);
        } else try appendCode(a, &out, code);
    }
    return out.items;
}

// ── Numbers and ToString ─────────────────────────────────────────────────────

/// ECMAScript's Number::toString at radix 10 — `String(value)`, **not** the canonical writer.
///
/// The two differ in both directions and the difference is visible in frozen output: this writes `1e-7`
/// and `1e+21` where rule 3 writes them positionally, and writes `NaN` and `Infinity` where rule 3
/// refuses them.
///
/// Stated over three integers: `s` with `k` digits and no trailing zero, and `n` where `s * 10^(n-k)` is
/// the value. `std.fmt.float.render` in scientific mode gives all three at once — its digits are `s`,
/// their count is `k`, and its exponent is `n - 1`.
pub fn numberToString(a: Allocator, number: f64) ![]const u8 {
    if (std.math.isNan(number)) return "NaN";
    if (number == 0) return "0";
    if (std.math.isInf(number)) return if (number < 0) "-Infinity" else "Infinity";

    var buf: [std.fmt.float.bufferSize(.scientific, f64)]u8 = undefined;
    const rendered = try std.fmt.float.render(&buf, @abs(number), .{ .mode = .scientific });

    const marker = std.mem.indexOfScalar(u8, rendered, 'e').?;
    var digits: [32]u8 = undefined;
    var k: usize = 0;
    for (rendered[0..marker]) |byte| {
        if (byte != '.') {
            digits[k] = byte;
            k += 1;
        }
    }
    // ECMAScript's `s` carries no trailing zero, and the rules below count digits.
    while (k > 1 and digits[k - 1] == '0') k -= 1;
    const n = (try std.fmt.parseInt(i64, rendered[marker + 1 ..], 10)) + 1;

    var out: std.ArrayList(u8) = .empty;
    if (number < 0) try out.append(a, '-');
    const digit_count: i64 = @intCast(k);
    if (digit_count <= n and n <= 21) {
        // The digits, then `n - k` zeros.
        try out.appendSlice(a, digits[0..k]);
        try out.appendNTimes(a, '0', @intCast(n - digit_count));
    } else if (0 < n and n <= 21) {
        // A point after `n` digits.
        const point: usize = @intCast(n);
        try out.appendSlice(a, digits[0..point]);
        try out.append(a, '.');
        try out.appendSlice(a, digits[point..k]);
    } else if (-6 < n and n <= 0) {
        try out.appendSlice(a, "0.");
        try out.appendNTimes(a, '0', @intCast(-n));
        try out.appendSlice(a, digits[0..k]);
    } else {
        // Exponential — which is the whole reason the canonical writer does not use this function.
        try out.append(a, digits[0]);
        if (k > 1) {
            try out.append(a, '.');
            try out.appendSlice(a, digits[1..k]);
        }
        try out.print(a, "e{s}{d}", .{ if (n - 1 >= 0) "+" else "-", @abs(n - 1) });
    }
    return out.items;
}

/// ECMAScript's ToString over a parsed JSON value — what a template literal does to whatever it is
/// handed.
///
/// `present` is false for a field that was absent, which is JavaScript's `undefined` and renders as the
/// word — a different rendering from a field that was present and null, exactly as rule 5 says they are
/// different facts.
pub fn toString(a: Allocator, held: ?*const value.Value, present: bool) ![]const u8 {
    if (!present) return "undefined";
    const v = held orelse return "undefined";
    return switch (v.kind) {
        .null => "null",
        .bool => if (v.truth) "true" else "false",
        .number => try numberToString(a, v.number),
        .string => v.text,
        .array => blk: {
            // Array.prototype.join: a null or undefined element is the empty string rather than "null",
            // which is the one place ToString's answer for null is not the word.
            var out: std.ArrayList(u8) = .empty;
            for (v.items.items, 0..) |element, index| {
                if (index > 0) try out.append(a, ',');
                if (value.nullish(element)) continue;
                try out.appendSlice(a, try toString(a, element, true));
            }
            break :blk out.items;
        },
        .object => "[object Object]",
    };
}

/// ECMAScript's ToNumber over a string — `Number(text)`, which is not `parseFloat`.
///
/// Three differences decide corpus cases. `Number("0x1f")` is 31, because a string may hold a non-decimal
/// integer literal — and it carries **no sign** in the grammar, so `-0x10` is NaN where `-16` is -16.
/// `Number("Infinity")` is infinite, spelled with that capital and no other. And `Number("")` is *zero*,
/// not a failure, which is why the reference's `toNumber` guards with its own blank test before
/// converting.
pub fn stringToNumber(text: []const u8) f64 {
    const trimmed = trim(text);
    if (trimmed.len == 0) return 0;

    if (trimmed.len > 2 and trimmed[0] == '0') {
        const base: ?u8 = switch (trimmed[1]) {
            'x', 'X' => 16,
            'o', 'O' => 8,
            'b', 'B' => 2,
            else => null,
        };
        if (base) |radix| {
            var out: f64 = 0;
            for (trimmed[2..]) |byte| {
                const digit: u8 = switch (byte) {
                    '0'...'9' => byte - '0',
                    'a'...'z' => byte - 'a' + 10,
                    'A'...'Z' => byte - 'A' + 10,
                    else => return std.math.nan(f64),
                };
                if (digit >= radix) return std.math.nan(f64);
                out = out * @as(f64, @floatFromInt(radix)) + @as(f64, @floatFromInt(digit));
            }
            return out;
        }
    }

    var at: usize = 0;
    var sign: f64 = 1;
    if (trimmed[at] == '+' or trimmed[at] == '-') {
        sign = if (trimmed[at] == '-') -1 else 1;
        at += 1;
    }
    if (std.mem.eql(u8, trimmed[at..], "Infinity")) return sign * std.math.inf(f64);

    // StrUnsignedDecimalLiteral, validated here and converted by `parseFloat` — the validation is what
    // keeps `parseFloat`'s wider grammar (`inf`, `nan`, a hex float, an underscore) out.
    var scan = at;
    var integer_digits: usize = 0;
    while (scan < trimmed.len and trimmed[scan] >= '0' and trimmed[scan] <= '9') {
        scan += 1;
        integer_digits += 1;
    }
    var fraction_digits: usize = 0;
    if (scan < trimmed.len and trimmed[scan] == '.') {
        scan += 1;
        while (scan < trimmed.len and trimmed[scan] >= '0' and trimmed[scan] <= '9') {
            scan += 1;
            fraction_digits += 1;
        }
    }
    if (integer_digits == 0 and fraction_digits == 0) return std.math.nan(f64);
    if (scan < trimmed.len and (trimmed[scan] == 'e' or trimmed[scan] == 'E')) {
        scan += 1;
        if (scan < trimmed.len and (trimmed[scan] == '+' or trimmed[scan] == '-')) scan += 1;
        var exponent_digits: usize = 0;
        while (scan < trimmed.len and trimmed[scan] >= '0' and trimmed[scan] <= '9') {
            scan += 1;
            exponent_digits += 1;
        }
        if (exponent_digits == 0) return std.math.nan(f64);
    }
    if (scan != trimmed.len) return std.math.nan(f64);
    return std.fmt.parseFloat(f64, trimmed) catch std.math.nan(f64);
}
