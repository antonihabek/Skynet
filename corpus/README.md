# The conformance corpus

> **An edition that has not passed the corpus is not an edition.** It cannot carry the name, appear
> in the matrix, or be installed by `skynetd` — which is the same mechanism SCTS applies to robots,
> pointed at implementations. (§31.4)

Skynet OS ships as multiple language editions, and N implementations of a safety kernel is N sets of
latent bugs. Protocol Buffers has implementations in twenty languages, WebAssembly has a dozen
runtimes, SQL has hundreds of engines; none of them stays coherent by good intentions. They stay
coherent because **the specification and its conformance corpus are the artifact, and every
implementation is measured against it rather than against the others.**

This directory is that corpus.

## Format

Each file holds one corpus and a list of cases:

```json
{
  "corpus": "transport",
  "generated_by": "reference/typescript, via tools/corpus-gen/generate.ts",
  "cases": [
    {
      "id": "scaled-down",
      "note": "A policy trained at 180 Nm on an 80 Nm joint is scaled and the operator is told the figure.",
      "input": { "source": {}, "target": {}, "options": {} },
      "expected": "{\"bindings\":[...]}"
    }
  ]
}
```

Four properties, each load-bearing:

1. **`expected` is a string, not an object.** The requirement is byte-identical output, so the bytes
   are what the file holds. A reviewer sees exactly what an edition must emit, and a runner compares
   strings rather than walking two trees and deciding what "equal" means.
2. **`input` is a plain object**, because a human has to read it to judge whether the case tests what
   its note claims.
3. **Every case carries a note.** A vector whose purpose nobody recorded is a vector nobody dares
   change when it fails.
4. **Nothing is random.** The fuzz corpora use a seeded generator written out in full in
   `tools/corpus-gen/generate.ts`, so regenerating produces byte-identical files and a diff means a
   behaviour change rather than a reshuffle.

## What is here

| Corpus | Input | Expected output |
|---|---|---|
| `roles/` | Role strings, valid and malformed | Parse result or null |
| `transport/` | Source embodiment + descriptor + options | The exact `TransportPlan` |
| `cycle/` | Cycle vector + resolved limits + envelope + body | The exact `CycleVerdict` |
| `limits/` | Tier, envelope, transport clamp, profile, availability, frames | The exact `ResolvedLimits` |
| `frames/validate` | A §44.3 frame graph | The fault, or null when the tree is usable |
| `frames/bindings` | A frame graph **and the joint list of the body it came from** | The fault, or null when every name resolves |
| `kinematics/` | A frame graph + joint bindings + a command | Every frame's pose, to the last bit |
| `blackbox/` | Ring records, headers, MCAP schemas, a whole sealed file | The exact bytes, by digest |
| `admission/` | Package manifest + device capacity + flags | The exact finding list, in order |
| `certification/` | SCTS results + field conformance + bench age | The exact award |
| `readiness/` | Registry subject | Attainable tier and blocker list |
| `fuzz/` | Generated transport and cycle inputs | Their outputs |

`kinematics/` is the newest, and it exists because `cycle/` cannot do its job. A guard trip is a
boolean, and two editions could agree on every trip in the corpus while disagreeing about every pose
by a millimetre — right up to the day one of them clamps and the other does not. Freezing the poses
themselves is what makes the agreement mean something, and it is what holds the two editions' sine,
cosine and square root to each other. Those three are written out in
`crates/skynet-kernel/src/real.rs` and `reference/typescript/kinematics.ts` rather than taken from a
platform library, because no platform library is required to round the way another one does, and
§31.4 asks for identical bytes.

**A corpus can also fail by not looking, and this one did.** `proximity` and `mode_gate` were
implemented in the Rust edition and absent from the reference, and nothing caught it for four
sessions: the generator never produced a cycle carrying a separation distance or a capability token,
so every case compared two editions on inputs where both did nothing. Two of the eight guards were
pinned by one edition's own suite and by nothing else — which is the arrangement §31.4 exists to
prevent. Both fields are now in the reference's cycle vector, in curated cases and in the fuzz
generator, and six of the eight guards appear in `fuzz/cycle`'s verdicts. The lesson generalises: a
corpus grows a blind spot wherever an *input* field is missing, not only where an implementation is.

`limits/` is not one of the six §31.4 lists. Limit resolution is contract behaviour with a
serialisable output and three composition rules — the minimum rule, the widening refusal, the guard
partition — that would otherwise be pinned only indirectly, through the cycle vectors that consume
it. The corpus is required to grow with every feature, so it grew.

## Canonical JSON

`expected` is written in one canonical form, defined once and implemented once per edition:

1. No insignificant whitespace.
2. Object keys sorted by the byte sequence of their UTF-8 encoding.
3. Numbers finite. An integral value below 2^53 as a decimal integer, `-0` as `0`; anything else as
   the shortest decimal that round-trips, **always positional, never exponential**.
4. Strings escape `"`, `\` and the C0 controls and nothing else.
5. Absent fields are omitted; `null` is written. The two are different facts — the whole system
   distinguishes "nobody declared a safety envelope" from "the envelope is null".

`crates/skynet-json` and `reference/typescript/canonical.ts` are two implementations of that rule,
and the duplication is the point: a corpus whose expected values were produced by the same code that
checks them would prove nothing.

## Rules

* **A new case lands in the corpus before the fix lands in any edition.** This is what keeps the
  corpus ahead of the implementations rather than a retrospective description of them.
* **A divergence blocks the edition that diverged, never the others.** It is either a specification
  ambiguity or an implementation defect, and which of the two is a question about the specification
  rather than about the diff.
* **Channel maps are written with sorted keys.** The reference iterates insertion order, and an
  edition that parses into a sorted map iterates sorted order; both produce the same `command`
  object, but `clamped` is an *array* and its order follows the iteration. Sorting at generation
  removes the disagreement instead of papering over it.

## Regenerating

```bash
node tools/corpus-gen/generate.ts
```

```bash
cargo test -p skynet-corpus
```

A diff in `corpus/` after regenerating means the reference implementation's behaviour changed. That
is sometimes correct and never incidental — review it as the specification change it is.
