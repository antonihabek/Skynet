//! Minimal semver parsing, comparison and range satisfaction — `reference/typescript/semver.ts`.
//!
//! It is a contract rather than a utility because §54.2 made `semver_satisfies` an operator the robot
//! evaluates before a skill may start: a robot deciding whether its runtime satisfies `^1.2.0`
//! differently from the Hub that sold it the skill is the §31.2 failure, *a format with two
//! implementations of its own definition is not a format*.
//!
//! Note what is deliberately **not** shared with it. `package.ts` compares OS versions with its own
//! numeric-segment comparator, because `min_os_version` is matched against whatever an image reports,
//! which is `2.0` as often as `2.0.0`. Two comparators with two jobs.

const std = @import("std");
const Allocator = std.mem.Allocator;
const value = @import("../value.zig");
const js = @import("../js.zig");
const Value = value.Value;

/// A parsed version, or null for anything that is not `major.minor.patch[-prerelease]`.
///
/// The three components are `f64` because the reference's are: `Number(match[1])` over a digit run with no
/// length limit, so a version whose major is twenty digits long parses to the double nearest it and is
/// written back out that way. An integer here would answer a different question from the oracle's on
/// exactly that input.
pub const Version = struct {
    major: f64,
    minor: f64,
    patch: f64,
    /// Null when there is no prerelease, which is a different fact from an empty one — the grammar cannot
    /// produce an empty prerelease, and the comparison treats absence as outranking.
    prerelease: ?[]const u8,
    raw: []const u8,
};

fn asciiDigit(byte: u8) bool {
    return byte >= '0' and byte <= '9';
}

/// `[0-9A-Za-z.-]`, the prerelease character class.
fn prereleaseByte(byte: u8) bool {
    return asciiDigit(byte) or (byte >= 'A' and byte <= 'Z') or (byte >= 'a' and byte <= 'z') or
        byte == '.' or byte == '-';
}

/// A maximal run of ASCII digits at `at`. `\d` in the reference's regex has no `u` flag and no Unicode
/// property, so it is ASCII 0-9 and nothing else — which is what makes the corpus case written in
/// Arabic-Indic digits unreadable rather than a version.
fn digitRun(text: []const u8, at: usize) usize {
    var end = at;
    while (end < text.len and asciiDigit(text[end])) end += 1;
    return end - at;
}

/// `Number(...)` over a run of ASCII digits: the double nearest the value the digits spell.
fn numberOf(text: []const u8) f64 {
    return std.fmt.parseFloat(f64, text) catch 0;
}

pub fn parse(text: []const u8) ?Version {
    const input = js.trim(text);

    // `^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$`, anchored at both ends. Digits and the point are
    // disjoint character classes, so a greedy run followed by a required point needs no backtracking: the
    // maximal run is the only run that can be followed by one.
    var at: usize = 0;
    const major_len = digitRun(input, at);
    if (major_len == 0) return null;
    const major_at = at;
    at += major_len;
    if (at >= input.len or input[at] != '.') return null;
    at += 1;
    const minor_len = digitRun(input, at);
    if (minor_len == 0) return null;
    const minor_at = at;
    at += minor_len;
    if (at >= input.len or input[at] != '.') return null;
    at += 1;
    const patch_len = digitRun(input, at);
    if (patch_len == 0) return null;
    const patch_at = at;
    at += patch_len;

    var prerelease: ?[]const u8 = null;
    if (at < input.len) {
        if (input[at] != '-') return null;
        at += 1;
        const start = at;
        while (at < input.len and prereleaseByte(input[at])) at += 1;
        if (at == start or at != input.len) return null;
        prerelease = input[start..at];
    }

    return .{
        .major = numberOf(input[major_at..][0..major_len]),
        .minor = numberOf(input[minor_at..][0..minor_len]),
        .patch = numberOf(input[patch_at..][0..patch_len]),
        .prerelease = prerelease,
        .raw = input,
    };
}

pub fn validVersion(text: []const u8) bool {
    return parse(text) != null;
}

/// Is every byte of this identifier an ASCII digit? The reference's `/^\d+$/` over a `.`-separated part,
/// which decides whether the part compares numerically or as a string.
fn allDigits(part: []const u8) bool {
    if (part.len == 0) return false;
    for (part) |byte| {
        if (!asciiDigit(byte)) return false;
    }
    return true;
}

/// `text.split('.')[index]`, with null past the end — the reference's `undefined`, which the comparison
/// reads as "this side ran out of identifiers".
fn dotPart(text: []const u8, index: usize) ?[]const u8 {
    var at: usize = 0;
    var which: usize = 0;
    while (true) {
        const start = at;
        while (at < text.len and text[at] != '.') at += 1;
        if (which == index) return text[start..at];
        if (at >= text.len) return null;
        at += 1;
        which += 1;
    }
}

fn dotCount(text: []const u8) usize {
    var parts: usize = 1;
    for (text) |byte| {
        if (byte == '.') parts += 1;
    }
    return parts;
}

/// Prerelease identifiers per semver §11: numeric below alphanumeric, and a longer list wins a tie.
///
/// The string comparison is JavaScript's `<`, which compares UTF-16 code units. Every byte a prerelease
/// may hold is ASCII, so a byte comparison is the same comparison here — and that is a property of the
/// grammar rather than a convenience, which is why it is written down.
fn comparePrerelease(left: ?[]const u8, right: ?[]const u8) i8 {
    if (left == null and right == null) return 0;
    // A version without a prerelease outranks one with it.
    if (left == null) return 1;
    if (right == null) return -1;
    const a = left.?;
    const b = right.?;
    if (std.mem.eql(u8, a, b)) return 0;

    const length = @max(dotCount(a), dotCount(b));
    for (0..length) |index| {
        const ai = dotPart(a, index) orelse return -1;
        const bi = dotPart(b, index) orelse return 1;
        const left_numeric = allDigits(ai);
        const right_numeric = allDigits(bi);
        if (left_numeric and right_numeric) {
            const difference = numberOf(ai) - numberOf(bi);
            if (difference != 0) return if (difference < 0) -1 else 1;
            continue;
        }
        if (left_numeric) return -1;
        if (right_numeric) return 1;
        switch (std.mem.order(u8, ai, bi)) {
            .lt => return -1,
            .gt => return 1,
            .eq => {},
        }
    }
    return 0;
}

/// -1 if a < b, 0 if equal, 1 if a > b — and **0 for anything either side could not parse**, which is the
/// reference's answer and the reason `package.ts` keeps its own comparator: a strict parser in that
/// position would silently admit a package onto an older OS.
pub fn compare(a: ?Version, b: ?Version) i8 {
    const left = a orelse return 0;
    const right = b orelse return 0;
    if (left.major != right.major) return if (left.major < right.major) -1 else 1;
    if (left.minor != right.minor) return if (left.minor < right.minor) -1 else 1;
    if (left.patch != right.patch) return if (left.patch < right.patch) -1 else 1;
    return comparePrerelease(left.prerelease, right.prerelease);
}

// ── Ranges ───────────────────────────────────────────────────────────────────

const Op = enum { eq, gte, gt, lte, lt, caret, tilde };

const Comparator = struct { op: Op, version: Version };

const Operator = struct { spelling: []const u8, op: Op };

/// The alternation order is the reference's and it decides two corpus cases: `>=` is tried before `>`, so
/// `>=1.0.0` is one comparator rather than `>` applied to `=1.0.0`. A token with no operator is an
/// equality comparator.
const operators = [_]Operator{
    .{ .spelling = ">=", .op = .gte },
    .{ .spelling = "<=", .op = .lte },
    .{ .spelling = ">", .op = .gt },
    .{ .spelling = "<", .op = .lt },
    .{ .spelling = "^", .op = .caret },
    .{ .spelling = "~", .op = .tilde },
    .{ .spelling = "=", .op = .eq },
};

fn operatorAt(text: []const u8, at: usize) ?Operator {
    for (operators) |candidate| {
        if (std.mem.startsWith(u8, text[at..], candidate.spelling)) return candidate;
    }
    return null;
}

/// `\s*` — JavaScript's whitespace class, which is the same set `trim()` removes and includes U+FEFF. A
/// range written with one before its version is a corpus case.
fn skipSpace(text: []const u8, at: usize) usize {
    var scan = at;
    while (scan < text.len) {
        var after = scan;
        if (!js.isSpace(js.next(text, &after))) break;
        scan = after;
    }
    return scan;
}

/// One token of the reference's tokenizer:
///
///     /(?:>=|<=|>|<|\^|~|=)?\s*\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?/g
///
/// Null when nothing matches at `at`, which is when the caller advances one byte and tries again — the way
/// a global regex walks a string. Tokenizing on the operator rather than on whitespace is what makes
/// `>= 1.2.0` and `>=1.2.0` the same range, and `>=1.0.0<2.0.0` two comparators.
fn tokenAt(text: []const u8, at: usize) ?usize {
    var scan = at;
    if (operatorAt(text, scan)) |found| scan += found.spelling.len;
    scan = skipSpace(text, scan);
    const major = digitRun(text, scan);
    if (major == 0) return null;
    scan += major;
    if (scan >= text.len or text[scan] != '.') return null;
    scan += 1;
    const minor = digitRun(text, scan);
    if (minor == 0) return null;
    scan += minor;
    if (scan >= text.len or text[scan] != '.') return null;
    scan += 1;
    const patch = digitRun(text, scan);
    if (patch == 0) return null;
    scan += patch;
    if (scan < text.len and text[scan] == '-') {
        const start = scan + 1;
        var run = start;
        while (run < text.len and prereleaseByte(text[run])) run += 1;
        // The prerelease group is optional, so a lone `-` with nothing readable after it simply is not
        // part of the token rather than failing it.
        if (run > start) scan = run;
    }
    return scan;
}

fn parseComparator(token: []const u8) ?Comparator {
    const trimmed = js.trim(token);
    var at: usize = 0;
    var op: Op = .eq;
    if (operatorAt(trimmed, at)) |found| {
        op = found.op;
        at += found.spelling.len;
    }
    at = skipSpace(trimmed, at);
    // `(.+)` requires at least one character, and `parse` trims again.
    if (at >= trimmed.len) return null;
    const version = parse(trimmed[at..]) orelse return null;
    return .{ .op = op, .version = version };
}

fn satisfiesComparator(target: Version, against: Comparator) bool {
    const order = compare(target, against.version);
    return switch (against.op) {
        .gte => order >= 0,
        .gt => order > 0,
        .lte => order <= 0,
        .lt => order < 0,
        .eq => order == 0,
        .caret => blk: {
            // Caret: compatible within the leftmost non-zero component.
            if (order < 0) break :blk false;
            const base = against.version;
            if (base.major > 0) break :blk target.major == base.major;
            if (base.minor > 0) break :blk target.major == 0 and target.minor == base.minor;
            break :blk target.major == 0 and target.minor == 0 and target.patch == base.patch;
        },
        // Tilde: patch-level changes within the same minor.
        .tilde => order >= 0 and target.major == against.version.major and
            target.minor == against.version.minor,
    };
}

const Group = struct {
    /// False when a token the tokenizer produced was refused by the comparator parser — which makes the
    /// whole group unreadable, exactly as `comparators.some(c => c === null)` does.
    readable: bool,
    comparators: []const Comparator,
};

const ParsedRange = struct {
    /// Every version satisfies it and there is nothing to compare against.
    wildcard: bool,
    groups: []const Group,
};

/// A range, parsed into its `||` groups.
///
/// A group that does not parse is marked unreadable rather than failing the whole range, which is what
/// keeps one unreadable alternative from invalidating the readable ones — and `validRange` is the caller
/// that wants the stricter answer, which it gets by asking about every group.
fn parseRange(a: Allocator, range: []const u8) !ParsedRange {
    const trimmed = js.trim(range);
    if (trimmed.len == 0 or std.mem.eql(u8, trimmed, "*")) return .{ .wildcard = true, .groups = &.{} };
    if (std.mem.eql(u8, try js.lower(a, trimmed), "any")) return .{ .wildcard = true, .groups = &.{} };

    var groups: std.ArrayList(Group) = .empty;
    // `split('||')`, which yields one more piece than there are separators and can yield empty ones.
    var pieces = std.mem.splitSequence(u8, trimmed, "||");
    while (pieces.next()) |raw| {
        const piece = js.trim(raw);
        var comparators: std.ArrayList(Comparator) = .empty;
        var unreadable = false;
        // The global tokenizer: at each position, match or advance one byte.
        var scan: usize = 0;
        while (scan < piece.len) {
            const end = tokenAt(piece, scan) orelse {
                scan += 1;
                continue;
            };
            const held = parseComparator(piece[scan..end]) orelse {
                unreadable = true;
                break;
            };
            try comparators.append(a, held);
            scan = end;
        }
        try groups.append(a, .{
            .readable = !unreadable and comparators.items.len > 0,
            .comparators = comparators.items,
        });
    }
    return .{ .wildcard = false, .groups = groups.items };
}

pub fn satisfies(a: Allocator, version: []const u8, range: []const u8) !bool {
    const target = parse(version) orelse return false;
    const groups = try parseRange(a, range);
    if (groups.wildcard) return true;
    for (groups.groups) |group| {
        if (!group.readable) continue;
        var all = true;
        for (group.comparators) |held| {
            if (!satisfiesComparator(target, held)) {
                all = false;
                break;
            }
        }
        if (all) return true;
    }
    return false;
}

/// Is `range` a range this edition can read at all?
///
/// `satisfies` answers false both for "this version is outside that window" and for "that is not a
/// window", and §54.2 needs those apart: a skill whose precondition names an unreadable range is a skill
/// nobody can ever run here, and its publisher is the one who has to hear about it.
pub fn validRange(a: Allocator, range: []const u8) !bool {
    const groups = try parseRange(a, range);
    if (groups.wildcard) return true;
    for (groups.groups) |group| {
        if (!group.readable) return false;
    }
    return true;
}

// ── The shapes, from `reference/typescript/shape.ts` ──────────────────────────

fn shapeVersion(a: Allocator, held: ?Version) !*const Value {
    const version = held orelse return value.nul();
    const out = try value.object(a);
    try value.set(a, out, "major", try value.number(a, version.major));
    try value.set(a, out, "minor", try value.number(a, version.minor));
    try value.set(a, out, "patch", try value.number(a, version.patch));
    try value.set(a, out, "prerelease", if (version.prerelease) |held_prerelease|
        try value.string(a, held_prerelease)
    else
        value.nul());
    try value.set(a, out, "raw", try value.string(a, version.raw));
    return out;
}

pub fn satisfiesCase(a: Allocator, input: ?*const Value) anyerror!*const Value {
    const version = value.asString(value.get(input, "version"), "");
    const range = value.asString(value.get(input, "range"), "");

    const out = try value.object(a);
    try value.set(a, out, "version", try value.string(a, version));
    try value.set(a, out, "range", try value.string(a, range));
    try value.set(a, out, "satisfies", value.boolean(try satisfies(a, version, range)));
    // Validity beside satisfaction, because the two are different facts: pinning only the boolean would
    // let an edition agree about every satisfaction and disagree about which ranges it could read at all —
    // the difference between refusing a skill and telling its publisher why.
    try value.set(a, out, "valid_version", value.boolean(validVersion(version)));
    try value.set(a, out, "valid_range", value.boolean(try validRange(a, range)));
    return out;
}

pub fn compareCase(a: Allocator, input: ?*const Value) anyerror!*const Value {
    const left = parse(value.asString(value.get(input, "a"), ""));
    const right = parse(value.asString(value.get(input, "b"), ""));

    const out = try value.object(a);
    try value.set(a, out, "compare", try value.number(a, @floatFromInt(compare(left, right))));
    try value.set(a, out, "a", try shapeVersion(a, left));
    try value.set(a, out, "b", try shapeVersion(a, right));
    return out;
}
