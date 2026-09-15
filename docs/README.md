# Specification

One document: [`skynet-os-architecture.md`](skynet-os-architecture.md), the specification of
Skynet OS. Every `§` reference in this repository — in code, in commit messages, in error messages
a robot prints — points into it.

## It is shared with the Hub, and never diverges

The same file lives in the Hub's repository, which is private. The two are separate for good
reasons (§72.1: licensing history, vendor readability, toolchain overlap, release cadence), and the
specification is not one of the things that should be split by that boundary: the Hub is built
against this design and so is the OS, and two copies drifting apart would mean two teams building
against two designs while both believed they were reading the same one.

> **The specification is edited in both repositories or in neither.**

When it changes:

1. Make the edit.
2. Make the same edit in the other repository.
3. Re-pin from the two agreeing copies:

```bash
node tools/check-mirrors.mjs --pin ../path-to-hub
```

`mirror-digests.json` holds the agreed sha256, so the check needs only one checkout — which is what
CI has, and what anybody who cloned only this repository has:

```bash
node tools/check-mirrors.mjs
```

That exits non-zero when the specification no longer matches its pin. Editing the pin by hand
switches the check off; `--pin` refuses unless both copies agree.

Where the specification mentions the Hub's own architecture (`docs/skynet-architecture.md`), that
document is in the Hub's repository and is not published here.

## Reading order

Part I is the design. **Part VII is the honest account of what does not work**, and it is worth
reading before the rest — the word "universal" invites a strong reading that the document spends a
good deal of its length refusing.

You do not need Parts II–VII to build a driver. [`developer-guide.md`](developer-guide.md) is the
golden path (§65): what a developer reads, in order, to reach a moving robot.
