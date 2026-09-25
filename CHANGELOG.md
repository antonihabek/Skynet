# Changelog

All notable changes to Skynet OS, in Keep a Changelog form. Versions are the
workspace version in `Cargo.toml`, tagged `v<version>`; `package.yml` refuses a tag
the workspace disagrees with, so the changelog, the tag and `skynet version` cannot
drift apart. Dates are release dates — entries land under Unreleased first.

## [Unreleased]

### Added

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

## [0.1.0] — unreleased

No tags exist yet, so everything above is also everything below. The first `v0.1.0`
tag will exercise the release bundle, the installer-against-bundle test and the
documentation publish for the first time; until then this file describes the tree,
not a release.
