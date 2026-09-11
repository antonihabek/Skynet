//! The embodiment descriptor and the role vocabulary — `reference/typescript/embodiment.ts`.
//!
//! The role vocabulary is the part of this module the whole system leans on: a role is how two bodies from
//! two vendors are recognised as having the same shoulder, and §6.3's transport plan is built out of the
//! match. The rule that keeps it honest is that an unrecognised role never *silently* maps to something
//! plausible — `parseRole` reports that it could not read one, and the caller treats that as "this joint
//! can only be matched by exact name". A body the vocabulary does not describe is still usable; it just
//! cannot take part in cross-body remapping, which is the truthful outcome, because nobody has
//! established what its joints mean.

const std = @import("std");
const Allocator = std.mem.Allocator;
const value = @import("../value.zig");
const js = @import("../js.zig");
const Value = value.Value;

/// Twelve chains, and `aux` and `tail` are in the list so that a body nobody anticipated — a snake robot's
/// segments, a second mast, a tail-mounted sensor boom — can still declare a role that parses, rather than
/// being pushed into a wrong-but-close chain because the vocabulary had no honest slot for it.
pub const canonical_chains = [_][]const u8{
    "left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand",
    "torso",    "neck",      "head",     "base",      "tail",      "aux",
};

/// Axis of motion, when a link has more than one degree of freedom. A three-DoF shoulder is three joints in
/// every real driver, and the axis suffix is what keeps `left_arm.shoulder.pitch` from colliding with
/// `left_arm.shoulder.roll`.
pub const canonical_axes = [_][]const u8{
    "pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip",
};

pub const Role = struct {
    chain: []const u8,
    link: []const u8,
    /// Null when the role named no axis, which two segments always mean: `torso.yaw` is a link called
    /// `yaw`, not an axis.
    axis: ?[]const u8,
    /// An ordinal for repeated links. Null is the reference's null, and it is optional rather than a
    /// sentinel because `base.wheel#0` is a valid role whose index is zero — which is the corpus case
    /// `ordinal-zero` exists for.
    index: ?f64,
};

fn inList(list: []const []const u8, text: []const u8) bool {
    for (list) |held| {
        if (std.mem.eql(u8, held, text)) return true;
    }
    return false;
}

fn lowerOrUnderscore(byte: u8) bool {
    return (byte >= 'a' and byte <= 'z') or byte == '_';
}

fn linkByte(byte: u8) bool {
    return lowerOrUnderscore(byte) or (byte >= '0' and byte <= '9');
}

fn lowerByte(byte: u8) bool {
    return byte >= 'a' and byte <= 'z';
}

fn digitByte(byte: u8) bool {
    return byte >= '0' and byte <= '9';
}

fn runOf(text: []const u8, at: usize, member: *const fn (u8) bool) usize {
    var end = at;
    while (end < text.len and member(text[end])) end += 1;
    return end - at;
}

pub fn parseRoleText(a: Allocator, role: []const u8) !?Role {
    // `role.trim().toLowerCase()`, in that order and with the engine's full mapping: the corpus holds
    // `necK.link.pitch`, which is a chain only after lower-casing, and a role wrapped in U+FEFF, which is
    // whitespace only to JavaScript.
    const text = try js.lower(a, js.trim(role));

    // `^([a-z_]+)\.([a-z0-9_]+)(?:\.([a-z]+))?(?:#(\d+))?$`. Every character class here is disjoint from
    // the separator that follows it, so a greedy run is the only run that can match and there is nothing
    // to backtrack.
    var at: usize = 0;
    const chain_len = runOf(text, at, lowerOrUnderscore);
    if (chain_len == 0) return null;
    const chain = text[at..][0..chain_len];
    at += chain_len;
    if (at >= text.len or text[at] != '.') return null;
    at += 1;
    const link_len = runOf(text, at, linkByte);
    if (link_len == 0) return null;
    const link = text[at..][0..link_len];
    at += link_len;

    var axis: ?[]const u8 = null;
    if (at < text.len and text[at] == '.') {
        const after = at + 1;
        const axis_len = runOf(text, after, lowerByte);
        // The axis group is optional, but a point with nothing readable after it cannot be consumed by
        // anything else either, so the whole role fails — `a.b.3` and `a.b.` both.
        if (axis_len == 0) return null;
        axis = text[after..][0..axis_len];
        at = after + axis_len;
    }

    var index: ?f64 = null;
    if (at < text.len and text[at] == '#') {
        const after = at + 1;
        const index_len = runOf(text, after, digitByte);
        if (index_len == 0) return null;
        index = std.fmt.parseFloat(f64, text[after..][0..index_len]) catch return null;
        at = after + index_len;
    }
    if (at != text.len) return null;

    if (!inList(&canonical_chains, chain)) return null;
    if (axis) |held| {
        if (!inList(&canonical_axes, held)) return null;
    }
    return .{ .chain = chain, .link = link, .axis = axis, .index = index };
}

/// `if (!role) return null` — a null, an absent field and an empty string are all falsy, and all three mean
/// the same thing here: no role was declared.
pub fn parseRole(a: Allocator, role: ?*const Value) !?Role {
    const held = role orelse return null;
    if (held.kind != .string or held.text.len == 0) return null;
    return parseRoleText(a, held.text);
}

/// The canonical string form, so two parsed roles can be compared as keys.
pub fn roleKey(a: Allocator, role: Role) ![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    try out.appendSlice(a, role.chain);
    try out.append(a, '.');
    try out.appendSlice(a, role.link);
    if (role.axis) |axis| {
        try out.append(a, '.');
        try out.appendSlice(a, axis);
    }
    if (role.index) |index| {
        try out.append(a, '#');
        try out.appendSlice(a, try js.numberToString(a, index));
    }
    return out.items;
}

pub fn rolesVocabularyCase(a: Allocator, input: ?*const Value) anyerror!*const Value {
    const parsed = (try parseRole(a, value.get(input, "role"))) orelse return value.nul();
    const out = try value.object(a);
    try value.set(a, out, "chain", try value.string(a, parsed.chain));
    try value.set(a, out, "link", try value.string(a, parsed.link));
    try value.set(a, out, "axis", if (parsed.axis) |axis| try value.string(a, axis) else value.nul());
    try value.set(a, out, "index", if (parsed.index) |index| try value.number(a, index) else value.nul());
    try value.set(a, out, "key", try value.string(a, try roleKey(a, parsed)));
    return out;
}
