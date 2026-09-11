# The Go edition

> §31.4: *"An edition that has not passed the corpus is not an edition."* Whether this one has is
> answered by running it, and the answer is printed on every run and recorded in
> `reference/reports/editions.json` by `tools/edition-report.mjs`, which refuses any edition with a
> single case unanswered. The figure is deliberately not written here: a README that states a
> passing number is a claim with nothing behind it (finding 158 — the Python edition's drifted by 39
> cases and two corpora while the runner beside it printed the truth).

§31.3 gives Go a **Guest** ceiling and a **Full** standing — *"GC pauses. Excellent for
`skynetd`-equivalent fleet services off the robot, and for tooling."* This is §42's **L2 —
Contracts**: transport, admission, certification and readiness computed natively and held byte for
byte to the oracle. It is not a runtime (L3) and not a kernel (L4), and the ceiling is why it will
never be the latter — which the edition says rather than implies.

## Build and check

```bash
make -C editions/go check
```

`check` is six things, all blocking:

| Step | What it holds |
|---|---|
| `fmt` | `gofmt` is not advisory here; a formatting diff fails the build |
| `vet` | Go's own correctness checks, which are warnings nowhere else |
| `test` | 219 whole strings the reference's engine case-converted, and the case tables' ordering |
| `roundtrip` | Every `expected` string in the corpus parses and re-serialises to identical bytes — the canonical writer, proven before anything is built on it |
| `selfcheck` | A source scan refusing the standard-library spellings that are *nearly* right; the runner's four-bucket accounting over synthetic corpora; rule 3 of the canonical form at its edges; the compiler's own assembly on the six architectures that fuse a multiply-add (below) |
| `corpus` | Every case this edition answers, at the ratchet `MIN_PASSED` in the `Makefile` |

No dependencies, for the reason `Cargo.toml` gives for the Rust workspace having none: §31.4
requires byte-identical output, so the number-formatting rule has to be ours rather than whatever a
dependency does this year. `go build ./...` works with no network.

## How it was written

From `reference/typescript/*` only — never from `corpus/`, which would agree by construction, and
never from the Python or C++ editions, which would prove only that one hand did both. One file under
`internal/contracts/` per module there, named the same, so a reader holding the reference open can
find the Go beside it.

## What Go spells differently, and the gate that says so

The reason this edition needs an `internal/js` package at all is that Go's standard library has a
function for nearly everything the reference calls, and *nearly* is the problem: an edition built on
the near-equivalent agrees on every input anybody thought to test and diverges on one nobody did.

| The reference calls | Go's near-equivalent | Why it is not the same |
|---|---|---|
| `trim()` | `strings.TrimSpace` | Keeps U+FEFF and strips U+0085. ECMAScript does the opposite. |
| `\s` in a pattern | `\s` | Five characters in Go, twenty-five in ECMAScript |
| `toLowerCase()` | `strings.ToLower` | The simple mapping, and no final-sigma rule |
| `toUpperCase()` | `strings.ToUpper` | Leaves `ß` and `ﬁ` alone where the reference gives `SS` and `FI` |
| `Math.round` | `math.Round` | Rounds halves away from zero; `Math.round` rounds toward +∞ |
| `` `${n}` `` | `strconv.FormatFloat` | ECMAScript reaches for an exponent outside 1e-6..1e21; rule 3 never does |
| `JSON.parse` | `encoding/json` | Loses insertion order, and cannot tell an absent field from a null one |

Each of these has a right spelling in `internal/js` or `internal/canonical`, and `cmd/selfcheck`
refuses the wrong one by walking the AST — finding 159: *a rule written down in one module is not a
rule.* The exemption list names files rather than directories and its length is printed on every
run (§6.3).

Two of them are Go-specific rather than shared with the other editions, and both are about the
language's own conveniences:

**Map iteration is randomised.** The reference iterates objects in insertion order and several
contracts' output depends on it — `evaluateCycle` appends one entry to `clamped` per channel it
reduced, so that array's order is the document's. An edition built on `map[string]any` would emit
the same set of clamps in a different order *on every run*. `internal/canonical` keeps objects as a
slice of members and sorts only when writing.

**A field that is absent is not a field that is null.** The whole system distinguishes "nobody
declared a safety envelope" from "the envelope is null", and a map cannot hold that without a
sentinel that can leak into output. Here the absence is a `Set` that was never called, so rule 5 is
unrepresentable rather than enforced.

## The multiply-add the compiler is allowed to fuse

The third Go-specific rule, and the only one no corpus run on this repository's machines can see.
The Go specification permits an implementation to *"combine multiple floating-point operations into
a single fused operation, possibly across statements"*, and on arm64, riscv64, ppc64, s390x, loong64
and amd64 at `GOAMD64=v3` the compiler does: `x*y + z` becomes one instruction that does not round
`x*y`. §31.2 makes the kernel's sine, cosine and square root a fixed sequence of correctly rounded
operations so that every edition computes the same bits, and a fused operation is a different
sequence.

Measured rather than assumed, on 2026-09-11:

| | `x*y + z` | `p := x*y; p + z` |
|---|---|---|
| amd64, default level | not fused | not fused |
| arm64; amd64 at `GOAMD64=v3` | fused | fused |
| riscv64, ppc64le, s390x, loong64 | fused | not measured |

The second column is why this is a compiler-output check and not a source rule. The live defect it
found on its first run was neither spelling: `micros` handed `ms * 1000` to `roundHalfAway`, the
compiler inlined it, and on arm64, ppc64le and loong64 computed the fraction from the *unrounded*
product — exactly the half-way points that function exists to decide — while every corpus run on
amd64 passed.

The specification's remedy is an explicit conversion, which *"rounds to the precision of the target
type, preventing fusion"*: `float64(x*y)` where a product meets a sum, or `v = float64(v)` where a
helper receives one. `cmd/selfcheck` compiles the edition for all six targets with `-gcflags=-S` and
fails on any fused instruction in it, and `internal/fusionprobe` fuses on purpose so the scan can
prove, per target, that it is able to see one.

The C++ edition has the same exposure through GCC's default `-ffp-contract=fast` and the same gate in
its Makefile. Rust, Python and the TypeScript oracle cannot contract.

## Two functions the reference names alike

`observation.ts` and `kinematics.ts` each have a `roundHalfAway`, and they are different functions:
one truncates and then tests the fraction, the other adds a half and then truncates, and at
0.49999999999999994 they return 0 and 1. This edition keeps them as `roundHalfAway` and
`quadrantRound`, each in the file its reference lives in. Merging them would be a bug that passes
every case the corpus holds today.

## The generated file

`internal/js/unicode_case.go` and `internal/js/case_vectors_test.go` are the full
`toUpperCase`/`toLowerCase` mappings, the two properties the final-sigma rule reads, and strings the
engine converted whole — **read out of the engine the reference runs on** by `tools/unicode-case.ts`,
the same generator the C++ edition's tables come from. One derivation, two renderings: copying it
would have put the derivation in two places, to agree on the day they were written and diverge at
the next Unicode version.

```bash
node tools/unicode-case.ts
```

The files record their Unicode version, and CI checks them under the Node that produced them
(`case-tables`), because Node's Unicode version decides what the generator writes.
