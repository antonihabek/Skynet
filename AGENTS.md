# Skynet OS — guidance for coding agents

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) before planning, editing, reviewing or validating code, and
follow it. The rules in its *enforced, not requested* section are CI checks; run the commands in
*Before you open a pull request* before calling any change done.

[`docs/skynet-os-architecture.md`](docs/skynet-os-architecture.md) is authoritative for what the
system must do. Cite it by section (`§33.1`) in code, comments and messages. It is shared with the Hub's
repository, so it is edited in both copies or in neither, and re-pinned — see
[`docs/README.md`](docs/README.md).

Maintainers may keep additional, machine-specific guidance in `CLAUDE.local.md`, which is not
committed.

Standing owner rulings (2026-09-28, do not re-ask): downloads needed for the work are allowed
unless malicious; bug reports about vendors' own software stay as local drafts and are never
posted publicly; SAFE clears from the robot's own socket only; a service's network is a console
switch, never hardcoded.
