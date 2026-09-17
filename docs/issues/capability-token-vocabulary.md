# Proposed amendment — §8 gains the token vocabulary it is said to have

**Status: an issue, not a change.** See [`README.md`](README.md).

It is `DRY-COVERAGE-PLAN.md` Phase 4's third item — *"Capability token vocabulary and `extensions`.
Pure contract, pure corpus"* — and the contract half cannot be written, because the vocabulary it
would enforce does not exist.

## The gap

§5.0 ends with this sentence:

> The token vocabulary is §8's, and `extensions` carries the ones nobody standardised — the same
> extensibility rule §5.1 applies to roles, for the same reason: a vendor token must be expressible
> without being mistaken for a canonical one.

**§8 has no vocabulary.** It defines the permission model completely — what a package declares, what
it requests, what an operator grants, and the two rules that keep it from decaying into *grant
everything* — and never says what a token *is*. The document names six in passing, in five different
sections: `manipulation.grasp` (§8.2), `hri.warning` (§49.1, reserved), `sensing.audio` (§57),
and `arm.left`, `sensing.camera`, `locomotion.base` (§54.2's examples). Nothing collects them and
nothing is normative.

§5.1 does this properly for roles one section earlier: a grammar, a closed set of chains, a closed
set of axes, and an explicit extension channel. Capability tokens have the extension channel —
`extensions` on the descriptor — and neither of the other two.

## What it costs today

A typo is indistinguishable from a body that does not have the capability.

A package requesting `sensing.camers` on a robot that declares `sensing.camera` is refused with
`capability_ungranted`, which is a true sentence about a token nobody has and a useless one about a
typo. §64's rule is that every refusal names the fix, and the fix here is *"there is no such
capability"* — which no component can say, because no component knows which capabilities there are.

The same hole makes `embodiment.capabilities.declared` — §15's thirty-sixth test, which measures
*"every token a resident package requires appears in the list"* — a check on set membership with no
set behind it.

## The amendment

### 8.5 The capability token vocabulary

A capability token is `<domain>.<capability>`: two segments, lowercase ASCII with underscores, no
further dots. The grammar is checkable on its own and is not the substance; the closed **domain** set
is.

| Domain | What a token in it governs |
|---|---|
| `manipulation` | commanding an arm, a gripper or a tool |
| `locomotion` | commanding a base, legs or wheels |
| `sensing` | reading a sensor the body declares |
| `hri` | anything a person perceives — a tone, a light, speech |
| `compute` | an accelerator, a model runtime, a share of the machine |
| `network` | reaching anything off the robot |
| `storage` | writing anywhere the package does not own |

The canonical tokens are enumerated per domain, as §5.1 enumerates chains and axes. A token whose
domain is canonical and whose capability is not is **refused by name** — `capability_unknown`, a new
rejection code — rather than silently never matching. A token whose domain is *not* canonical belongs
in `extensions` and is refused in `capabilities` for §5.1's reason: a vendor token must be
expressible without being mistaken for a canonical one.

**Reserved tokens keep their own list.** `hri.warning` is already reserved by §49.1 and refused by
`CapabilityReserved`; nothing here changes that, and the two lists answer different questions —
*is this a token* and *may anybody be granted it*.

### What this does not change

* **The permission model.** §8.2's two rules are untouched: `requests` is a subset of declared
  capabilities, and ungranted is a warning rather than an error.
* **`extensions`.** It carries exactly what it carries now, and gains a job it cannot do today:
  being the place a token goes when it is *not* one of these.

## What ratifying it costs, in order

1. `reference/typescript/package.ts` — the domains, the tokens, the grammar, and
   `capability_unknown` in the rejection codes.
2. `tools/corpus-gen/generate.ts` — cases into `corpus/admission/` and a new closed set in
   `corpus/vocabulary/closed-sets.json`. Every case added moves §73's corpus count.
3. The four complete editions.
4. `skynet skill check` and `skynet driver check` report an unknown token by name.
