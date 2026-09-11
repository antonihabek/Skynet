//! The canonical JSON value model and writer — this edition's half of
//! `reference/typescript/canonical.ts`.
//!
//! §31.4 requires byte-identical output, and corpus/README.md states the form: no insignificant
//! whitespace, object keys sorted by their UTF-8 bytes, numbers finite and positional, only the quote,
//! the backslash and the C0 controls escaped, absent fields omitted and `null` written.
//!
//! # Rule 3 is the one place Zig's standard library does the work
//!
//! `std.fmt.float.render` in `.decimal` mode is shortest-round-trip and **positional** — it writes
//! `0.0000001` where `JSON.stringify` writes `1e-7`, and `1000000000000000000000` where every C library
//! reaches for an exponent. That is rule 3 almost exactly, and it is worth saying plainly because the
//! C edition beside this one implements the whole search by hand: a precision loop against `strtod`,
//! then an exponential-to-positional conversion. Here there is one departure to make, `-0`, and the
//! rest of the rule is the library's.
//!
//! The *reason* to say so rather than just use it: an edition whose number formatting is a library call
//! is an edition whose number formatting changes when the library does. The `roundtrip` step is what
//! holds it — every number the reference ever wrote into `corpus/`, parsed and re-serialised here.
//!
//! # Strings carry a length, because Zig's already do
//!
//! A `[]const u8` is bytes and a count, so the U+0000 a corpus input can hold costs nothing to carry.
//! In C that was a decision; here it is the default, and the note exists so a reader comparing the two
//! editions does not go looking for the equivalent care.

const std = @import("std");
const Allocator = std.mem.Allocator;

pub const Kind = enum { null, bool, number, string, array, object };

pub const Member = struct {
    key: []const u8,
    value: *const Value,
};

/// A value. Built by appending and read by index or key, which is what the corpus documents need and
/// nothing more: there is no removal, because no contract removes anything.
pub const Value = struct {
    kind: Kind = .null,
    truth: bool = false,
    number: f64 = 0,
    text: []const u8 = "",
    items: std.ArrayList(*const Value) = .empty,
    members: std.ArrayList(Member) = .empty,

    pub fn isNull(self: *const Value) bool {
        return self.kind == .null;
    }
};

/// The three values with no fields, one per process rather than one per construction.
var null_value: Value = .{ .kind = .null };
var true_value: Value = .{ .kind = .bool, .truth = true };
var false_value: Value = .{ .kind = .bool, .truth = false };

pub fn nul() *const Value {
    return &null_value;
}

pub fn boolean(truth: bool) *const Value {
    return if (truth) &true_value else &false_value;
}

pub fn number(a: Allocator, value: f64) !*const Value {
    const out = try a.create(Value);
    out.* = .{ .kind = .number, .number = value };
    return out;
}

/// `text` is not copied. Every caller either owns it in the arena already or is handing over a literal,
/// and a copy per string in a document of ten thousand of them is a cost with no reader.
pub fn string(a: Allocator, text: []const u8) !*const Value {
    const out = try a.create(Value);
    out.* = .{ .kind = .string, .text = text };
    return out;
}

pub fn array(a: Allocator) !*Value {
    const out = try a.create(Value);
    out.* = .{ .kind = .array };
    return out;
}

pub fn push(a: Allocator, list: *Value, item: *const Value) !void {
    try list.items.append(a, item);
}

pub fn object(a: Allocator) !*Value {
    const out = try a.create(Value);
    out.* = .{ .kind = .object };
    return out;
}

/// Append `key`, replacing a key already present **in place** so it keeps its original position — which
/// is what assigning to a field twice does in the reference, and matters because the order a contract
/// inserted in is what a failure prints.
pub fn set(a: Allocator, obj: *Value, key: []const u8, value: *const Value) !void {
    for (obj.members.items) |*member| {
        if (std.mem.eql(u8, member.key, key)) {
            member.value = value;
            return;
        }
    }
    try obj.members.append(a, .{ .key = key, .value = value });
}

/// `set` unless `value` is null — the reference's `undefined`, which rule 5 omits. Spelled differently
/// from `set` so that omitting a field is a decision at the call site rather than a value that happened
/// to be absent.
pub fn setOptional(a: Allocator, obj: *Value, key: []const u8, value: ?*const Value) !void {
    if (value) |held| try set(a, obj, key, held);
}

// ── Reading ──────────────────────────────────────────────────────────────────
//
// Every accessor is total: a kind mismatch answers the fallback rather than trapping, because a
// contract reading a document reads whatever the document holds — exactly as the reference does when a
// field's declared type and its actual value disagree.

/// True when the field is absent (Zig's null) or present and JSON null — the reference's `x == null`,
/// which is the loose comparison it uses throughout and which is true of both.
pub fn nullish(value: ?*const Value) bool {
    return value == null or value.?.kind == .null;
}

pub fn asBool(value: ?*const Value, fallback: bool) bool {
    const held = value orelse return fallback;
    return if (held.kind == .bool) held.truth else fallback;
}

pub fn asNumber(value: ?*const Value, fallback: f64) f64 {
    const held = value orelse return fallback;
    return if (held.kind == .number) held.number else fallback;
}

pub fn asString(value: ?*const Value, fallback: []const u8) []const u8 {
    const held = value orelse return fallback;
    return if (held.kind == .string) held.text else fallback;
}

pub fn stringIs(value: ?*const Value, literal: []const u8) bool {
    const held = value orelse return false;
    return held.kind == .string and std.mem.eql(u8, held.text, literal);
}

pub fn len(value: ?*const Value) usize {
    const held = value orelse return 0;
    return switch (held.kind) {
        .array => held.items.items.len,
        .object => held.members.items.len,
        else => 0,
    };
}

/// Element `index`, or null past the end — which `nullish` reads as the reference's `undefined`.
pub fn at(value: ?*const Value, index: usize) ?*const Value {
    const held = value orelse return null;
    if (held.kind != .array or index >= held.items.items.len) return null;
    return held.items.items[index];
}

/// Member `key`, or null when absent.
pub fn get(value: ?*const Value, key: []const u8) ?*const Value {
    const held = value orelse return null;
    if (held.kind != .object) return null;
    for (held.members.items) |member| {
        if (std.mem.eql(u8, member.key, key)) return member.value;
    }
    return null;
}

/// The members in insertion order, for the contracts that iterate one. Empty for anything else.
pub fn members(value: ?*const Value) []const Member {
    const held = value orelse return &.{};
    if (held.kind != .object) return &.{};
    return held.members.items;
}

/// True when `value` is an object with a member `key`, present even if null — the reference's
/// `key in object`, which is a different question from `object[key] != null`.
pub fn has(value: ?*const Value, key: []const u8) bool {
    const held = value orelse return false;
    if (held.kind != .object) return false;
    for (held.members.items) |member| {
        if (std.mem.eql(u8, member.key, key)) return true;
    }
    return false;
}

// ── The writer ───────────────────────────────────────────────────────────────

pub const WriteError = error{ NonFiniteNumber, NoSpaceLeft } || Allocator.Error || std.fmt.float.Error;

/// Rule 3, with the one departure from `std.fmt.float.render` written out.
pub fn formatNumber(buf: []u8, value: f64) WriteError![]const u8 {
    if (!std.math.isFinite(value)) return error.NonFiniteNumber;
    // Negative zero. `render` writes `-0`; rule 3 writes `0`, and the comparison rather than a sign-bit
    // test is the rule itself: `-0.0 == 0.0` is true and that is the whole of it.
    if (value == 0) {
        buf[0] = '0';
        return buf[0..1];
    }
    return std.fmt.float.render(buf, value, .{ .mode = .decimal });
}

/// A buffer large enough for any `f64` in `.decimal` mode, which for the smallest subnormal is over
/// three hundred digits. Taken from the library rather than guessed.
pub const number_buffer_size = std.fmt.float.bufferSize(.decimal, f64);

/// Rule 2: keys ascending by their UTF-8 byte sequence. `std.mem.order` compares the shared bytes and
/// then the lengths, which is what makes `a` sort before `ab` — the comparison a `memcmp` over the
/// shared prefix alone cannot decide.
fn keyBefore(_: void, left: Member, right: Member) bool {
    return std.mem.order(u8, left.key, right.key) == .lt;
}

/// Rule 4: escape the quote, the backslash and the C0 controls, and nothing else.
///
/// Bytes above 0x7F are copied through untouched, which is the rule read literally and is also the only
/// reading that produces the reference's bytes: `JSON.stringify` emits UTF-8 for every non-control
/// character, and an edition that escaped non-ASCII would differ from the corpus on every case carrying
/// a degree sign.
fn writeString(text: []const u8, out: *std.ArrayList(u8), a: Allocator) !void {
    try out.append(a, '"');
    for (text) |byte| {
        switch (byte) {
            '"' => try out.appendSlice(a, "\\\""),
            '\\' => try out.appendSlice(a, "\\\\"),
            0x08 => try out.appendSlice(a, "\\b"),
            0x0C => try out.appendSlice(a, "\\f"),
            '\n' => try out.appendSlice(a, "\\n"),
            '\r' => try out.appendSlice(a, "\\r"),
            '\t' => try out.appendSlice(a, "\\t"),
            else => {
                if (byte < 0x20) {
                    var escape: [6]u8 = undefined;
                    const rendered = try std.fmt.bufPrint(&escape, "\\u{x:0>4}", .{byte});
                    try out.appendSlice(a, rendered);
                } else {
                    try out.append(a, byte);
                }
            },
        }
    }
    try out.append(a, '"');
}

fn writeValue(a: Allocator, value: ?*const Value, out: *std.ArrayList(u8)) WriteError!void {
    const held = value orelse {
        try out.appendSlice(a, "null");
        return;
    };
    switch (held.kind) {
        .null => try out.appendSlice(a, "null"),
        .bool => try out.appendSlice(a, if (held.truth) "true" else "false"),
        .number => {
            var buf: [number_buffer_size]u8 = undefined;
            try out.appendSlice(a, try formatNumber(&buf, held.number));
        },
        .string => try writeString(held.text, out, a),
        .array => {
            try out.append(a, '[');
            for (held.items.items, 0..) |item, index| {
                if (index > 0) try out.append(a, ',');
                try writeValue(a, item, out);
            }
            try out.append(a, ']');
        },
        .object => {
            const sorted = try a.dupe(Member, held.members.items);
            std.mem.sort(Member, sorted, {}, keyBefore);
            try out.append(a, '{');
            for (sorted, 0..) |member, index| {
                if (index > 0) try out.append(a, ',');
                try writeString(member.key, out, a);
                try out.append(a, ':');
                try writeValue(a, member.value, out);
            }
            try out.append(a, '}');
        },
    }
}

/// Serialise `value` in canonical form.
pub fn encode(a: Allocator, value: ?*const Value) WriteError![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    try writeValue(a, value, &out);
    return out.items;
}
