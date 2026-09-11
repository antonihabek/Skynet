# The Go edition

> **In progress.** It is not an edition yet. §31.4: *"An edition that has not passed the corpus is
> not an edition — it cannot carry the name, appear in the matrix, or be installed by `skynetd`."*
> This one answers some of the corpus and says so in a number the runner prints on every run. The
> figure is deliberately not written here, and neither is the one it will eventually reach: a README
> that states a passing number is a claim with nothing behind it (finding 158 — the Python
> edition's drifted by 39 cases and two corpora while the runner beside it printed the truth).
>
> `reference/reports/editions.json` lists the editions that have passed. This one is absent from it
> on purpose, and `tools/edition-report.mjs` refuses any edition with a non-zero unimplemented
> count rather than recording that it half passes.

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
| `selfcheck` | A source scan refusing the standard-library spellings that are *nearly* right; the runner's four-bucket accounting over synthetic corpora; rule 3 of the canonical form at its edges |
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
