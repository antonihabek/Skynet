# The reference edition

This is the TypeScript the Hub runs, vendored here with one job changed.

Part I of the specification implemented the contracts — transport, admission, certification,
readiness, the safety kernel — in TypeScript, and argued that running the same function on the Hub
and on the robot is what makes the two unable to disagree. §23.2 is the correction: **a robot cannot
run TypeScript**, so as written the design implied a second implementation on the robot, and two
implementations of "will this policy destroy this arm" can drift. Which is the exact failure the
shared-function argument was introduced to prevent.

So the TypeScript is kept and its role changes. It is now:

* **the reference** — where the specification is ambiguous and two editions disagree, this is what
  the specification is amended to say;
* **the differential-test oracle** — `tools/corpus-gen/generate.ts` runs curated inputs through
  these modules and freezes the canonical bytes into `corpus/`, and every edition is then held to
  those bytes.

Keeping it is not sentiment. A specification written in a language the domain experts read, tested
against the implementation, is worth more than a comment block — and it is what makes
`safety.kernel.parity` mean something.

## Files

| | |
|---|---|
| `embodiment.ts` | The descriptor, the role vocabulary, and `resolveTransport` |
| `safety.ts` | Tiers, the eight guards, `resolveLimits`, `evaluateCycle` |
| `package.ts` | The `.sky` manifest and `admitPackage` |
| `conformance.ts` | SCTS and `awardCertification` |
| `readiness.ts` | `evaluateReadiness` and `summarizeReadiness` |
| `types.ts` | `SafetyEnvelope`, restated so the oracle is standalone |
| `canonical.ts` | Canonical JSON — a deliberate reimplementation of `crates/skynet-json` |
| `shape.ts` | The wire shape of every output: which fields are nullable and which are optional |

The first five are the Hub's `lib/skynet/os/*.ts`, changed only in their import paths. That is
deliberate: a vendored oracle that has been tidied is an oracle that no longer describes the thing
it is the reference for. When the Hub's copy changes, this copy is updated and the corpus is
regenerated, and the diff in `corpus/` is the review.

## Running it

Node 23 and later strip types natively, so there is no build step and no `node_modules`:

```bash
node tools/corpus-gen/generate.ts
```

An oracle that needed a Next.js application checked out beside it is an oracle nobody runs, which is
why `types.ts` restates the one type these modules import from outside themselves.

## Two things this edition cannot do

Its tier ceiling is **off-robot** (§31.3). It is the Hub, the workstation CLI, and the WASM host for
the contracts; it never runs on a robot, so the question of whether it could host an enforcing
kernel does not arise.

And its `CommandCycle` carries no capability token and no separation distance, so the mode gate and
the proximity guard cannot be exercised from a corpus case. The Rust edition implements both and
covers them in its own suite; until the reference grows the fields, a corpus case that set them
would be comparing against an expectation nobody computed.
