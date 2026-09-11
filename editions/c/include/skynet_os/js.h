/*
 * The JavaScript semantics the reference relies on, spelled out.
 *
 * The reference is TypeScript, and a transcription of it into C is only as faithful as its account of
 * what the engine does. Three of those are not what a C standard library does:
 *
 * * **`trim()` trims Unicode whitespace**, not `isspace`. The semver corpus holds a range written
 *   with a U+FEFF before the version, and JavaScript trims it; `isspace` in the C locale does not, so
 *   the range would be unreadable here and readable in the reference.
 * * **`toUpperCase()` and `toLowerCase()` are full mappings.** A sharp s upper-cases to two letters,
 *   the Kelvin sign lower-cases to an ASCII `k`, and a capital sigma lower-cases to a *different*
 *   letter at the end of a word. `toupper` is one byte, one-to-one, in one locale. The tables come
 *   from `tools/unicode-case.ts`, read out of the engine the reference runs on rather than out of the
 *   Unicode data files, because what is being transcribed is what that engine does.
 * * **`${value}` is not the canonical writer.** The reference builds messages that end up in frozen
 *   output — `Frame ${node.name} has kind ${node.kind}` — and those go through ECMAScript's ToString:
 *   `undefined` is the word, a number goes through Number::toString (which writes `1e-7`, where the
 *   canonical writer writes `0.0000001`), an array is its elements joined with commas, and an object
 *   is `[object Object]`.
 *
 * Every string here is UTF-8, which is what the value model holds and what the corpus is written in.
 */
#ifndef SKYNET_OS_JS_H
#define SKYNET_OS_JS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "skynet_os/arena.h"
#include "skynet_os/value.h"

/* The Unicode version the tables were generated under, for the self-check to compare against the
 * vectors' — two files generated under different versions would disagree about characters neither of
 * them is wrong about. */
const char *sk_js_unicode_version(void);

/* ── UTF-8 ─────────────────────────────────────────────────────────────────── */

/*
 * Decode the code point at `at`, advancing `at` past it.
 *
 * A byte that is not part of a well-formed sequence is returned as U+FFFD and consumed one byte at a
 * time, which is what a JavaScript string holding lone surrogates becomes when it is encoded. No
 * corpus document contains one; the behaviour is defined rather than left to run off the end.
 */
uint32_t sk_utf8_next(const char *text, size_t len, size_t *at);
/* Encode one code point, returning the number of bytes written (at most four). */
size_t sk_utf8_encode(uint32_t code, char *out);

/* ── Whitespace ────────────────────────────────────────────────────────────── */

/* ECMAScript's WhiteSpace and LineTerminator, which is what `trim()` removes. */
bool sk_js_is_space(uint32_t code);
const char *sk_js_trim(sk_arena *arena, const char *text, size_t len, size_t *out_len);

/* ── Case ──────────────────────────────────────────────────────────────────── */

/* True when both mapping tables are ascending, which the lookups binary-search. Asserted by the
 * self-check rather than trusted: a table that lost its order would still answer most lookups. */
bool sk_js_tables_sorted(void);

const char *sk_js_upper(sk_arena *arena, const char *text, size_t len, size_t *out_len);
/* Lower-casing applies the one context rule `toLowerCase` has outside the Turkish and Lithuanian
 * tailorings: a capital sigma is final when a cased letter precedes it and none follows, skipping
 * case-ignorable characters in both directions. */
const char *sk_js_lower(sk_arena *arena, const char *text, size_t len, size_t *out_len);

/* ── Numbers and ToString ──────────────────────────────────────────────────── */

/*
 * ECMAScript's Number::toString at radix 10 — `String(value)`, not the canonical writer.
 *
 * The two differ in both directions and the difference is visible in frozen output: this writes
 * `1e-7` and `1e+21` where canonical form writes them positionally, and it writes `NaN` and
 * `Infinity` where the canonical writer refuses them.
 */
const char *sk_js_number_to_string(sk_arena *arena, double number);

/*
 * ECMAScript's ToNumber over a string — `Number(text)`, which is not `strtod`.
 *
 * Three differences decide corpus cases. `Number("0x1f")` is 31, because a string may hold a
 * non-decimal integer literal; `strtod` accepts hex too but with a different grammar and an optional
 * sign, where ECMAScript allows no sign on one. `Number("Infinity")` is infinite, spelled with that
 * capital and no other. And `Number("")` is **zero**, not a failure — the reference's `toNumber`
 * guards that with its own `trim() !== ''` test rather than relying on the conversion.
 *
 * Returns NaN for a string that is not a numeric literal, which is what the engine returns.
 */
double sk_js_string_to_number(sk_arena *arena, const char *text, size_t len);

/*
 * ECMAScript's ToString over a parsed JSON value — what a template literal does to whatever it is
 * handed.
 *
 * `present` is false for a field that was absent, which is JavaScript's `undefined` and renders as
 * the word — a different rendering from a field that was present and null, exactly as rule 5 says
 * they are different facts. Pass the value NULL and `present` false for an absent field, which is
 * what `sk_get` returns for one.
 */
const char *sk_js_to_string(sk_arena *arena, const sk_value *value, bool present);

#endif /* SKYNET_OS_JS_H */
