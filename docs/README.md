# Specification

Four documents. Three of them are **mirrors** of files that also live in the BotMarket repository,
and one is written here.

| File | Origin | Rule |
|---|---|---|
| `skynet-os-architecture.md` | BotMarket `docs/` | Mirror — must stay byte-identical |
| `skynet-architecture.md` | BotMarket `docs/` | Mirror — must stay byte-identical |
| `skynet-build-plan.md` | BotMarket repository root | Mirror — must stay byte-identical |
| `../PROGRESS.md` | Here | The build log for this repository |

## The mirroring rule

> **A shared document is edited in both repositories or in neither. It never diverges.**

The two repositories are separate for good reasons (§72.1: licensing history, vendor readability,
toolchain overlap, release cadence) and the specification is not one of the things that should be
split by that boundary. The Hub is built against this design and so is the OS; two copies drifting
apart would mean two teams building against two designs while both believed they were reading the
same one.

So when a shared document changes:

1. Make the edit.
2. Copy the file to the other repository.
3. Verify byte equality before considering it done.

`tools/check-mirrors.mjs` does step 3, and exits non-zero on divergence so it can be a pre-commit
hook:

```bash
node tools/check-mirrors.mjs
```

It looks for BotMarket beside this checkout; pass a path if it lives elsewhere. `--sync` copies
BotMarket's copies over this repository's — BotMarket is the origin of all three, so it wins a sync,
and an edit made here first should be carried across by hand rather than by a flag that could
silently overwrite it.

When BotMarket is not present the script exits **zero** and says so. That is deliberate: a vendor who
has cloned only this repository to write a driver should never be told their tree is broken because
a private repository is missing. For the same reason it is not a CI job — CI checks out one
repository, and a check that cannot run is not a check.

## Where progress is logged

**In `PROGRESS.md`, here.** Not in `skynet-build-plan.md`.

The build plan is the Hub's document and its session log is the Hub's history. Work on the operating
system belongs to this repository, and appending it to a mirrored file would either put OS history
into the Hub's log or break the mirroring rule above — both of which are worse than having two logs
that each cover one thing.

The one case where an OS session touches the build plan is when it changes something **in the Hub**.
Then the change is made in both copies, and the reason is recorded in `PROGRESS.md` as well, so this
repository's log stays a complete account of what was done from here.

## Reading order

Part I of `skynet-os-architecture.md` is the design. Part VII is the honest account of what does not
work, and it is worth reading before the rest — the word "universal" invites a strong reading that
the document spends a good deal of its length refusing.

`../README.md` is the golden path (§65): what a developer reads, in order, to reach a moving robot
without reading Parts II through VII.
