//! A JSON parser, for the two things this edition reads: the corpus files, and the `expected` strings
//! inside them that the `roundtrip` step re-serialises.
//!
//! Deliberately a parser of JSON rather than of canonical JSON. The corpus files are written by
//! `tools/corpus-gen/generate.ts` through `JSON.stringify` with indentation, so they carry whitespace,
//! unsorted keys and `\uXXXX` escapes for anything this writer would emit as UTF-8 — and a parser that
//! only accepted the canonical form could not read the corpus it exists to read.
//!
//! What it refuses is everything outside JSON proper: a trailing comma, a comment, `NaN`, `Infinity`, a
//! bare word, a leading `+`, a leading zero, a control character inside a string. Each of those is a
//! file this edition cannot have produced, and accepting one would be reading a document nobody wrote.
//! §3.9's rule applies here rather than being softened because the input is a checked-in file: all
//! input from outside the process is untrusted, and a corpus path is an argument.
//!
//! `std.json` is deliberately not used, for the reason the Go edition gives for not using
//! `encoding/json`: it is *nearly* right. It would parse these documents, and then the number that came
//! back would be whatever its own conversion produced and the duplicate-key rule would be whatever it
//! chose this year. §31.4 asks for identical bytes, so the conversion has to be ours.

const std = @import("std");
const Allocator = std.mem.Allocator;
const value = @import("value.zig");
const Value = value.Value;

pub const Error = error{ Invalid, OutOfMemory } || std.fmt.ParseFloatError;

/// Where a parse stopped and why.
pub const Diagnostic = struct {
    message: []const u8 = "",
    offset: usize = 0,
    line: usize = 0,
    column: usize = 0,

    pub fn render(self: Diagnostic, a: Allocator) ![]const u8 {
        return std.fmt.allocPrint(a, "{d}:{d}: {s}", .{ self.line, self.column, self.message });
    }
};

/// Bounds the nesting a document may reach. §7.5: bound recursion on untrusted input. Deeper than any
/// corpus document and shallow enough that the recursion cannot reach the stack this edition runs on.
const max_depth = 512;

const Parser = struct {
    a: Allocator,
    bytes: []const u8,
    at: usize = 0,
    depth: u32 = 0,
    diagnostic: *Diagnostic,

    fn fail(self: *Parser, message: []const u8) Error {
        // Produced once, so the line and column are counted here rather than tracked on every byte.
        var line: usize = 1;
        var column: usize = 1;
        for (self.bytes[0..@min(self.at, self.bytes.len)]) |byte| {
            if (byte == '\n') {
                line += 1;
                column = 1;
            } else column += 1;
        }
        self.diagnostic.* = .{ .message = message, .offset = self.at, .line = line, .column = column };
        return error.Invalid;
    }

    fn atEnd(self: *const Parser) bool {
        return self.at >= self.bytes.len;
    }

    fn peek(self: *const Parser) u8 {
        return if (self.atEnd()) 0 else self.bytes[self.at];
    }

    fn skipSpace(self: *Parser) void {
        while (!self.atEnd()) {
            switch (self.bytes[self.at]) {
                ' ', '\t', '\n', '\r' => self.at += 1,
                else => break,
            }
        }
    }

    fn literal(self: *Parser, word: []const u8) Error!void {
        if (self.bytes.len - self.at < word.len or
            !std.mem.eql(u8, self.bytes[self.at..][0..word.len], word))
        {
            return self.fail("not a JSON value");
        }
        self.at += word.len;
    }

    fn hex4(self: *Parser) Error!u21 {
        if (self.bytes.len - self.at < 4) return self.fail("a truncated \\u escape");
        var out: u21 = 0;
        for (self.bytes[self.at..][0..4]) |byte| {
            const digit: u21 = switch (byte) {
                '0'...'9' => byte - '0',
                'a'...'f' => byte - 'a' + 10,
                'A'...'F' => byte - 'A' + 10,
                else => return self.fail("a \\u escape with a non-hex digit"),
            };
            out = out * 16 + digit;
        }
        self.at += 4;
        return out;
    }

    /// Append one code point as UTF-8. A lone surrogate is written as the code point it is rather than
    /// refused: JavaScript strings admit one, so the reference can hold one, and an edition that refused
    /// it would fail a document the oracle produced. `std.unicode` will not encode a surrogate, so the
    /// three bytes are assembled here.
    fn appendCodepoint(self: *Parser, out: *std.ArrayList(u8), code: u21) Error!void {
        if (code >= 0xD800 and code <= 0xDFFF) {
            try out.append(self.a, @intCast(0xE0 | (code >> 12)));
            try out.append(self.a, @intCast(0x80 | ((code >> 6) & 0x3F)));
            try out.append(self.a, @intCast(0x80 | (code & 0x3F)));
            return;
        }
        var buf: [4]u8 = undefined;
        const width = std.unicode.utf8Encode(code, &buf) catch return self.fail("an unencodable code point");
        try out.appendSlice(self.a, buf[0..width]);
    }

    fn stringBytes(self: *Parser) Error![]const u8 {
        if (self.peek() != '"') return self.fail("expected a string");
        self.at += 1;
        var out: std.ArrayList(u8) = .empty;
        while (true) {
            if (self.atEnd()) return self.fail("an unterminated string");
            const byte = self.bytes[self.at];
            if (byte == '"') {
                self.at += 1;
                break;
            }
            if (byte < 0x20) return self.fail("an unescaped control character in a string");
            if (byte != '\\') {
                try out.append(self.a, byte);
                self.at += 1;
                continue;
            }
            self.at += 1;
            if (self.atEnd()) return self.fail("a trailing backslash in a string");
            const escape = self.bytes[self.at];
            self.at += 1;
            switch (escape) {
                '"' => try out.append(self.a, '"'),
                '\\' => try out.append(self.a, '\\'),
                '/' => try out.append(self.a, '/'),
                'b' => try out.append(self.a, 0x08),
                'f' => try out.append(self.a, 0x0C),
                'n' => try out.append(self.a, '\n'),
                'r' => try out.append(self.a, '\r'),
                't' => try out.append(self.a, '\t'),
                'u' => {
                    var code = try self.hex4();
                    // A surrogate pair is two escapes for one code point, and `JSON.stringify` writes one
                    // for every character above the BMP.
                    if (code >= 0xD800 and code <= 0xDBFF and self.bytes.len - self.at >= 6 and
                        self.bytes[self.at] == '\\' and self.bytes[self.at + 1] == 'u')
                    {
                        const before = self.at;
                        self.at += 2;
                        const low = try self.hex4();
                        if (low >= 0xDC00 and low <= 0xDFFF) {
                            code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                        } else self.at = before;
                    }
                    try self.appendCodepoint(&out, code);
                },
                else => return self.fail("an unknown escape in a string"),
            }
        }
        return out.items;
    }

    fn numberValue(self: *Parser) Error!*const Value {
        const start = self.at;
        if (self.peek() == '-') self.at += 1;
        // JSON forbids a leading zero before another digit, and accepting one would be accepting a
        // document this edition's writer cannot produce.
        if (self.peek() == '0') {
            self.at += 1;
            if (self.peek() >= '0' and self.peek() <= '9') return self.fail("a number with a leading zero");
        } else if (self.peek() >= '1' and self.peek() <= '9') {
            while (self.peek() >= '0' and self.peek() <= '9') self.at += 1;
        } else return self.fail("a number with no digits");
        if (self.peek() == '.') {
            self.at += 1;
            if (!(self.peek() >= '0' and self.peek() <= '9')) return self.fail("a number with no digits after the point");
            while (self.peek() >= '0' and self.peek() <= '9') self.at += 1;
        }
        if (self.peek() == 'e' or self.peek() == 'E') {
            self.at += 1;
            if (self.peek() == '+' or self.peek() == '-') self.at += 1;
            if (!(self.peek() >= '0' and self.peek() <= '9')) return self.fail("a number with no digits in its exponent");
            while (self.peek() >= '0' and self.peek() <= '9') self.at += 1;
        }
        // The run of bytes has been validated as a JSON number above, so this converts rather than
        // parses — and `parseFloat` is the correctly rounded conversion the reference's `Number(...)`
        // also performs.
        const parsed = std.fmt.parseFloat(f64, self.bytes[start..self.at]) catch
            return self.fail("a number this edition cannot convert");
        return value.number(self.a, parsed);
    }

    fn arrayValue(self: *Parser) Error!*const Value {
        self.at += 1;
        const out = try value.array(self.a);
        self.skipSpace();
        if (self.peek() == ']') {
            self.at += 1;
            return out;
        }
        while (true) {
            try value.push(self.a, out, try self.parseValue());
            self.skipSpace();
            if (self.peek() == ',') {
                self.at += 1;
                self.skipSpace();
                // A trailing comma is the one JSON5 habit a hand-edited corpus file would pick up, and it
                // is named separately so the message says what is wrong rather than "not a value".
                if (self.peek() == ']') return self.fail("a trailing comma in an array");
                continue;
            }
            if (self.peek() == ']') {
                self.at += 1;
                return out;
            }
            return self.fail("expected a comma or a closing bracket");
        }
    }

    fn objectValue(self: *Parser) Error!*const Value {
        self.at += 1;
        const out = try value.object(self.a);
        self.skipSpace();
        if (self.peek() == '}') {
            self.at += 1;
            return out;
        }
        while (true) {
            self.skipSpace();
            const key = try self.stringBytes();
            self.skipSpace();
            if (self.peek() != ':') return self.fail("expected a colon after a key");
            self.at += 1;
            // A duplicate key replaces the earlier one in place, which is what the reference's object
            // literal does.
            try value.set(self.a, out, key, try self.parseValue());
            self.skipSpace();
            if (self.peek() == ',') {
                self.at += 1;
                self.skipSpace();
                if (self.peek() == '}') return self.fail("a trailing comma in an object");
                continue;
            }
            if (self.peek() == '}') {
                self.at += 1;
                return out;
            }
            return self.fail("expected a comma or a closing brace");
        }
    }

    fn parseValue(self: *Parser) Error!*const Value {
        if (self.depth >= max_depth) return self.fail("a document nested deeper than this parser will go");
        self.skipSpace();
        if (self.atEnd()) return self.fail("the document ended where a value was expected");
        self.depth += 1;
        defer self.depth -= 1;
        return switch (self.peek()) {
            '{' => self.objectValue(),
            '[' => self.arrayValue(),
            '"' => value.string(self.a, try self.stringBytes()),
            't' => blk: {
                try self.literal("true");
                break :blk value.boolean(true);
            },
            'f' => blk: {
                try self.literal("false");
                break :blk value.boolean(false);
            },
            'n' => blk: {
                try self.literal("null");
                break :blk value.nul();
            },
            else => self.numberValue(),
        };
    }
};

/// Parse `bytes`. Trailing whitespace is allowed; trailing anything else is not, so a file holding two
/// documents fails rather than silently reporting the first.
pub fn parse(a: Allocator, bytes: []const u8, diagnostic: *Diagnostic) Error!*const Value {
    var p: Parser = .{ .a = a, .bytes = bytes, .diagnostic = diagnostic };
    // A UTF-8 byte-order mark, which an editor on Windows will add to a file it saves. Skipped rather
    // than refused, and only at the start.
    if (std.mem.startsWith(u8, bytes, "\xEF\xBB\xBF")) p.at = 3;
    const out = try p.parseValue();
    p.skipSpace();
    if (!p.atEnd()) return p.fail("trailing bytes after the document");
    return out;
}
