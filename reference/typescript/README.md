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
| `digest.ts` | `versionDigest` — the identity a package is verified against at install |
| `types.ts` | `SafetyEnvelope`, restated so the oracle is standalone |
| `canonical.ts` | Canonical JSON — a deliberate reimplementation of `crates/skynet-json` |
| `kinematics.ts` | Forward kinematics, the specified sine/cosine/square root, and the pose-guard geometry |
| `shape.ts` | The wire shape of every output: which fields are nullable and which are optional |
| `predicate.ts` | The predicate language, and §54.2's three-valued precondition outcome |
| `semver.ts` | Version parsing, ordering, and range satisfaction |
| `skill.ts` | §54: preconditions, success judged by the OS, the timeout, and composition |
| `cell.ts` | §55.2: the cell descriptor, its `env.*` facts and its workspace layer |

`conformance.ts`, `package.ts`, `readiness.ts`, `digest.ts`, `canonical.ts`, `predicate.ts` and
`semver.ts` are the Hub's `lib/skynet/os/*.ts`, changed only in their import paths. `blackbox.ts` and `kinematics.ts` never
were: the Hub writes no black boxes and computes no forward kinematics, so those two are second
implementations written here against the same specification the Rust is written against.

**`embodiment.ts` and `safety.ts` are no longer byte-for-byte, and this file used to claim they
were.** The frame graph of §44.3, the geometry it carries and the two pose guards of §44.5 were
built in this repository, and the Hub has no consumer for it — it plans transports and awards
tiers; it does not compute where an arm is, and it holds no `frames` to compute one from. So the two files are ahead here by that work and the
divergence is deliberate. It is recorded rather than hidden because the sentence above it was
false for two sessions, and a stale claim of identity is worse than an admitted difference: it is
the reason nobody re-checks.

**`predicate.ts` and `semver.ts` came the other way**, and they are the first two that did. They
were the Hub's — `lib/skynet/compatibility/predicate.ts` and `lib/skynet/semver.ts`, written for
compatibility rules long before Part VI existed. §54.2 makes a skill's `preconditions` predicates
the **robot** evaluates before the skill may start, and `semver_satisfies` is one of the operators,
so both became contracts and moved here. The Hub's two old paths are now re-exports, and the
modules themselves are mirrored like every other contract. Two things changed on the way, both
described in `predicate.ts`'s own header: the boolean answer is now derived from a three-valued
outcome, so a rule the engine cannot evaluate no longer *passes*; and the engine is total for real,
where two malformed shapes used to throw.

Keeping the rest vendored byte-for-byte is deliberate: an oracle that has been tidied is an oracle
that no longer describes the thing it is the reference for. When the Hub's copy changes, this copy
is updated and the corpus is regenerated, and the diff in `corpus/` is the review. When a change
starts here — as the SCTS expansion of §52 did — it is carried the other way by hand and recorded
in `PROGRESS.md`, which is the same rule `docs/README.md` applies to the shared documents.

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

And it never runs a control loop, so what `evaluateCycle` here proves is that the arithmetic agrees
— not that it agrees inside a millisecond.

Its `CommandCycle` used to carry no capability token and no separation distance, which meant the
mode gate and the proximity guard could not be exercised from a corpus case at all. Both fields are
now here, along with the two pose guards of §44 and the `kinematics.ts` that computes their input;
all eight guards are implemented in both editions and six of the eight appear in the fuzz corpus.
`balance` is the one guard neither edition implements — there is no centre-of-pressure input in
either `CommandCycle` — and it reports itself unavailable rather than passing.
