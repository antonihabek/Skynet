# Changelog

All notable changes to Skynet OS, in Keep a Changelog form. Versions are the
workspace version in `Cargo.toml`, tagged `v<version>`; `package.yml` refuses a tag
the workspace disagrees with, so the changelog, the tag and `skynet version` cannot
drift apart. Dates are release dates — entries land under Unreleased first.

## [Unreleased]

### Added

- The fleet sweep (`fleet/`, `DRY-COVERAGE-PLAN.md` Phase 10): every public robot description in
  `robot_descriptions` and DeepMind's menagerie, with this repository's own vendor descriptions —
  pinned by upstream commit and by bytes in `fleet/catalogue.json` — run through Skynet's URDF
  importer, the MuJoCo driver behind the C host, `skynet driver check`, `skynetk` with a policy
  asking past every rating and a replay of the sealed window, a check of both ends of every
  joint's travel, and `skynet transport` of a skill trained on another robot of the class.
  `reference/reports/fleet.json` is the result; `tools/check-fleet.mjs` holds it to the catalogue,
  to `drivers/models.json` and to the figures `docs/status.md` and the README quote, and refuses
  a release stage in it; `.github/workflows/fleet.yml` re-runs it nightly and fails on a row that
  got worse.
- `skynetk run --source <policy.json>`: the policy's source embodiment, resolved against the
  body's own descriptor by the planner `skynet transport` runs. Channels, each joint's ceiling in
  its own mode's units and the modes come from the plan, so the envelope guard has something to
  hold (F158). A refused plan, or one that binds a channel anywhere but the joint at its index, is
  refused. `skynetk`'s summary now names the first failed write — cycle, code and mode.
- `drivers/mujoco`: `hold_gains` in the configuration, so a `<motor>`-driven joint holds position
  on a zero mask (§25) with the operator's stiffness, and is still refused without one (F162).
- Proposed amendments `docs/issues/travel-has-two-ends.md` (F159),
  `the-plan-never-reaches-the-kernel.md` (F158) and `four-legs-in-a-two-leg-vocabulary.md` (F163).

- Remote-policy provenance (§90.6): the recording marks which cycles ran on a remote policy's
  proposals, with the session id and the model digest. `skynetp --backend remote` writes each
  session fact (opened, first proposal, last proposal) into a queue beside its bridge socket;
  `skynetd` carries them into the ring as `remote_session` events (a new §26 event code) on each
  reconcile pass, and a reader joins them to cycles by the executed command's stamp. The
  daemon's recording index and `skynet blackbox` list each session with its digest, its commands
  and the cycles it drove. Facts the policy host could not queue are recorded as a gap.
- Two §74 telemetry counters, `remote_proposals` and `remote_stale` (§74.7): what a remote
  policy proposed, and the turns it stayed quiet because its newest chunk was older than its
  deadline, carried from the same queue as deltas. Unmeasured on a robot with no remote policy,
  and in a window where a policy host stopped without writing its final count. Every edition's
  vocabulary and the corpus carry them.

- Driver release stages: every driver that commands a physical robot carries a stage in
  `drivers/models.json` — `alpha` (tested only against counterparts written in this
  repository) or `beta` (also against the maker's own controller software or simulator, or
  an independent implementation of its protocol) — with what the stage rests on, per body
  for `ros2_control`. `skynet setup --robot` prints it before installing and on every
  refusal, and says first that the driver has never run on a physical robot. `stable` is
  reserved for a driver that has, and none has; `tools/check-release-stages.mjs` holds the
  manifest, `docs/status.md` and the nightly workflow to each other in CI.
- Nightly substrate workflow (`.github/workflows/substrates.yml`): every beta driver runs
  against its maker's software each night at the pinned versions and again at the makers'
  newest — URSim for both RTDE drivers and the UR5e behind `ros2_control`, DRCF for the
  Doosan M1013, Unitree's MuJoCo simulator, AgiBot's own X1 deployment, franka-sim, and the
  EGM client — so a vendor release that breaks a driver is found the night it ships. The
  UR5e and X1 substrates are new pinned images (`ros2_control/ur/Dockerfile`,
  `x1/sim/Dockerfile`); before them each could be run on one machine only.

- Supervisor watchdog: `skynetd.service` is `Type=notify` with `WatchdogSec=120s`;
  the daemon sends `READY=1` once serving and `WATCHDOG=1` from the loop body, so a
  hung loop is restarted within two minutes. `install.sh --check` pins both the
  watchdog and the deliberate asymmetry — `skynetk` must *not* be watchdogged, since
  a hung control loop holds rather than restarting into unknown hardware state.
- Pass overrun warnings and `GET /metrics`: each reconcile pass is timed against its
  interval, overruns warn per pass, and the route serves the daemon's counters
  (passes, overruns, requests, uptime) beside the live queue and unit counters —
  rendered as the console's Supervisor panel and the `passes` row of `skynet doctor`
  when overruns exist.
- Machine-readable API error codes: every §33.7 refusal carries a `code` beside its
  detail — the daemon's `api.*` vocabulary plus the pairing and skill codes passed
  through — all catalogued for `skynet explain` (51 new entries, `docs/error-codes.md`
  regenerated), with the suite scanning the emission sites against the table. The CLI
  prints the code and the lookup on every refused request.
- Kernel crash parity: `skynetk --crash-file` (wired in the unit file) writes panic
  reports in the shared `skynet-crash` format; the supervisor's every pass takes the
  file, logs it at error, records `kernel_crashed`, and serves it as `kernel_exit` —
  the `kernel` row of `skynet doctor` and a console banner. Hook writes are atomic
  (sibling + rename) so a per-pass reader never catches a half-written report.
- Four-state previous exit: a clean-stop marker (a `Drop` guard that writes nothing
  while unwinding) joins the crash file, so `GET /health` reports `clean`, `crashed`,
  `unclean` (killed, OOM, abort, or power loss — the journal names which) or `unknown`
  (first boot) instead of guessing. Doctor and the console banner the two abnormal
  states; `docs/troubleshooting.md` gained both rows.
- Release profile unwinds: `panic = "unwind"`, because the crash-report hook only runs
  on unwind and every release-build crash — every crash on a robot — previously read
  as an unrecorded kill. Safe for the control loop (the RHAL ABI has no callbacks
  into Rust), and pinned by a CI step since no test could catch a revert.
- Failing console panels name their code: the page printed the refusal's sentence
  without the `code` beside it, so nothing on screen could be looked up. Every
  failing panel now prints `sentence [api.*]`, and the suite pins the three fields
  on all four refusal shapes (401, 403, 404, 405).
- The installer narrates: `install.sh` numbers and times its steps (`✓ [3] (+45ms)`),
  mirrors every line to `--log-file` (same bytes, no color codes), and gives a
  sentence to the failures that used to exit silently — a `run` wrapper carrying the
  command's own words for the systemctl calls, and an exit trap naming the step any
  other death stopped after. Pinned by `packaging/test-install-output.sh` in CI.
- `skynet setup`: the install wizard, running on the PC. Host and driver are the whole
  of the required input — the body is derived from the driver and the bundle fetched
  for the robot's architecture (checksummed, `get.sh`'s scheme) when either is
  missing, each named in the plan. It copies a stage over SSH (one tar pipe,
  byte-count verified), runs the installer remotely while streaming its numbered
  steps, verifies with `--check`, and keeps a local `skynet-setup-<host>.log` —
  refusing before the copy when the robot is unreachable, the wrong architecture
  (read from the ELF headers, the one failure the installer cannot catch), or not
  writable by the SSH user. `--dry-run` prints the plan and every ssh invocation
  exactly. Covered by 25 CLI unit tests and `packaging/test-setup-wizard.sh`
  (stub ssh, real tar, real fetch over file://) in CI.
- The wizard takes `--robot <model>` (`drivers/models.json` maps all twenty-four: built
  libraries resolve from the bundle or the checkout, the rest refuse typed — missing
  SDK, simulation-only, Python-under-a-host, unmapped), offers to start the units and
  run the first health check after installing (both best-effort; `--yes` prints the
  commands instead), verifies downloads with `certutil` on Windows, and asks one fewer
  question (the SSH user defaults silently and shows at the confirmation). The
  developer guide's step 8 is now the one documented install path.
- `skynet doctor` reads fourteen sections: `metrics` (the counters behind health) and
  `audit` (who changed what, when) join the bundle; the architecture's section count
  is updated in both copies and re-pinned.
- JSON log format and operation IDs: `skynetd --log-format json` prints one
  JSON object per stderr line (service, wall timestamp, level, operation,
  message) for fleet log consumers; start-up, each reconcile pass and each
  served request run inside an operation (`boot`, `pass-N`, `req-N`) carried on
  every line. The text default is byte-identical.
- Crash reports (§64): `skynetd` installs a panic hook writing
  `state/last-crash.json`; the next boot logs it at error, records a
  `supervisor_crashed` black-box event, and carries it on `GET /health` as
  `previous_exit` — bannered on the console health panel and the `previous` row
  of `skynet doctor`.
- Troubleshooting guide (`docs/troubleshooting.md`): symptom-first reference from
  install failures to fleet mysteries, held to the same mechanical checks as the
  golden path — every command it shows is one the CLI dispatches, every `make`
  target exists, every link resolves.
- Versioned documentation: every `v*` tag publishes the checked documentation set
  to GitHub Pages under `versions/<tag>/` with a `latest/` copy and an index
  (`.github/workflows/docs.yml`, `tools/build-docs-site.mjs`).
- Apptronik Apollo simulator driver (`drivers/apollo`): the menagerie MJCF at a
  pinned commit through `skynet_mujoco` — 81-check suite, layout cross-check,
  sim conformance, Guest tier.
- RobotStudio runbook (`docs/robotstudio-runbook.md`): the Windows-side procedure
  for the ABB-live EGM substrate, plus the no-RobotStudio fake-controller route.
- KUKA KR 6 R900 over RSI behind `ros2_control` (`make -C drivers/ros2_control
  check-rsi-docker`): KUKA's own `KukaRSIHardwareInterface` against KUKA's own
  `kuka_rsi_simulator`, in a pinned image at the release set ROS Jazzy ships — the pose
  read, an axis moved and brought back, every closed session ended by RSI's stop flag as
  the simulator's own log counts it. The body is `beta` and runs in the nightly substrate
  workflow at the pins and at KUKA's `master` — there with the description KUKA's `master`
  builds, since KUKA moves its plugin and its descriptions together.

### Fixed

- `skynet driver check` probed "accepts a well-formed command" in position mode on every body, so
  a body that declares torque alone — every quadruped in DeepMind's menagerie — reached no tier for
  refusing a mode it never offered. The probe now uses a declared mode, gentlest first (F161).
- `skynet descriptor from-urdf` read a joint's axis in its own rotated frame, calling every joint
  of a Universal Robots arm a yaw; it now reads the axis in the body's frame at the zero pose (F166).
  It also copied the sentinels URDFs write for "unlimited" — `effort="1e30"`, `lower="-1e16"`,
  `effort="-1"` — into descriptors as ratings; they are now left out and named (F168).
- `drivers/mujoco` ignored actuator gear: a geared servo settled at its command divided by the
  gear, a geared motor applied gear times the torque, and every force read back and every force
  ceiling was in the actuator's units rather than the joint's. A `<motor>` bounded by `ctrlrange`
  declared no rating at all. Both are carried through the gear now (F167). A moving base declared
  for a scene whose root is fixed to the world is refused rather than written into a descriptor
  whose own provenance said the base was fixed (F164).
- `drivers/ros2_control` reads and writes its hardware plugins in turn — one write after
  each read, as `controller_manager` does — instead of when its caller does. A
  request/reply plugin (KUKA's RSI client) wedged on the first read's second read and
  then reported its first telegram's pose as live under fresh stamps; a command with no
  read waiting now goes with the next turn's write, and the first read no longer sleeps
  when it already holds a sample.
- `drivers/ros2_control`'s `close()` deactivates its hardware plugins before shutting
  them down, so a plugin's own stop path runs: KUKA's sends RSI's stop flag in
  `on_deactivate`, and the controller of every closed session had been left to find out
  by timing out.
- `drivers/ros2_control`'s `open()` refuses a body whose hardware plugin failed to load or
  to initialize. The resource manager reports no component at all after either failure, so
  the check that every component came up active had nothing to refuse, and `open()`
  succeeded with no plugin behind the body — as it did for KUKA's plugin at `master`, handed
  the URDF of KUKA's last description release. The check now walks the components the URDF
  declares.

## [0.1.0] — unreleased

No tags exist yet, so everything above is also everything below. The first `v0.1.0`
tag will exercise the release bundle, the installer-against-bundle test and the
documentation publish for the first time; until then this file describes the tree,
not a release.
