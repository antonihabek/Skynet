package canonical

import (
	"fmt"
	"strconv"
	"strings"
	"unicode/utf16"
	"unicode/utf8"
)

// Decode parses JSON text into a Value.
//
// Written rather than delegated to encoding/json for a reason that is the whole point of this
// package: encoding/json unmarshals objects into map[string]any, which loses insertion order, and
// it cannot distinguish an absent field from a null one without a second pass. Both facts are
// load-bearing here (see the package comment), so the corpus's inputs are parsed by the same value
// model the outputs are built in — one representation, one set of rules, one place to be wrong.
//
// It is also the other half of the round-trip proof: `cmd/roundtrip` parses every `expected`
// string in the corpus with this and re-encodes it with Encode, and the bytes have to come back
// identical. A parser that quietly normalised something would show up there rather than as a
// contract failure three hundred cases later.
func Decode(text string) (Value, error) {
	parser := &parser{text: text}
	parser.skipWhitespace()
	value, err := parser.value()
	if err != nil {
		return Null(), err
	}
	parser.skipWhitespace()
	if parser.at != len(parser.text) {
		return Null(), parser.errorf("unexpected trailing input")
	}
	return value, nil
}

type parser struct {
	text string
	at   int
}

func (p *parser) errorf(format string, args ...any) error {
	return fmt.Errorf("canonical: %s at byte %d", fmt.Sprintf(format, args...), p.at)
}

// skipWhitespace steps over JSON's four insignificant characters.
//
// JSON's whitespace set is space, tab, CR and LF and nothing else — it is *not* the set
// `internal/js`.Trim removes, and it is not Unicode whitespace. Spelt out here so that neither of
// those two gets reached for by someone tidying up: this function is about the grammar of the
// file, and that one is about the semantics of a contract.
func (p *parser) skipWhitespace() {
	for p.at < len(p.text) {
		switch p.text[p.at] {
		case ' ', '\t', '\r', '\n':
			p.at++
		default:
			return
		}
	}
}

func (p *parser) value() (Value, error) {
	if p.at >= len(p.text) {
		return Null(), p.errorf("unexpected end of input")
	}
	switch ch := p.text[p.at]; {
	case ch == '{':
		return p.object()
	case ch == '[':
		return p.array()
	case ch == '"':
		text, err := p.string()
		if err != nil {
			return Null(), err
		}
		return String(text), nil
	case ch == 't':
		return p.literal("true", Bool(true))
	case ch == 'f':
		return p.literal("false", Bool(false))
	case ch == 'n':
		return p.literal("null", Null())
	case ch == '-' || (ch >= '0' && ch <= '9'):
		return p.number()
	default:
		return Null(), p.errorf("unexpected character %q", ch)
	}
}

func (p *parser) literal(word string, value Value) (Value, error) {
	if !strings.HasPrefix(p.text[p.at:], word) {
		return Null(), p.errorf("expected %s", word)
	}
	p.at += len(word)
	return value, nil
}

func (p *parser) object() (Value, error) {
	p.at++ // '{'
	out := Object()
	p.skipWhitespace()
	if p.at < len(p.text) && p.text[p.at] == '}' {
		p.at++
		return out, nil
	}
	for {
		p.skipWhitespace()
		if p.at >= len(p.text) || p.text[p.at] != '"' {
			return Null(), p.errorf("expected a key")
		}
		key, err := p.string()
		if err != nil {
			return Null(), err
		}
		p.skipWhitespace()
		if p.at >= len(p.text) || p.text[p.at] != ':' {
			return Null(), p.errorf("expected ':'")
		}
		p.at++
		p.skipWhitespace()
		value, err := p.value()
		if err != nil {
			return Null(), err
		}
		// A repeated key keeps its first position and takes its last value, which is what
		// JSON.parse does and therefore what the reference saw when it read the same bytes.
		out = out.Set(key, value)
		p.skipWhitespace()
		if p.at >= len(p.text) {
			return Null(), p.errorf("unterminated object")
		}
		if p.text[p.at] == ',' {
			p.at++
			continue
		}
		if p.text[p.at] == '}' {
			p.at++
			return out, nil
		}
		return Null(), p.errorf("expected ',' or '}'")
	}
}

func (p *parser) array() (Value, error) {
	p.at++ // '['
	out := Array()
	p.skipWhitespace()
	if p.at < len(p.text) && p.text[p.at] == ']' {
		p.at++
		return out, nil
	}
	for {
		p.skipWhitespace()
		value, err := p.value()
		if err != nil {
			return Null(), err
		}
		out = out.Push(value)
		p.skipWhitespace()
		if p.at >= len(p.text) {
			return Null(), p.errorf("unterminated array")
		}
		if p.text[p.at] == ',' {
			p.at++
			continue
		}
		if p.text[p.at] == ']' {
			p.at++
			return out, nil
		}
		return Null(), p.errorf("expected ',' or ']'")
	}
}

func (p *parser) string() (string, error) {
	p.at++ // '"'
	var out strings.Builder
	for {
		if p.at >= len(p.text) {
			return "", p.errorf("unterminated string")
		}
		ch := p.text[p.at]
		switch {
		case ch == '"':
			p.at++
			return out.String(), nil
		case ch == '\\':
			p.at++
			if err := p.escape(&out); err != nil {
				return "", err
			}
		case ch < 0x20:
			return "", p.errorf("unescaped control character")
		default:
			out.WriteByte(ch)
			p.at++
		}
	}
}

func (p *parser) escape(out *strings.Builder) error {
	if p.at >= len(p.text) {
		return p.errorf("unterminated escape")
	}
	ch := p.text[p.at]
	p.at++
	switch ch {
	case '"', '\\', '/':
		out.WriteByte(ch)
		return nil
	case 'b':
		out.WriteByte('\b')
		return nil
	case 'f':
		out.WriteByte('\f')
		return nil
	case 'n':
		out.WriteByte('\n')
		return nil
	case 'r':
		out.WriteByte('\r')
		return nil
	case 't':
		out.WriteByte('\t')
		return nil
	case 'u':
		return p.unicodeEscape(out)
	default:
		return p.errorf("unknown escape \\%c", ch)
	}
}

// unicodeEscape reads \uXXXX, pairing surrogates.
//
// A high surrogate followed by a low one is one code point and must be written as one UTF-8
// sequence; an unpaired surrogate is not a code point at all and is written as U+FFFD, which is
// what Go's UTF-8 encoder does with it and the only representable answer in a language whose
// strings are UTF-8 rather than UTF-16. The corpus holds no unpaired surrogate — this branch
// exists so that an input carrying one fails visibly at its own character rather than producing a
// string that differs from the reference's somewhere later.
func (p *parser) unicodeEscape(out *strings.Builder) error {
	first, err := p.hex4()
	if err != nil {
		return err
	}
	if utf16.IsSurrogate(rune(first)) && strings.HasPrefix(p.text[p.at:], "\\u") {
		mark := p.at
		p.at += 2
		second, err := p.hex4()
		if err != nil {
			return err
		}
		if combined := utf16.DecodeRune(rune(first), rune(second)); combined != utf8.RuneError {
			out.WriteRune(combined)
			return nil
		}
		// Not a pair after all: put the second escape back for the loop to read on its own.
		p.at = mark
	}
	out.WriteRune(rune(first))
	return nil
}

func (p *parser) hex4() (uint32, error) {
	if p.at+4 > len(p.text) {
		return 0, p.errorf("truncated \\u escape")
	}
	value, err := strconv.ParseUint(p.text[p.at:p.at+4], 16, 32)
	if err != nil {
		return 0, p.errorf("malformed \\u escape")
	}
	p.at += 4
	return uint32(value), nil
}

func (p *parser) number() (Value, error) {
	start := p.at
	if p.at < len(p.text) && p.text[p.at] == '-' {
		p.at++
	}
	for p.at < len(p.text) && p.text[p.at] >= '0' && p.text[p.at] <= '9' {
		p.at++
	}
	if p.at < len(p.text) && p.text[p.at] == '.' {
		p.at++
		for p.at < len(p.text) && p.text[p.at] >= '0' && p.text[p.at] <= '9' {
			p.at++
		}
	}
	if p.at < len(p.text) && (p.text[p.at] == 'e' || p.text[p.at] == 'E') {
		p.at++
		if p.at < len(p.text) && (p.text[p.at] == '+' || p.text[p.at] == '-') {
			p.at++
		}
		for p.at < len(p.text) && p.text[p.at] >= '0' && p.text[p.at] <= '9' {
			p.at++
		}
	}
	value, err := strconv.ParseFloat(p.text[start:p.at], 64)
	if err != nil {
		return Null(), p.errorf("malformed number %q", p.text[start:p.at])
	}
	return Number(value), nil
}
