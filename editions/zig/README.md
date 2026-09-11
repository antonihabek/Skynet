# The Zig edition

> **This edition is partial, and the runner is what says so.** §31.4: *"An edition that has not passed the
> corpus is not an edition."* This one has not, yet. The figure is deliberately not written here — the
> runner prints it on every run, and `reference/reports/editions.json` holds the figures for the editions
> that are complete, which this is not (finding 158: a README that states a passing number is a claim with
> nothing behind it).

§31.3 gives Zig a **Host** ceiling and a **Community** standing — *"genuine systems language with a native
C ABI story. No qualified toolchain exists, so it cannot reach the certified tier — a limitation of the
ecosystem, not the language."*

This is §42's **L2 — Contracts**: the contracts in `corpus/`, computed natively and held byte for byte to
the oracle. It is not a runtime (L3) and not the Safety Kernel (L4). The Host ceiling means a Zig edition
*could* one day host an enforcing kernel where the Guest-ceiling editions never can — and the reason it
does not today is a missing qualified toolchain, which is not something an edition can fix by being
written.

## Build and check

```bash
zig build check
```

`check` is five things, all blocking:

| Step | What it holds |
|---|---|
| `fmt` | `zig fmt` is not advisory here; a formatting diff fails the build |
| `round trip` | Every `expected` string in the corpus parses and re-serialises to identical bytes — the canonical writer, proven before anything is built on it |
| `self check` | Rule 3 at its edges; rules 2, 4 and 5 as facts; the documents the parser must refuse; the runner's four-bucket accounting; and 219 whole strings the reference's engine case-converted |
| `fusion` | Every source file compiled to assembly for the two architectures that fuse a multiply-add, and no fused instruction in any of them — with `src/fusion_probe.zig` required to fuse |
| `corpus` | Every case this edition answers, at the ratchet `-Dmin-passed` |

`ReleaseSafe` is the default rather than `Debug`, and that is a decision rather than a habit: this edition's
whole job is to produce bytes, and the bounds and overflow checks ReleaseSafe keeps are the ones that turn
a wrong index into a stop instead of a wrong document. `ReleaseFast` would remove them, and there is
nothing here whose speed is worth that.

No dependencies — `build.zig.zon` does not exist, because there is nothing to declare. §31.4 requires
byte-identical output, so the number-formatting rule has to be ours rather than whatever a dependency does
this year.

## What Zig's standard library gets right, and the one place that is a risk

**Rule 3 is nearly free here, and that is worth saying plainly rather than quietly relying on.**
`std.fmt.float.render` in `.decimal` mode is shortest-round-trip *and* positional: it writes `0.0000001`
where `JSON.stringify` writes `1e-7`, and `1000000000000000000000` where every C library reaches for an
exponent. The C edition beside this one implements that whole search by hand — a precision loop against
`strtod`, then an exponential-to-positional conversion — and here there is exactly one departure to make:

* **Negative zero.** `render` writes `-0`; rule 3 writes `0`.

The risk in that is not the departure, it is the dependency: an edition whose number formatting is a
library call is an edition whose number formatting changes when the library does. That is what the
`round trip` step is for — every number the reference ever wrote into `corpus/`, parsed and re-serialised
here, on every build.

## Comptime float arithmetic is f128, and a test written the obvious way asserts the wrong thing

Zig evaluates `comptime_float` arithmetic at **f128** precision. So `0.1 + 0.2` written as a literal is
`0.3` — not the `0.30000000000000004` that an `f64` addition produces at runtime, which is what the
reference computes and what the corpus holds.

This was found by writing the obvious probe and getting the wrong answer from a correct formatter. Every
numeric expectation in `src/selfcheck.zig` therefore goes through a runtime `f64` (`sink`) rather than a
literal expression, and the comment there says why. It is a specific instance of finding 170 — *a clean
run proves the bits on the machine that ran it* — with "the machine" being the compiler's constant folder.

## Contraction is off by default, and the scan is here anyway

Zig's default float mode is `.strict`, which forbids contracting `x*y + z` into a fused multiply-add. So
unlike the C and C++ editions there is no flag to set — and that is exactly why `tools/fusion-scan.sh`
exists rather than being omitted. A default is a thing that can change, and `@setFloatMode(.optimized)` is
one line in one file away from making this edition's arithmetic differ from the reference's on every
machine with an FMA. §31.2 pins the kernel's sine, cosine and square root as a fixed sequence of correctly
rounded operations; a fused operation is a different sequence.

The scan compiles every source file to assembly for aarch64 (FMA in its base instruction set) and x86-64
at v3, and fails on any fused instruction. `src/fusion_probe.zig` asks for `.optimized` on purpose and
**must** fuse, so a host where the scan cannot see an FMA fails rather than passing by not looking
(finding 171).

## How it was written

From the modules under `reference/typescript` only — never from `corpus/`, which would agree by
construction, and never from the Python, C++, Go or C editions, which would prove only that one hand did
both. One file under `src/contracts/` per module there, named the same. `src/contracts/dispatch.zig` is the
only file with no counterpart: the reference has no such table, because in TypeScript the corpus generator
calls the functions directly.

## What Zig cannot borrow from its standard library

`src/js.zig`. The near-equivalent is the trap in every row of this table — an edition built on it agrees on
every input anybody thought to test and diverges on one nobody did:

| The reference calls | Zig's near-equivalent | Why it is not the same |
|---|---|---|
| `trim()` | `std.mem.trim` with `&std.ascii.whitespace` | ECMAScript's whitespace set includes U+FEFF and U+00A0 and excludes U+0085. ASCII whitespace is none of those, and both directions appear in the corpus |
| `toUpperCase()` / `toLowerCase()` | `std.ascii.upperString` / `lowerString` | ASCII only, and one-to-one. A sharp s upper-cases to *two* letters, the Kelvin sign lower-cases to an ASCII `k`, and a capital sigma lower-cases to a different letter at the end of a word |
| `${value}` | `{d}` on an `f64` | Number::toString writes `1e-7` and `1e+21` where rule 3 writes them positionally, and writes `NaN` where rule 3 refuses it. Both renderings appear in frozen output, in different places |
| `Number(text)` | `std.fmt.parseFloat` | `Number("0x1f")` is 31 and carries **no sign** in the grammar, so `-0x10` is NaN; `Number("Infinity")` needs that capital; and `Number("")` is *zero* |
| a JSON parser | `std.json` | It would parse these documents, and then the number that came back would be whatever its own conversion produced and the duplicate-key rule would be whatever it chose this year |

The case tables come from `tools/unicode-case.ts`, shared with the C, C++ and Go editions: one derivation,
four renderings. The two generated files are **excluded from `zig fmt`**, because the formatter aligns an
array literal into columns whose width depends on the widest element in each run — which no generator can
predict without reimplementing the formatter. `node tools/unicode-case.ts --check` is their gate instead. A
generated file that is permanently one `zig fmt` away from failing a gate is worse than one the formatter
does not touch.
