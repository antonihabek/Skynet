# The C edition

> **This edition is partial, and the runner is what says so.** §31.4: *"An edition that has not passed
> the corpus is not an edition. It cannot carry the name, appear in the matrix, or be installed by
> `skynetd`."* This one has not, yet. The figure is deliberately not written here — the runner prints
> it on every run, and `reference/reports/editions.json` holds the figures for the editions that are
> complete, which this is not (finding 158: a README that states a passing number is a claim with
> nothing behind it, and the Python edition's drifted by 39 cases and two corpora while the runner
> beside it printed the truth).

§31.3 gives C a **Certified** ceiling and a **Full** standing — *"MISRA C is the most trodden
certification path in the industry and the smallest MCU footprint. Strongest candidate for the
safety-MCU build specifically."*

This is §42's **L2 — Contracts**: the contracts in `corpus/`, computed natively and held byte for byte
to the oracle. It is **not** a runtime (L3) and **not** the Safety Kernel (L4), and the distinction is
the whole reason this edition exists before that one. §42 says a second L4 edition *"roughly doubles
the certification bill and should be driven by a named customer's safety case rather than by
preference"* — that decision is the owner's. What an L2 C edition does is establish that the contracts
compute identically in C, which is the thing that decision would be made on top of, at none of its
cost.

## Build and check

```bash
make -C editions/c check
```

`check` is four things, all blocking:

| Step | What it holds |
|---|---|
| `round trip` | Every `expected` string in the corpus parses and re-serialises to identical bytes — the canonical writer, proven before anything is built on it |
| `self check` | Rule 3 at its edges; rules 2, 4 and 5 as facts rather than consequences; the documents the parser must refuse; the runner's four-bucket accounting over synthetic corpora; and 219 whole strings the reference's engine case-converted |
| `fusion` | Every translation unit compiled to assembly with a fused multiply-add available, and no fused instruction in any of them — with `tests/fusion_probe.c` required to fuse, so the scan proves it can see one |
| the corpus | Every case this edition answers, at the ratchet `MIN_PASSED` in the `Makefile` |

Warnings are errors (`-Wall -Wextra -Wpedantic -Wconversion -Wshadow -Werror`, plus
`-Wstrict-prototypes -Wmissing-prototypes -Wold-style-definition -Wvla`), objects build in parallel
with compiler-recorded header dependencies, and the runner links statically so it runs wherever it is
copied. The three warnings C needs that C++ does not are there because a function declared without a
prototype, defined without one, or defined in the old style is how a C translation unit calls something
with the wrong arguments and links anyway.

POSIX is asked for by name — `-D_POSIX_C_SOURCE=200809L` — rather than by relaxing the standard to
`gnu17`, so the only extension this edition uses is the one it requested. `dirent.h` and `getcwd` are
the whole of it.

## Floating-point contraction is off, and C is where that is least obvious

`FP_FLAGS := -ffp-contract=off`, on every compile, in a variable of its own so an override of `CFLAGS`
cannot drop it. §31.2 pins the kernel's sine, cosine and square root as a sequence of correctly rounded
operations so that every edition computes the same bits, and a fused multiply-add is a different
sequence: it computes `x*y + z` without rounding `x*y`.

The C++ edition's version of this paragraph says GCC's default for C++ is `-ffp-contract=fast`. C is
not the same story, and the difference is worse:

* GCC's default under `-std=c17` is **`-ffp-contract=on`**.
* **GCC 13 implements `on` as `off`.** So a C edition built with these flags on GCC 13 does not
  contract, whatever the Makefile says, and removing `FP_FLAGS` would change nothing measurable.
* **GCC 14 implements `on`** as contraction within an expression, and does fuse.

So the same source, the same flags, two compilers, two sets of bits — and the version that does not
fuse is the one a reviewer is most likely to be holding. That is exactly the shape of finding 170: *a
clean corpus run proves the bits on the machine that ran it.* The `fusion` step is what makes the flag
a gate rather than a line somebody could remove, and `tests/fusion_probe.c` is compiled with
`-ffp-contract=fast` — the only place in the Makefile that asks for a fusion — because at the default
it would not fuse on GCC 13 and the scan would report that it could not see one (finding 171).

## Memory is an arena, and no contract contains a `free`

One region per corpus case, reset after it. This is the one allocation decision an L2 edition can make
in the direction of the L4 one its row exists for: the build that ceiling is for is a MISRA C safety-MCU
with no dynamic allocation after init, and a region is the closest honest analogue on a host.

It is also what makes the contract code readable. The reference is TypeScript, where every intermediate
object is garbage; transcribing that into C with per-object ownership would put a lifetime decision in
every line of a file whose job is to compute the same bytes.

`sk_arena_alloc` **never returns NULL** — it aborts, naming the request it could not serve. That
invariant is written down in `include/skynet_os/arena.h` rather than left implicit, because it is the
one that would bite: the alternative is a NULL check at several thousand construction sites, which in
practice means a missing one somewhere, and a contract that reads a NULL value computes a wrong answer
instead of stopping. A runner that dies saying it is out of memory has told the truth; one that emits a
plausible document has not.

## Strings carry a length

A JSON string may hold U+0000, and rule 4 writes it as an escape rather than dropping it. A value model
built on NUL-terminated C strings would truncate exactly there and pass every other case in the corpus,
so `sk_value`'s strings are bytes plus a length, and the NUL written past the end is a convenience for
`printf` rather than a terminator anything relies on.

## How it was written

From the modules under `reference/typescript` only — never from `corpus/`, which would agree by
construction, and never from the Python, C++ or Go editions, which would prove only that one hand did
both. One file under `src/contracts/` per module there, named the same, so a reader holding the
reference open can find the C beside it. `src/contracts/dispatch.c` is the only file with no
counterpart: the reference has no such table, because in TypeScript the corpus generator calls the
functions directly.

## What C cannot borrow from its standard library

`src/js.c`, and it is the largest piece of this edition that is not a contract. Three of the
reference's most ordinary operations have no C equivalent, and using the near-equivalent would produce
an edition that agrees on every input anybody thought to test:

| The reference calls | C's near-equivalent | Why it is not the same |
|---|---|---|
| `trim()` | `isspace` | ECMAScript's whitespace set includes U+FEFF and excludes U+0085; Unicode's `White_Space` is the other way round on both. The semver corpus holds a range written with a byte-order mark before the version, and the predicate corpus holds a string that `trim()` blanks and nothing else does |
| `toUpperCase()` / `toLowerCase()` | `toupper` / `tolower` | One byte, one-to-one, in one locale. A sharp s upper-cases to *two* letters, the Kelvin sign lower-cases to an ASCII `k`, and a capital sigma lower-cases to a different letter at the end of a word |
| `${value}` and `String(x)` | `printf("%g")` | ECMAScript's Number::toString writes `1e-7` and `1e+21` where canonical form writes them positionally, and writes `NaN` where canonical form refuses it. Both renderings appear in frozen output, in different places |
| `Number(text)` | `strtod` | `Number("0x1f")` is 31, `Number("Infinity")` is infinite with that capital and no other, and `Number("")` is **zero**. `strtod` accepts `inf`, `nan` and a hex float, and the grammars differ on a sign |

The case tables come from `tools/unicode-case.ts`, which is shared with the C++ and Go editions: one
derivation, three renderings, because a second copy of it would agree on the day it was written and
diverge at the next Unicode version. Two things about the C rendering are C's alone:

1. **C forbids a universal character name below U+00A0.** The C++ vectors spell every code point
   `\U0000XXXX`; as C, every vector carrying a Latin letter is rejected outright. The C target emits
   hex escapes, which have no such rule.
2. **A C comment cannot contain the two characters that open one.** `-Werror=comment` refuses a
   sentence naming `reference/typescript` with a wildcard after it, which is the compiler doing exactly
   what §6.1 of the coding standard ranks it second for.

## Where it is

`src/contracts/dispatch.c` is the inventory: a corpus file in that table is answered, and one absent
from it is counted as *not implemented* and printed separately from the failures. The runner's summary
is the honest sentence, and none of its four numbers is useful without the others.
