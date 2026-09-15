# The wire fixtures

> **What a robot actually sends, frozen, so the other end can be tested against it.**

The conformance corpus in `corpus/` holds N language editions to one specification. These hold the
**two repositories** to one document: each file here is the exact canonical body `skynetd` puts on
the wire for one §17 route, produced by the real producer rather than typed out by hand.

## Why they exist

The twenty-seventh session found that Skynet OS and the Hub had both built the episode path — a
producer, a queue, a route, a table — and had never once put one document through the other. They
did not agree about a single field name. `POST /episodes` ignores fields it does not know and
answers `201`, so every episode would have been stored with a null task, a null duration and nothing
identifying the skill, with both suites passing the whole time.

Neither suite could have caught it. The OS's asserted what the OS sends; the Hub's asserted what the
Hub accepts. **A test of a boundary has to hold one side's real output against the other side's real
contract**, and that means the fixture has to be produced by the code rather than written beside the
test that reads it.

## How they are kept honest

Both directions, so neither repository can drift alone:

* **In this repository**, `skynetd-io`'s suite rebuilds each document from its producer on every run
  and compares. A change to a producer fails here, with the command that re-freezes it.
* **In the Hub**, `lib/__tests__/skynet-wire.test.ts` pushes its own vendored copy through the
  receiving route's validation and asserts the facts survive. A change to a route that drops a field
  fails there.
* **The two copies are pinned by digest** in `mirror-digests.json`, in both repositories, written by
  `node tools/check-contract-mirrors.mjs --sync`. So a fixture that moved here without the Hub being
  resynced fails in CI with one checkout, which is what CI has.

## Re-freezing

When a producer legitimately changes:

```
SKYNET_WRITE_WIRE=1 cargo test -p skynetd-io
node tools/check-contract-mirrors.mjs --sync
```

Then run the Hub's suite. If it fails, the change is one the Hub owes and the two land together —
which is the entire point of freezing them.

## What is here, and what is deliberately not

| File | Route | Producer |
|---|---|---|
| `episode.json` | `POST /episodes` | §54.4's lifecycle, at the terminal transition |
| `telemetry.json` | `POST /devices/{id}/telemetry` | §74's window, closed by a reconcile pass |
| `incident.json` | `POST /devices/{id}/incidents` | §38.1's profile-signature alarm |

**The check-in is not here.** It already has this exact treatment in the other direction: the Hub's
`skynet-fleet.test.ts` carries the document verbatim from the corpus case
`checkin/document.json`, under a header explaining that a Hub tested against a document a Hub author
invented is a Hub tested against its own assumptions. Adding a second frozen check-in here would be
a second source of truth for one document, which is the failure §31.2 names.

**Nothing is here that no producer emits.** A frozen fixture for a document nothing writes is a
fixture that freezes somebody's guess, and the gap it hides is the one this directory exists to
find.
