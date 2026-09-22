# Troubleshooting

Symptom first, then the diagnosis, then the fix — in the order each one bites. The
developer guide's ["When something is wrong"](developer-guide.md#when-something-is-wrong)
is the orientation; this is the reference it points at. Every refusal the system prints
already names its cause and next action with the spec section behind it, so if you are
here from an error message, the message is usually the faster answer.

Three commands open nearly every session below. On a robot they run under `sudo` (the
socket is root's), and everything past `status` takes `--token`:

```bash
skynet status                      # is the supervisor up, which body, is stop reachable
skynet state                       # which of §27's conditions holds the robot where it is
skynet doctor --out bundle.json    # all of it and more, in one file — attach it to bug reports
```

## Install and bring-up

### `systemd is not PID 1 here`

Skynet OS is supervised by systemd (§33.1's ordering lives in the unit files), and the
machine was booted under something else. On Windows this means WSL without systemd:
enable it in `/etc/wsl.conf` (`[boot] systemd=true`), then `wsl --shutdown` from
Windows and reopen. On a Mac, use a Linux VM. There is no supported install without a
systemd manager — anything else would be an unsupervised supervisor.

### `this needs root`

`install.sh` and `virtual-robot.sh up` write to `/usr/bin`, `/etc` and the unit
directory. Re-run with `sudo`. The download step (`packaging/get.sh`) never needs it.

### `install.sh --check` reports a failed postcondition

`--check` re-runs every assertion `install` promised: binaries executable, both env
files present, the env file modes (an admin token readable by anybody is a finding, not
a file), and — read back from systemd rather than from the unit files — the cpuset and
the writable paths. A failure names the path and the expectation; fix what it names and
re-run. The two subtle ones:

- **`226/NAMESPACE` or a dead unit at first start.** Under `ProtectSystem=strict` every
  writable path must exist before the namespace can be built. `--check` verifies each
  `ReadWritePaths` entry for exactly this reason; a unit dying here means a path is
  missing, not that the binary is broken.
- **A directive systemd silently dropped.** systemd does not fail a unit for a directive
  it cannot parse — it drops it and logs once. `--check` asks `systemctl show` what
  landed rather than reading the unit file, so trust it over the file.

### The install failed — where the record is

Every step is numbered and timed (`✓ [3] (+45ms)`), and three logs exist depending on
how the install ran. Send the matching one with any report:

- `skynet setup` (the wizard): `skynet-setup-<host>.log` on the PC — the plan, every
  streamed line, and the verdict. The remote side is left at `/tmp/skynet-setup/` on a
  failure (with `install.log` beside the stage) and removed on success.
- `install.sh --log-file <path>`: byte-identical to what the terminal showed, without
  color codes, truncated per run.
- Neither: re-run with `--log-file`; the failure reproduces, the installer is
  idempotent and never re-issues an existing credential.

The wizard's own early refusals, in the order it makes them: **cannot reach the robot**
carries ssh's first line and the remedy it implies (key, user, sshd, DNS — plus the
exact call); **wrong architecture** means the bundle's ELF headers disagree with the
robot's `uname -m`, caught before anything is copied because installing would succeed
and then every unit would die; **cannot write** names the paths the SSH user cannot
write — connect as root. A **short copy** (bytes sent vs bytes the robot holds) is the
network, not either end: run again.

Two of the wizard's defaults can also refuse, and both name the flag that overrides
them. With no bundle anywhere it fetches the robot's release (checksummed, like
`packaging/get.sh`): a fetch that fails — no curl, no network, no release for the
robot's architecture — falls back to asking for a path, or to an error under `--yes`.
With no `--descriptor` it derives the body from the driver: a driver that will not
describe itself fails the run, and `--descriptor` with a hand-written body is the way
past it.

### `virtual-robot.sh up` fails an assertion

The ten assertions are read back *from the robot*, so a failure is a sentence about the
robot, not the installer. Re-run the read side alone against what is up:

```bash
sudo ./packaging/virtual-robot.sh smoke
```

then read what it points at: `journalctl -u skynetd -u skynetk -n 80`. The usual
suspects, in order: the socket file exists with nothing behind it (a stopped robot
leaves the file — `socket_answers` checks the answer, not the file); one unit active
and the other not (`systemctl is-active skynetk.service skynetd.service` — they are
separate on purpose, §33.1, so half a robot is a diagnosable state); the supervisor
still starting (the assertions wait for the token first — a fresh `up` needs seconds).

### The socket does not answer at all

`skynet status` against `/run/skynetd.sock` failing with a connection error means no
listener: either `skynetd` is not running (`systemctl status skynetd.service`), or you
are looking at the wrong socket (a workstation `skynetd` run by hand listens where it
was told — check `--socket`). A stale socket file from a stopped robot is normal; stop
leaves it, start replaces it.

## Access and identity

### `401` / `refused` on routes past `status`

Past the observer routes, every call needs the admin token: `SKYNET_ADMIN_TOKEN` in
`/etc/skynet/skynetd.env`, passed as `--token` (or `$SKYNET_TOKEN`). `skynet doctor`
without one still produces a readable bundle — the refused routes are recorded as
refused with the reason — which is itself the diagnostic: if the bundle says refused
everywhere past `status`, the token is missing, not the robot.

### Pairing refuses the code

`skynet pair` refuses a mistyped code *before* any network exists, and refuses a
credential pasted where a code belongs. Read the refusal: it distinguishes the two.
Past that, pairing is one HTTPS request (§10) — a failure there is a network or Hub
problem, and `skynet outbound` shows the queue it would have joined.

## The robot will not move

### `skynet state` names a holding condition

This is the normal case and it is working as designed: §27 derives the state from eight
conditions in rule order, and the first one that fails names the blocker. Read them in
order — a robot held at rule 5 (`homing`) also has unread rule-8 state behind it, so
fixing the named condition and re-reading is the procedure, not the workaround. The
three that confuse people:

- **`unhomed` on a robot with absolute encoders.** Bits 4–5 of §24's flags word are a
  two-bit homing field: `unreported` (the driver never answered) holds exactly like
  `pending` but sends you to the driver instead of the homing routine.
- **`policy_not_ready` vs `no_policy`.** "Nothing to run" and "something installed that
  has not got a lease" are different sentences with different remedies; the second
  means `skynet ps` shows a commanding package whose unit is not running, or a plan
  that was refused.
- **`guard_hold: advisory`.** An advisory hold is *not* a hold: a tier that cannot
  promise a clamp records the verdict and forwards the command anyway (§7.4). A Guest
  robot reporting `advisory` while moving is telling the truth.

### The tier is Guest and you expected Host

Guest is where every robot lands until §15's bench evidence says otherwise — check what
caps it before assuming a defect: `simulator: true` on the descriptor caps at Guest by
mechanism (§35.3), a null envelope caps by §7.6 (guards that need one report
unavailable, which `skynet limits` shows per guard with its missing input), and a
Python policy process caps by §31.3. `skynet conformance` runs the eight runtime tests
on the robot itself and names the rung each one waits on.

### Limits resolve to nothing useful

`skynet limits` shows the resolved set *with provenance* — the descriptor's envelope,
the signed profile's ceilings, the cell, the transport clamp — and every missing input
by name. An empty-feeling resolution is usually one input: `people_detection` is not a
thing §5 lets a body declare, so `proximity` stays unavailable with the sensor named
rather than the profile; an unrecognised component gates like the kernel (§12) until a
bench pass binds it to a digest.

## The supervisor

### `the previous supervisor crashed`

The `skynetd` before this one panicked: the panic message and the `file:line` are the
diagnosis. You meet it in three places — the journal line at boot (`the previous
skynetd (pid N) panicked: …`), the `previous` row of `skynet doctor`'s summary, and
the banner on the console's health panel — and all three read the same report.

The report itself lived in `state/last-crash.json` and is already gone: the boot that
found it cleared it, after writing it into the journal and recording a
`supervisor_crashed` black-box event beside the cycles. `GET /health` carries it as
`previous_exit` until the next restart. If the robot has *not* restarted since the
crash — the daemon died and nothing replaced it — the file is still there to read.

### `the previous supervisor left no record of its exit`

No crash report *and* no clean-stop marker, on a machine that has run before. The
previous daemon was killed, ran out of memory, aborted, or lost power — which one is
not recorded, because none of those runs the code that would record it. The journal
around the death is where that question is answered: an OOM kill names its victim,
a signal names its sender in the unit's stop lines, and power loss names nothing at
all. `GET /health` carries this as `previous_exit: unclean`, distinct from `clean`
(the marker: the process chose its exit) and `unknown` (first boot: nothing to judge).

The supervisor is only half the story: the kernel host (`skynetk`) writes its own
report to `state/last-kernel-crash.json`, which the supervisor's next pass takes into
a `kernel_crashed` event and serves as `kernel_exit` — the `kernel` row of
`skynet doctor` and the second banner on the console's health panel. The kernel writes
no clean-stop marker (it has no graceful stop path — every systemd stop SIGTERMs it),
so its exit is crashed or nothing. A crashing supervisor, or a crashing kernel, is a
bug in that binary: file it with the doctor bundle, which carries the message, the
location and the events around it.

## Drivers

### `skynet driver check` is red

Run it the way the author does — through the driver's own suite first, which speaks the
driver's language, then the conformance run:

```bash
cd drivers/mock && make check
```

`driver check` reports the tier the evidence supports, not a bare pass or fail, and
each red check names the property. The common ones: `embodiment.roles.coverage` (a
joint role outside the eight canonical axes — the vocabulary refuses to guess, §5.1),
a sensor the driver declares and cannot read (a declared stream nothing can read binds
a policy's observation to nothing), and a descriptor whose digest does not match the
body it describes.

### A Python driver will not load

Three environment variables, all three set by the Makefiles that work: `PYTHONPATH`
must include `bindings/python` and the driver's own directory, `SKYNET_PYTHON_DRIVER`
names the module, and `SKYNET_PYTHON` names the interpreter when it is not `python3`.
The failure is loud about which one is wrong. On Windows checkouts, two environment
traps live outside any driver: a Windows `node`/`npm` earlier on `PATH` shadowing the
Linux toolchain, and Zig's cache rename failing on the mount (keep `ZIG_LOCAL_CACHE_DIR`
and `ZIG_GLOBAL_CACHE_DIR` on a Linux filesystem).

### MuJoCo imports fail or render nothing

MuJoCo resolves the GL backend named by `MUJOCO_GL` *at import*, so a missing library
fails the import rather than a later render — install the backend's library
(`libosmesa6` for osmesa, Mesa EGL for egl) or name one that is installed. With no
backend at all the driver declares no cameras, which is a real branch and a weaker
test: `MUJOCO_GL=osmesa` in CI where the library is installed, `MUJOCO_GL=egl` on a
machine with Mesa.

### The ABB live row never reaches a controller

That row's substrate is a RobotStudio virtual controller speaking EGM, and the whole
procedure — install, controller, WSL networking, bridge side — is
[the RobotStudio runbook](robotstudio-runbook.md). The single most common failure is
the UDPUC remote address: EGM is controller → ROS over UDP, so the address must be the
WSL machine's IP, not localhost, unless mirrored networking is on.

## Skills

### A skill is accepted and never starts

`skynet skill run` means accepted, not started: the next reconcile pass evaluates
§54.2's preconditions against the live context and may refuse, naming the clause. A
fact the robot cannot observe is *absent* rather than defaulted, so a precondition
naming it refuses with the path named — that names the sensor to fit, not a bug to
file. Past preconditions, leases acquire all-or-nothing (§33): a partial acquisition
rolls back and the episode records which lease was missing.

### A skill times out or is judged failed

The timeout is enforced by the OS with an input the package declares, and success is
judged over post-state (§54.3), not self-reported. `skynet blackbox` shows the episode
with its transitions; a judgement that surprises you is usually a precondition that
held at start and a post-state that did not survive contact. `skynet skill check`
against the package directory replays the workstation half — manifest, digest,
over-requested capabilities, timeout input — without a robot.

## Updates and fleet

### OTA waits and never applies

An apply waits for a declared safe state (§12): if the robot never reaches one, the
update waits, correctly. Past that, a boot confirms or condemns its own slot within a
budget — a condemnation is the rollback working, and `skynet ota` shows the slot and
the generation lag. An image that changes the Safety Kernel or the envelope without a
bench pass bound to its digest is refused, not queued.

### The Hub never sees this robot

`skynet checkin` shows what the robot would send; `skynet outbound` shows the queue and
its depth. An unpaired robot queues nothing deliberately — a check-in addressed to
nobody would be counted as dropped state. A paired robot that still never appears is
usually the credential (`skynet identity`) or the network the pull arrives on; a Hub
serving two documents under one generation is refused by §11, and the refusal names
the generation.

## Building and developing

### The corpus regenerate check is red

`node tools/corpus-gen/generate.ts` followed by a diff in `corpus/` means the
reference implementation's behaviour changed without the corpus being regenerated in
the same commit (§31.4). Regenerate and review the diff as the specification change it
is — the editions are measured against those bytes, so a stale corpus is a stale exam.

### An edition diverges

Each edition's runner reports passed/failed/not-implemented per file; red on a
divergence, green on a gap. A divergence is a defect in the edition (or, rarely, a
reference change the corpus has not caught up with — see above). A gap that reopens is
the ratchet's job: `--min-passed` only ever rises. Run the edition's own line from
[the editions list](../editions/README.md) before anything else.

### Clippy is clean here and red in CI

A lint result is only as broad as the target it ran on (§57): CI runs clippy on Linux
and Windows, and `cfg(unix)` code warns where it compiles. The Windows job also builds
the tests, which has caught missing `cfg`s that Linux never sees. Match CI's scope
before pushing: `RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets`.

### TypeScript passes locally and fails in CI (or the reverse)

Node strips types without checking them — `node` running a file proves nothing about
its annotations. The gate is `npx tsc -p .` after installing `typescript` and
`@types/node`, and it covers the oracle, the tools and every edition-local generator.
Node 24 runs the corpus job and Node 26 the case tables (the Unicode version differs),
so a case-table failure under the wrong major is an environment mismatch, not a defect.

## Reading a doctor bundle

`skynet doctor --out bundle.json` writes fourteen sections — thirteen routes plus the
black-box events — in §41.3's order, and prints the half a person acts on. Each
section records its route so the request can be reproduced by hand:

| Section | Route | Answers |
|---|---|---|
| state | `GET /state` | which condition holds the robot |
| health | `GET /health` | is the supervisor itself well |
| metrics | `GET /metrics` | passes, overruns, restarts — the counters behind health |
| status | `GET /status` | the one-line summary |
| ota | `GET /ota` | image slot, what is staged |
| intent | `GET /intent` | generation lag against the Hub |
| limits | `GET /limits` | resolved guards, limits, provenance |
| packages | `GET /packages` | budgets versus observed usage |
| outbound | `GET /outbound` | connectivity and queue depth |
| power | `GET /power` | thermal and battery history |
| plan | `GET /plan` | the last reconcile plan and its deferrals |
| blackbox | `GET /blackbox` | the window's shape and recent end |
| events | `GET /blackbox/events` | the last N events (default 200) |
| audit | `GET /audit` | who changed what, when |

A section recorded as `unread` with a reason is data: a refused credential, a 503 from
a recorder that is not running, a route this tier does not serve. A bundle of refusals
still diagnoses — it says where the asking stopped.

## Filing a bug

Use the bug template, and attach `skynet doctor --out bundle.json` taken from the
robot that misbehaved (or from the bench over `--host`). A security or safety issue —
a command bypassing the kernel, a skippable credential check, a sandbox escape — does
**not** go in a public issue; see [SECURITY.md](../SECURITY.md).
