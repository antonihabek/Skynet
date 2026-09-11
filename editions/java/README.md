# The Java edition

> **This edition is partial, and the runner is what says so.** §31.4: *"An edition that has not passed the
> corpus is not an edition."* This one has not, yet. The figure is deliberately not written here — the runner
> prints it on every run, and `reference/reports/editions.json` holds the figures for the editions that are
> complete, which this is not (finding 158).

§31.3 gives Java / Kotlin a **Guest** ceiling and a **Community** standing — *"GC. Industrial automation shops
with existing JVM stacks."* This is §42's **L2 — Contracts**.

The Guest ceiling is a property of a garbage collector, not a judgement about Java: a collector cannot promise a
clamp inside a 1 ms deadline, so this edition is an **observer**. It computes the same verdicts as the Rust
edition and enforces none of them.

## Build and check

```bash
make -C editions/java check
```

| Step | What it holds |
|---|---|
| `round trip` | Every `expected` string in the corpus parses and re-serialises to identical bytes |
| `self check` | 101 checks: rule 3 at its edges, rules 2, 4 and 5 as facts, the documents the parser must refuse, the runner's four-bucket accounting, and 219 whole strings the reference's engine case-converted |
| `corpus` | Every case this edition answers, at the ratchet `MIN_PASSED` in the `Makefile` |

`-Xlint:all -Werror`, and it earned that on its first run by refusing a missing `serialVersionUID` — a warning
nobody would have read. No Maven, no Gradle, no `pom.xml`: §31.4 requires byte-identical output, so the number
conversion and the duplicate-key rule have to be ours, and a build that needs a package index is one that
cannot run on the machine an edition is being reviewed on.

**There is no fused multiply-add scan, and that is the platform rather than an omission.** The JVM specification
defines `dmul` and `dadd` as separate IEEE-754 binary64 operations with no licence to contract them, and
`strictfp` became the only behaviour in Java 17. So unlike the C, C++ and Zig editions there is no flag to set
and no compiler default to distrust.

## Java is the closest edition to the reference, which is what makes it dangerous

A `String` here is UTF-16, exactly as a JavaScript string is. Every other edition has had to decide how to
represent the reference's strings and its case mappings; this one does not. That removes a whole class of work
and puts something worse in its place: the places where Java *looks* like the reference and is not.

### Rule 3 is not `Double.toString`, and the counter-example is a number in the corpus's own domain

`Double.toString` gives shortest-round-trip **digits** for almost everything, which is exactly why this matters:
an edition built on it passes thousands of cases and fails these.

| Input | `Double.toString` | Rule 3 |
|---|---|---|
| `Double.MIN_VALUE` | `4.9E-324` | `0.000…0005` — **one** significant digit |
| `1e-7` | `1.0E-7` | `0.0000001` |
| `1e21` | `1.0E21` | `1000000000000000000000` |
| `1.0` | `1.0` | `1` |
| `-0.0` | `-0.0` | `0` |

The first row is the one that decides the approach rather than the formatting. `Double.parseDouble("5E-324")`
**is** `Double.MIN_VALUE`, so the shortest decimal that round-trips has one digit and the library prints two.
Both halves of that are asserted in `SelfCheck`, so a reader who wonders why this edition runs a precision loop
finds the answer in a failing test rather than in a comment.

### Rule 2 is not `String.compareTo`

Keys sort by their UTF-8 **bytes**. `compareTo` compares UTF-16 code units, and the two disagree above the basic
multilingual plane: an astral character is a surrogate pair, and a surrogate code unit (0xD800…) sorts *below*
U+E000…U+FFFF while its UTF-8 encoding (0xF0…) sorts *above*. So `compareTo` puts an astral key first and rule 2
puts it last. `SelfCheck` asserts both orders on the same pair of keys.

### `trim()` is neither `String.trim()` nor `String.strip()`

`trim()` removes anything at or below U+0020 and nothing above it, so U+00A0 and U+FEFF survive it. `strip()`
uses `Character.isWhitespace`, which *excludes* U+00A0 and U+FEFF and *includes* U+001C–001F. ECMAScript's set is
neither, and the corpus turns on both directions: a semver range written with a byte-order mark before the
version, and a predicate case whose string is blank to `trim()` and not to anything else.

### `toUpperCase()` is right about the mappings and wrong about the version

Java's is one of the few standard libraries that does apply the full mappings *and* the final-sigma rule. It is
still not used, because the Unicode version it applies them under is the JDK's. An edition calling it would
agree with the reference on every character in whichever version the two happened to share and diverge on the
ones added between them — a divergence that appears when somebody upgrades a JDK, in a build nobody changed. So
the tables come from `tools/unicode-case.ts`, read out of the engine the reference runs on, like every other
edition's.

### `Number(text)` is not `Double.parseDouble`

`Number("0x1f")` is 31 and carries **no sign** in the grammar, so `-0x10` is NaN. `Number("Infinity")` needs
that capital. `Number("")` is *zero*. And `parseDouble` accepts `1d`, `0x1p3` and a trailing `f`, none of which
is a JSON number — which is why the parser validates the run of bytes before handing it over, and why three of
those spellings are in the list of documents `SelfCheck` requires it to refuse.

## Two things javac decides that no other edition's compiler does

1. **A unicode escape is processed before the lexer runs, including inside a comment.** A comment containing the
   four characters that spell a unicode escape does not compile — `error: illegal unicode escape` — and the fix
   is to double the backslash, because a backslash is only eligible when preceded by an even number of them.
   This is the same shape as the C edition's discovery that a C comment cannot contain the two characters that
   open one.
2. **A static array initialiser is the class initialiser, and javac limits a method to 64 KiB of bytecode.** The
   generated case tables are `error: code too large` as array literals. `tools/unicode-case.ts` therefore emits
   them as numbered fill methods, one class per table — the alternative being a packed string literal, which has
   no size limit and no diff.

## Encoding is set explicitly on every stream

JDK 18 made `file.encoding` UTF-8 by default and left the console streams on the platform's native encoding. This
edition prints its report to stdout for `tools/edition-report.mjs` to parse, so the `Makefile` passes
`-Dstdout.encoding=UTF-8` and `-Dstderr.encoding=UTF-8`: a figure the Hub publishes must not depend on the locale
of the machine that measured it.

## How it was written

From the modules under `reference/typescript` only — never from `corpus/`, which would agree by construction, and
never from the Python, C++, Go, C or Zig editions, which would prove only that one hand did both. One class under
`src/skynet/os/contracts/` per module there, named for it. `Dispatch` is the only one with no counterpart: the
reference has no such table, because in TypeScript the corpus generator calls the functions directly.
