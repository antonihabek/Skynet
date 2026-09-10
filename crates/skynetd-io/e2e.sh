#!/bin/sh
# The whole chain, on a real machine (§33.7, §37).
#
#   skynet install → the store → skynetd → the reconciler → the local socket → skynet ps
#
# Everything the suite proves against a temporary directory, proved again against a running daemon
# with a Unix socket in front of it. The suite can be wrong about how the pieces fit; this cannot.
#
#   sh crates/skynetd-io/e2e.sh                       # against whatever `cargo` builds
#   SKYNET_BIN=target/release sh crates/skynetd-io/e2e.sh
#
# It needs systemd only for the one step that starts a package, and says so when there is none —
# a check that quietly does not run is the failure §15 spends a rule on.

set -eu

# Plain text, so `grep` reads what a person reads. `skynet` colours when TERM says a terminal is
# there, and a check that passed only on a machine with no TERM is a check that does not run.
NO_COLOR=1
export NO_COLOR

ROOT="${ROOT:-/tmp/skynet-e2e}"
BIN="${SKYNET_BIN:-}"
FAILED=0

say()  { printf '\n\033[1m%s\033[0m\n' "$1"; }
pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED=$((FAILED + 1)); }
want() { # want <description> <expected substring> <file>
  if grep -q -- "$2" "$3"; then pass "$1"; else fail "$1 - expected '$2' in:"; sed 's/^/      /' "$3"; fi
}

# Every background process this script starts, so the exit trap can end all of them.
#
# This is not tidiness. Until this round the trap killed **one** of the five — `$DAEMON` — and the
# other four (`$MDNS`, `$KDAEMON`, `$KLOOP`, `$KSTOP`) were killed only by the lines that follow
# them, which an early exit never reaches. A leftover `skynetd` is not a stray process: §33's unit
# names are `skynet:<ns>:<name>.service` in **one** user manager, so a second supervisor observes
# the first one's units, finds them absent from its own desired state, and stops them. Two daemons
# on one machine fight over every package on it.
#
# What that cost: a `skynetd` abandoned by an aborted run three hours earlier was still stopping
# `openrobot/holder` half a second after each start, so §59's ten confinement checks read
# `systemctl show`'s defaults for a collected unit and reported ten directives as not landed, and
# §15's isolation test found no sandboxed unit to run against. Every one of those failures named
# the wrong thing, and the run they came from looked like a regression in the daemon.
#
# A robot has one `skynetd`, so none of this is a defect in the daemon — it is a defect in a script
# that starts five and cleans up one.
TRACKED=""
track() { TRACKED="$TRACKED $1"; }
kill_tracked() {
  for pid in $TRACKED; do
    kill "$pid" 2>/dev/null || true
  done
  # And anything this script's own binaries left behind under this root — a process whose pid this
  # shell never learned, because `$!` is only ever the pid of the last one started.
  pkill -f "skynetd --root $ROOT" 2>/dev/null || true
}

# Start the daemon and wait until it actually answers.
#
# Waiting for the socket *file* is not enough, and finding that out here is the point: a Unix socket
# outlives the process that made it, so a killed daemon leaves a path that looks live to every
# client. `skynetd` unlinks it on bind; a client cannot, and gets `connection refused`.
start_daemon() {
  rm -f "$ROOT/run/skynetd.sock"
  "$SKYNETD" --root "$ROOT" --user --interval 1000 > "$ROOT/daemon.log" 2>&1 &
  DAEMON=$!
  track "$DAEMON"
  i=0
  while [ $i -lt 60 ]; do
    if "$SKYNET" status --socket "$ROOT/run/skynetd.sock" >/dev/null 2>&1; then return 0; fi
    i=$((i + 1)); sleep 0.1
  done
  echo "the daemon never answered"; cat "$ROOT/daemon.log"; exit 1
}

stop_daemon() {
  kill "$DAEMON" 2>/dev/null || true
  wait "$DAEMON" 2>/dev/null || true
}

if [ -z "$BIN" ]; then
  cargo build -p skynetd-io -p skynet-cli -p skynet-kernel-host -p skynet-policy-host >/dev/null
  BIN="$(cargo metadata --format-version 1 --no-deps 2>/dev/null | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')/debug"
fi
SKYNETD="$BIN/skynetd"
SKYNET="$BIN/skynet"
SKYNETK="$BIN/skynetk"
[ -x "$SKYNETD" ] || { echo "no skynetd at $SKYNETD"; exit 1; }

# This file drives a Unix socket, an atomic symlink swap and systemd units, and `skynetd` refuses to
# serve anywhere but Linux — it says so itself and points at `skynetd once`. Run from Git Bash on
# Windows it stopped on `no skynetd at .../debug/skynetd`, which reads as a build failure and is
# not one: the binary is there with `.exe` on the end. A check that fails for a reason unrelated to
# its subject is worse than one that does not run, so this says which it is.
case "$(uname -s 2>/dev/null || echo unknown)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    echo "This is the end-to-end run, and it needs Linux — a Unix socket, an atomic symlink swap"
    echo "and systemd (§23.5). \`skynetd\` refuses to serve on Windows and says so on start-up."
    echo "\`cargo test --workspace\` covers what is portable; run this under WSL or on a robot."
    exit 1
    ;;
esac

# A second supervisor on this machine invalidates the run, so refuse rather than produce failures
# that name the wrong thing.
#
# §33's unit names live in one user manager, so any other `skynetd` observes the units this script
# starts, finds them absent from its own desired state, and stops them — within a second, on a
# level-triggered loop. What that produces is not an obvious error: it is `systemctl show` printing
# the manager's defaults for a collected unit, which reads as ten confinement directives that did
# not land, and `safety.policy.isolation` skipping because no sandboxed unit is running. Both name
# the daemon, and the daemon is fine.
#
# The refusal is by name and by pid because "something else is running" is not actionable and
# "this pid, this root" is.
OTHERS=$(pgrep -a skynetd 2>/dev/null | grep -v "skynetd once" || true)
if [ -n "$OTHERS" ]; then
  echo "Another skynetd is already running on this machine:"
  echo "$OTHERS" | sed 's/^/  /'
  echo
  echo "§33's unit names are global to one user manager, so that daemon will stop the packages"
  echo "this run starts and the failures will name confinement rather than the second supervisor."
  echo "Stop it and run again. A robot has one skynetd; so does this script."
  exit 1
fi

rm -rf "$ROOT"
mkdir -p "$ROOT/pkg/bin" "$ROOT/data/state"

# ── A package ────────────────────────────────────────────────────────────────
#
# Two files and a manifest. The digest is computed by `skynet install` itself and checked against
# what we claim, so the claim has to be right — which is the point of the step.

cat > "$ROOT/pkg/bin/run" <<'PROGRAM'
#!/bin/sh
while true; do sleep 5; done
PROGRAM
chmod +x "$ROOT/pkg/bin/run"

cat > "$ROOT/pkg/manifest.json" <<'MANIFEST'
{"budget":{"cpu_cores":0.5,"ram_mb":64},"id":"openrobot/holder","isolation":"wasm","kind":"policy","runtime":{"entrypoint":"bin/run"},"schema_version":1,"version":"1.0.0"}
MANIFEST

say "1. A package is refused when its content does not hash to the digest asked for (§37.1)"
WRONG=$(printf 'b%.0s' $(seq 1 64))
"$SKYNET" install "$ROOT/pkg" --root "$ROOT" --id openrobot/holder --kind policy \
  --version 1.0.0 --digest "$WRONG" > "$ROOT/out" 2>&1 || true
want "the mismatch is refused, and names what it observed" "not the bytes that were published" "$ROOT/out"
[ -z "$(ls -A "$ROOT/data/packages/blobs" 2>/dev/null)" ] \
  && pass "and nothing was staged" || fail "something was staged anyway"

say "2. The same package, with the digest it actually has"
DIGEST=$(sed -n 's/.*hashes to \([0-9a-f]\{64\}\).*/\1/p' "$ROOT/out")
[ -n "$DIGEST" ] || { echo "could not read the digest from the refusal"; exit 1; }
"$SKYNET" install "$ROOT/pkg" --root "$ROOT" --id openrobot/holder --kind policy \
  --version 1.0.0 --digest "$DIGEST" > "$ROOT/out" 2>&1
want "staged" "staged openrobot/holder" "$ROOT/out"
# Two assertions rather than one, because the sentence this replaces changed under it and nobody
# noticed. §19's staging report used to say the signature was "not checked"; the trust store landed
# and it now names the state the signature is actually in — `absent`, `untrusted`, `invalid` or
# `verified` — and this line kept grepping for the old words. It has been failing since, which means
# the end-to-end run has been red on a wording change while everything it exists to check was fine.
#
# So: assert the state that is true here, and assert separately that nothing claims verification.
# The second half is the one that matters and it is the half a single grep for a phrase could lose
# again the next time somebody improves the phrasing.
want "the signature state is reported" "signature: absent" "$ROOT/out"
if grep -q "signature: verified" "$ROOT/out"; then
  fail "an unsigned package was reported as verified"
else
  pass "and nothing claims it was verified (§19)"
fi

say "3. Desired state, and one reconcile pass"
cat > "$ROOT/data/state/desired.json" <<DESIRED
{"generation":42,"os":{"channel":"stable","version":""},
 "packages":[{"id":"openrobot/holder","version":"1.0.0","digest":"$DIGEST",
              "kind":"policy","state":"running","grants":["manipulation.grasp"]}]}
DESIRED

"$SKYNETD" once --root "$ROOT" --user > "$ROOT/first" 2>&1 || true
want "the package is activated" "activate_package openrobot/holder" "$ROOT/first"
want "the grant is applied" "grant_capability openrobot/holder" "$ROOT/first"
want "starting is deferred until somebody declares a safe state (§33.4)" "waiting for a safe state" "$ROOT/first"

# An operator declares it. Nothing measured this — see `refresh_safe_state`.
echo yes > "$ROOT/data/state/safe-state"
"$SKYNETD" once --root "$ROOT" --user > "$ROOT/out" 2>&1 || true
[ -L "$ROOT/data/packages/active/openrobot/holder/current" ] \
  && pass "activation is a symlink (§37.1)" || fail "activation is not a symlink"

if command -v systemd-run >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  want "and it starts" "start_package openrobot/holder → applied" "$ROOT/out"
  # §59's confinement, read back off the real unit rather than the argv that requested it.
  # The suite asserts what `command_line` builds; only this asks systemd what landed — and a
  # launcher whose sandbox is only ever seen by systemd is a sandbox nobody has read. The
  # holder is the unit to read: it travels the same `command_line` as every other policy.
  #
  # `LoadState` is asked for with the directives, and it is not decoration. `systemctl show`
  # **exits 0 for a unit it has never heard of** and prints the manager's defaults for every
  # property named — every directive present, every one of them `no`, `infinity` or the full
  # capability set. So the ten checks below cannot tell *the sandbox did not land* from *there
  # is no unit*, and they read the second as the first: ten failures, one per directive, every
  # one of them naming something that was in fact applied. This ran red that way for a whole
  # session before anybody asked what the dump was.
  #
  # The comment here used to say the holder "runs for the whole script, so there is no race".
  # That is what the code believed and it is not true of a transient unit: `--collect` reaps it
  # the moment it goes inactive. So wait for it the way `start_daemon` waits for an answer
  # rather than for a socket file — and if it never loads, fail **once**, saying which.
  unit_load_state() {
    systemctl --user show "skynet:openrobot:holder.service" -p LoadState 2>/dev/null \
      | sed -n 's/^LoadState=//p'
  }
  i=0
  while [ "$(unit_load_state)" != "loaded" ] && [ $i -lt 50 ]; do
    i=$((i + 1)); sleep 0.1
  done
  LOADED="$(unit_load_state)"
  STARTED=1
else
  want "the start fails, saying it could not reach systemd" "start_package openrobot/holder → failed" "$ROOT/out"
  pass "no systemd here, so the launch step is reported rather than skipped"
  STARTED=0
  LOADED="no systemd"
fi

if [ "$LOADED" = "loaded" ]; then
  systemctl --user show "skynet:openrobot:holder.service" \
    -p NoNewPrivileges,ProtectSystem,ProtectKernelTunables,ProtectKernelModules,ProtectControlGroups,RestrictSUIDSGID,CapabilityBoundingSet,MemoryMax,TasksMax,AllowedCPUs \
    > "$ROOT/unit" 2>&1 || true
  want "no new privileges on the unit" "NoNewPrivileges=yes" "$ROOT/unit"
  want "a read-only OS tree on the unit" "ProtectSystem=yes" "$ROOT/unit"
  want "no sysctl writes on the unit" "ProtectKernelTunables=yes" "$ROOT/unit"
  want "no module loads on the unit" "ProtectKernelModules=yes" "$ROOT/unit"
  want "no cgroup writes on the unit (§40)" "ProtectControlGroups=yes" "$ROOT/unit"
  want "no setuid on the unit" "RestrictSUIDSGID=yes" "$ROOT/unit"
  # Line-exact: a substring match would pass `CapabilityBoundingSet=cap_anything`.
  grep -qx "CapabilityBoundingSet=" "$ROOT/unit" \
    && pass "and no capabilities on the unit" \
    || { fail "the unit kept capabilities:"; sed 's/^/      /' "$ROOT/unit"; }
  # `MemoryMax=64M` as sent arrives as bytes; the 64 is the declared `ram_mb` in §2's
  # manifest, so this asserts the budget became the limit, not merely that a limit exists.
  want "the declared budget became the limit" "MemoryMax=67108864" "$ROOT/unit"
  want "and the fixed task ceiling" "TasksMax=128" "$ROOT/unit"
  # Present and non-empty, with the value deliberately unpinned: the core set is the
  # machine's, and pinning it would fail the check on every other machine. What is
  # asserted is that no misconfiguration silently omitted it — an omitted cpuset is every
  # core, including the kernel's (§40.2).
  grep -q "^AllowedCPUs=[0-9]" "$ROOT/unit" \
    && pass "and the unit holds a real cpuset" \
    || { fail "no cpuset on the unit:"; sed 's/^/      /' "$ROOT/unit"; }
elif [ "$LOADED" != "no systemd" ]; then
  # The package started and there is no unit to read it off. One failure rather than ten, and it
  # says what happened rather than listing directives: a transient unit is collected when it goes
  # inactive, so this is the package having stopped between being started and being read — which
  # is silent about the confinement in both directions. Ten "directive missing" lines here would
  # be ten false sentences, which is worse than one true one.
  fail "the unit was started and is not there to read: LoadState=${LOADED:-<nothing>}. §59's
      confinement is neither confirmed nor refuted by this run — \`systemctl show\` prints the
      manager's defaults for a unit it has never heard of, and exits 0 doing it"
fi

# §33 calls `once` "a legitimate way to operate a robot", so its record has to be the same record.
# Until this session it was not: the daemon loop recorded every pass and `once` recorded none, so a
# robot driven entirely from a script had a black box holding `recording_started` and no evidence of
# anything it had ever done. The daemon has not started yet at this point in the script, so anything
# in the ring here was put there by `once` and by nothing else.
if grep -aq grant_capability "$ROOT/data/blackbox/ring"; then
  pass "\`once\` recorded its pass in the black box, with no daemon running (§13, §33)"
else
  fail "\`once\` reconciled and the black box holds nothing about it"
fi

say "4. The local socket, and the CLI over it (§33.7)"
DAEMON=""
trap kill_tracked EXIT
start_daemon
pass "the daemon answers on its socket"

SOCK="--socket $ROOT/run/skynetd.sock"

"$SKYNET" status $SOCK > "$ROOT/out" 2>&1
want "status answers with no credential at all (§39.1)" "stop: reachable" "$ROOT/out"
want "and says the robot is unprovisioned rather than pretending otherwise" "not provisioned" "$ROOT/out"
want "and says the safe state was declared, not measured" "declared" "$ROOT/out"

"$SKYNET" ps $SOCK > "$ROOT/out" 2>&1 || true
want "ps is refused on an unprovisioned robot (§39.2)" "401" "$ROOT/out"

"$SKYNET" estop $SOCK > "$ROOT/out" 2>&1
want "estop needs no credential, and says what it actually did" "never touches an actuator" "$ROOT/out"

say "5. A credential, issued by presence on the machine (§39.2)"
stop_daemon
TOKEN=$("$SKYNETD" credential add --root "$ROOT" --label suite --role operator 2>"$ROOT/out")
want "the token is printed once and only a hash is stored" "only a salted hash" "$ROOT/out"
# A second credential at the narrower role, so the checks that are *about* §39.1's line between
# observer and operator can be made with a credential that is actually on the observer side of it.
# An operator token proves a route is reachable; it cannot prove a route is not gated too widely.
OBSERVER=$("$SKYNETD" credential add --root "$ROOT" --label suite-observer --role observer 2>/dev/null)
grep -q "$TOKEN" "$ROOT/data/state/credentials.json"   && fail "the token itself is on disk" || pass "the token itself is not on disk"

start_daemon

"$SKYNET" ps $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
want "ps lists the package" "openrobot/holder" "$ROOT/out"
want "with the grant it was given" "manipulation.grasp" "$ROOT/out"

"$SKYNET" plan $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
pass "plan is answered"

"$SKYNET" grants $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
want "grants lists what is granted" "manipulation.grasp" "$ROOT/out"

"$SKYNET" grants add openrobot/holder sensing.camera.rgb $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
want "a grant can be added" "granted" "$ROOT/out"

# And then the reconciler takes it away again, because §11's desired state does not name it. This
# check asserted `revoked` and had been failing for as long as anybody had run this file: it removed
# the grant itself and raced the next pass, which had usually got there first and answered
# `not_held`. What was actually wrong was not the assertion — it is the two writers underneath it.
# A grant made at the robot survives exactly until the next pass, and the console offers a button
# that says `granted` about a permission that will not outlive the second.
#
# The convergence is §33.3 working as specified. What is asserted here is the half that is not a
# question: it has to leave a record. Until this round the API wrote `capability.grant` to the audit
# log and the reconciler's revocation went to the ring alone — minutes of retention for the taking
# away, permanent for the giving.
i=0
while [ $i -lt 60 ]; do
  grep -q "capability.revoke" "$ROOT/data/state/audit.jsonl" && break
  i=$((i + 1)); sleep 0.1
done
grep -q "capability.revoke" "$ROOT/data/state/audit.jsonl"   && pass "and the next pass revokes it, with an audit row saying so (§38.3)"   || fail "the reconciler took the grant away and left no audit row"
grep -q '"actor":"desired state' "$ROOT/data/state/audit.jsonl"   && pass "attributed to the desired state rather than to a person"   || fail "the revocation names a person who did not make it"
"$SKYNET" grants $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
grep -q "sensing.camera.rgb" "$ROOT/out"   && fail "the grant outlived the pass that should have converged it away"   || pass "and the capability is gone from what the robot reports"

# §15's `safety.policy.isolation`, run by the robot on itself against the unit systemd is
# actually running. This is the half §59 could not do: that section built the confinement and the
# suite asserted it at the argv layer, which proves what was *requested*. A directive systemd
# rejected, dropped, or does not support on this kernel produces exactly the same argv — so until
# the daemon read one back, the check was skipped and said so.
#
# It runs here rather than in the Rust suite because the subject is this machine. A conformance
# result computed from `confinement_properties` would be this repository asserting its own
# constant, which is what `safety.kernel.parity` is skipped for being.
"$SKYNET" conformance $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
if [ "$STARTED" = "1" ]; then
  grep -A3 "safety.policy.isolation" "$ROOT/out" | grep -q "pass" \
    && pass "the robot executes safety.policy.isolation against its own running unit (§15)" \
    || { fail "isolation did not pass on a confined unit:"; sed 's/^/      /' "$ROOT/out"; }
  grep -q "not taken from the argv" "$ROOT/out" \
    && pass "and says the directives were read back, not assumed" \
    || fail "the pass does not say what it measured"
  # The third rung, and the reason it is asserted separately: a pass that had only reached the
  # manager's record would still say "pass" here, and a check that could not tell the two apart
  # would go quiet on exactly the machine where `/proc` stopped answering.
  grep -q "the kernel agrees" "$ROOT/out" \
    && pass "and the kernel agrees about the two it exposes, read off /proc (§15)" \
    || { fail "the pass never reached the kernel:"; sed 's/^/      /' "$ROOT/out"; }
else
  grep -A3 "safety.policy.isolation" "$ROOT/out" | grep -q "skipped" \
    && pass "no systemd here, so isolation is skipped naming what is missing — never passed" \
    || fail "isolation was not skipped on a machine with no units"
fi

"$SKYNET" status $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
grep -q "not provisioned" "$ROOT/out" && fail "still reported unprovisioned"   || pass "the robot no longer reports itself unprovisioned"

say "5a. §38.2's session override, on a running robot"
# The fifth layer, which had no implementation at all until 2026-09-10 while §43's open list said
# "profiles layer cleanly on one robot". What is proved here is the thing no unit test can: that a
# request over the socket changes the number `GET /limits` serves, on a daemon that composed it
# from a descriptor and a profile without knowing this route exists.

"$SKYNET" limits $SOCK --token "$OBSERVER" > "$ROOT/before" 2>&1
pass "the limits before any override are readable"

"$SKYNET" session $SOCK --token "$OBSERVER" > "$ROOT/out" 2>&1
want "and there is no override to begin with" "no session override" "$ROOT/out"

# An observer may read it and may not set one. Checked with a credential that is actually on the
# observer side of §39.1's line, for the reason the credential above was issued.
"$SKYNET" session set --for 20m --velocity 0.25 $SOCK --token "$OBSERVER" > "$ROOT/out" 2>&1 || true
want "an observer is refused" "needs the operator role" "$ROOT/out"

"$SKYNET" session set --for 20m --velocity 0.25 --name wrist-harness --reason "replacing the harness" $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
want "an operator may set one" "wrist-harness" "$ROOT/out"
want "and is told how long is left" "20m left" "$ROOT/out"

"$SKYNET" limits $SOCK --token "$OBSERVER" > "$ROOT/after" 2>&1
grep -q "0.25" "$ROOT/after"   && pass "the ceiling the robot reports has moved (§7.6)"   || { fail "the override did not reach resolve_limits"; sed 's/^/      /' "$ROOT/after"; }
grep -q "0.25" "$ROOT/before"   && fail "the fixture proves nothing: it was already 0.25"   || pass "and it was not that before, so the check is about the override"

# §38.2's hazard is a robot that is *quietly* slow. This is the half that stops it being quiet, and
# it has to reach a surface that knows nothing about this layer — `skynet limits` renders notices.
grep -q "wrist-harness" "$ROOT/after"   && pass "and the robot says which override is holding it back (§38.2)"   || { fail "a slowed robot did not say why"; sed 's/^/      /' "$ROOT/after"; }

# The widening refusal is **not** checked here, and the reason is worth writing down rather than
# leaving as an absence: this robot has no declared envelope and no profile ceilings — `skynet
# limits` says "source: none cited" three lines up — so there is nothing above the session for it to
# widen. A high number set here is not a widening at all; it is the only ceiling this robot has, and
# going from *unbounded* to *99 rad/s* is a narrowing however alarming it looks written down.
#
# That is worth asserting as the property it is, because it is exactly the unprovisioned-bench case
# where this layer gets used and somebody reading `99` on a limits page should be able to find out
# why it is there.
"$SKYNET" session set --for 5m --velocity 99 $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
"$SKYNET" limits $SOCK --token "$OBSERVER" > "$ROOT/out" 2>&1
grep -q "99" "$ROOT/out"   && pass "on a robot with no envelope, the override is the only ceiling there is"   || fail "the override did not become the ceiling"
grep -q "session_widened" "$ROOT/out"   && fail "nothing was above it, so nothing was widened"   || pass "and nothing is reported as widened, because nothing was (§38.2)"
# The refusal itself needs a body with declared ceilings, so it is pinned where such a body exists:
# `skynet-kernel`'s suite, and six corpus cases held byte-identical across three editions.

# There is no way to ask for one that does not end.
"$SKYNET" session set --velocity 0.25 $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1 || true
want "an override with no duration is refused" "time-boxed" "$ROOT/out"
"$SKYNET" session set --for 30d --velocity 0.25 $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1 || true
pass "and a duration this CLI cannot parse is refused before it is sent"

# §38.3's two records, for the change that actually landed.
grep -q '"action":"session_override"' "$ROOT/data/state/audit.jsonl"   && pass "the change left an audit row (§38.3)"   || fail "a configuration change left no audit row"
grep -q '"previous"' "$ROOT/data/state/audit.jsonl"   && pass "carrying the value it replaced"   || fail "the audit row cannot say what changed"

"$SKYNET" session clear $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
pass "it can be cleared"
"$SKYNET" limits $SOCK --token "$OBSERVER" > "$ROOT/out" 2>&1
grep -q "session_override" "$ROOT/out"   && fail "the robot still says it is held back"   || pass "and the robot stops saying it is held back"

say "5b. The black box, on a disk (§13, §26, §28)"
[ -f "$ROOT/data/blackbox/ring" ] \
  && pass "the ring was preallocated at start" || fail "no ring at data/blackbox/ring"
RINGBYTES=$(wc -c < "$ROOT/data/blackbox/ring")
[ "$RINGBYTES" -gt 1000000 ] \
  && pass "and it is a real file rather than a hole ($RINGBYTES bytes)" \
  || fail "the ring is $RINGBYTES bytes"

"$SKYNET" blackbox $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
want "the index reports the window" "window" "$ROOT/out"
want "and says the cycle channels are empty" "no control loop" "$ROOT/out"
want "and that what it holds is complete" "complete   yes" "$ROOT/out"

"$SKYNET" blackbox seal $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
want "sealing names the file it wrote" ".mcap" "$ROOT/out"
SEALED=$(sed -n 's/^sealed  *\(.*\.mcap\)$/\1/p' "$ROOT/out")
[ -n "$SEALED" ] && pass "and the name came back" || fail "no name in the seal's output"
[ -f "$ROOT/data/blackbox/sealed/$SEALED" ] \
  && pass "and the file is on the robot" || fail "$SEALED is not in data/blackbox/sealed"

# The magic MCAP puts at both ends of a file. `od` rather than `grep`, because the first byte is
# 0x89 and this is a binary file: a text tool would decide it had found no matches in a file with
# no lines.
HEAD=$(od -An -tx1 -N8 "$ROOT/data/blackbox/sealed/$SEALED" | tr -d ' \n')
[ "$HEAD" = "894d434150300d0a" ] \
  && pass "and it begins with the MCAP magic (§26)" || fail "it begins $HEAD"

(cd "$ROOT" && "$SKYNET" blackbox get "$SEALED" $SOCK --token "$TOKEN") > "$ROOT/out" 2>&1
[ -f "$ROOT/$SEALED" ] && pass "and it downloads over the socket" || fail "the download produced no file"
cmp -s "$ROOT/$SEALED" "$ROOT/data/blackbox/sealed/$SEALED" \
  && pass "byte for byte" || fail "the downloaded file differs from the one on disk"

(cd "$ROOT" && "$SKYNET" blackbox get "../../etc/shadow" $SOCK --token "$TOKEN") > "$ROOT/out" 2>&1 || true
want "a path in the name is refused" "not a sealed recording" "$ROOT/out"

# §41.3's events. The daemon's own ring holds events and no cycles, so this half checks the list
# exists and reports its denominator; the cycle exclusion is checked in section 12, against the
# kernel host's ring, which is the only ring on this machine that has cycles in it.
"$SKYNET" blackbox events $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
want "the event list answers" "in the ring" "$ROOT/out"
want "and names an event this run produced" "recording_started" "$ROOT/out"

# The route must not be taken for a file name. It sits above the download arm in both the router
# and the authorisation table, and if it ever stops doing so this asks for `events` with an
# observer credential and gets a 403 for the operator role the download needs.
"$SKYNET" blackbox events $SOCK --token "$OBSERVER" > "$ROOT/out" 2>&1
want "an observer may read the events (§39.1)" "in the ring" "$ROOT/out"

# §13's other half: reading a sealed recording back, with no robot involved. The file downloaded a
# few lines above is the input, so this is the whole round trip — a robot sealed it, a workstation
# opened it — and it runs against the *downloaded copy* rather than the one on the robot's disk,
# because the copy is what a support conversation actually has.
(cd "$ROOT" && "$SKYNET" blackbox inspect "$SEALED") > "$ROOT/out" 2>&1
want "a sealed recording can be opened off a robot" "provenance" "$ROOT/out"
want "and says whether it is complete (§26.3)" "complete" "$ROOT/out"
want "and names the topics it holds" "/sk/event" "$ROOT/out"
want "and does not claim to have replayed it" "other question" "$ROOT/out"
# Each topic once. MCAP repeats the channels in the summary, and a reader that counted both would
# report every topic twice — which is what it did until a real recording was put through it.
EVENTLINES=$(grep -c "/sk/event" "$ROOT/out")
[ "$EVENTLINES" -eq 1 ] \
  && pass "each topic once, not once per section" || fail "/sk/event appears $EVENTLINES times"

# A file that is not a recording, and one that stops early. Two different answers, because a
# truncated transfer is resumable (§79) and a damaged file is not.
printf 'not an mcap file' > "$ROOT/notmcap"
(cd "$ROOT" && "$SKYNET" blackbox inspect "$ROOT/notmcap") > "$ROOT/out" 2>&1 || true
want "a file that is not a recording says so" "not begin with MCAP" "$ROOT/out"
head -c 400 "$ROOT/data/blackbox/sealed/$SEALED" > "$ROOT/cut.mcap"
(cd "$ROOT" && "$SKYNET" blackbox inspect "$ROOT/cut.mcap") > "$ROOT/out" 2>&1 || true
want "and a cut one reads as a transfer that stopped" "ends in the middle" "$ROOT/out"

say "5c. §41.3's bundle, against a running robot"
# The command a support conversation starts with. Run here rather than in a unit test because what
# it is *for* is aggregating eleven live routes, and a doctor over doubles would be a doctor that
# had never met a 503 or a 403 — which are the two answers it exists to record faithfully.
(cd "$ROOT" && "$SKYNET" doctor $SOCK --token "$TOKEN" --out "$ROOT/doctor.json") > "$ROOT/out" 2>&1
want "the summary leads with §27's state" "state" "$ROOT/out"
want "and names the version it is talking to" "skynetd" "$ROOT/out"
want "and says how many sections it read" "sections read" "$ROOT/out"
[ -f "$ROOT/doctor.json" ] && pass "the bundle is a file" || fail "no bundle at $ROOT/doctor.json"
grep -q '"schema":"skynet.doctor.v1"' "$ROOT/doctor.json" \
  && pass "and it names its schema" || fail "the bundle has no schema"
# Every section is present whatever happened to it. A bundle that dropped the refusals would be one
# that erased its two most interesting findings, which is the rule the module is built on.
SECTIONS=$(grep -o '"section":' "$ROOT/doctor.json" | wc -l)
[ "$SECTIONS" -eq 12 ] \
  && pass "and carries all 12 sections" || fail "the bundle carries $SECTIONS sections"
grep -q '"route":"GET /limits"' "$ROOT/doctor.json" \
  && pass "including §7.6's guards and limits" || fail "no limits section"
grep -q '"route":"GET /blackbox/events"' "$ROOT/doctor.json" \
  && pass "and §41.3's events" || fail "no events section"

# Without a credential the observer routes still answer and the rest are recorded as refused. That
# is a *readable* bundle rather than a failed run, and it is how somebody with no token gets a file
# worth sending — so it is asserted rather than assumed.
(cd "$ROOT" && "$SKYNET" doctor $SOCK --out "$ROOT/doctor-anon.json") > "$ROOT/out" 2>&1
want "an uncredentialed run still writes a bundle" "sections read" "$ROOT/out"
want "and says which sections were refused" "unread" "$ROOT/out"
grep -q 'this needs the' "$ROOT/doctor-anon.json" \
  && pass "recording the role each refusal wanted" || fail "no refusal reason in the anonymous bundle"

say "6. The console refuses a routable address (§34.2)"
"$SKYNETD" --root "$ROOT/other" --lan 0.0.0.0:8099 > "$ROOT/out" 2>&1 || true
want "0.0.0.0 is refused" "34.2" "$ROOT/out"

stop_daemon

# ── §82: a robot answers a browse ──────────────────────────────
#
# The one check in this file that needs a network stack rather than a socket file, and the reason
# it is here rather than in the suite: the responder was written with unit tests that passed while
# it did the wrong thing on a link, twice. A browse is the only thing that asks the question an
# operator asks.
#
# Port 5353 is held permanently by Avahi and by mDNSResponder, so on a developer machine the
# daemon reports that it could not bind and this step reports that it did not run. Where the port
# is free the browse is *required* to find the robot: a check that quietly does not run is the
# failure §15 spends a rule on.
say "7. A robot answers a browse (§82)"
rm -rf "$ROOT/mdns" && mkdir -p "$ROOT/mdns"
# Before the daemon, because it reads its credentials once at start — the same ordering §5 above
# uses, and the reason the shutdown below can authenticate at all.
MTOKEN=$("$SKYNETD" credential add --root "$ROOT/mdns" --label bye --role admin 2>/dev/null)
"$SKYNETD" --root "$ROOT/mdns" --user --interval 1000 --lan 127.0.0.1:8099 \n  --advertise skynet-e2e > "$ROOT/mdns.log" 2>&1 &
MDNS=$!
track "$MDNS"
# Wait for the *claim*, not for the start-up line. The daemon prints "announcing ..." before it has
# probed, and a browse sent in that window is a browse the robot is right not to answer — which
# would have made this check a race rather than a test.
i=0
while [ $i -lt 60 ]; do
  grep -q "skynetd: mDNS" "$ROOT/mdns.log" 2>/dev/null && break
  i=$((i + 1)); sleep 0.1
done
if grep -q "no mDNS socket could be bound" "$ROOT/mdns.log"; then
  printf '  [2m- port 5353 is held on this machine; the browse step did not run[0m
'
else
  want "the daemon takes its name" "advertising" "$ROOT/mdns.log"
  "$SKYNET" discover --timeout 2500 > "$ROOT/browse" 2>&1 || true
  want "a browse finds it" "skynet-e2e" "$ROOT/browse"
  want "and points at the console rather than at 5353" "8099" "$ROOT/browse"
fi
# ── §82.4's goodbye, which had no caller for eleven rounds ───────────────────
#
# Asked properly rather than signalled. `skynetd` is stopped by a signal and handling one needs
# `unsafe`, which its crate forbids — so a robot switched off cleanly stayed in every browser's
# cache until the 120-second TTL expired. `POST /shutdown` is the path across, and the unit file's
# `ExecStop=` is what calls it on a real robot.
#
# The daemon must **exit on its own**: nothing here sends a signal, so a process still running
# afterwards is a shutdown path that does not work rather than one that is slow.
say "7b. The daemon can be asked to stop, and says goodbye on the way (§82.4, §13)"
"$SKYNET" shutdown --socket "$ROOT/mdns/run/skynetd.sock" --token "$MTOKEN" > "$ROOT/bye" 2>&1 || true
want "the shutdown is accepted, and says it stops the supervisor not the robot"   "not the robot" "$ROOT/bye"

i=0
while [ $i -lt 100 ]; do
  kill -0 "$MDNS" 2>/dev/null || break
  i=$((i + 1)); sleep 0.1
done
if kill -0 "$MDNS" 2>/dev/null; then
  fail "the daemon was asked to stop and is still running:"
  tail -5 "$ROOT/mdns.log" | sed 's/^/      /'
else
  pass "and the daemon exits on its own, with nothing signalled (§82.4)"
fi

# §13, and the reason this needed no new event code: `Reason::Shutdown` already exists for exactly
# this, and sealing writes §26's `sealed` event — so a clean stop is *in* the recording rather than
# being the moment it goes quiet.
want "it sealed the window, naming the reason" "reason shutdown" "$ROOT/mdns.log"
want "and said so plainly" "stopped." "$ROOT/mdns.log"
if [ -n "$(ls -A "$ROOT/mdns/data/blackbox/sealed" 2>/dev/null)" ]; then
  pass "and the sealed window is on the robot"
else
  fail "nothing was sealed into data/blackbox/sealed"
fi

# The withdrawal itself only happens where a responder was running. On a developer machine Avahi
# holds 5353, so there is no name to take back and none to give up — reported rather than asserted,
# for the same reason the browse above is.
if grep -q "no mDNS socket could be bound" "$ROOT/mdns.log"; then
  printf '  [2m- no responder bound here, so there was no name to withdraw[0m
'
else
  want "and it withdrew its mDNS name" "withdrew" "$ROOT/mdns.log"
fi

kill "$MDNS" 2>/dev/null || true
wait "$MDNS" 2>/dev/null || true

say "8. Rollback (§37.2)"
sed -i 's/"version":"1.0.0"/"version":"2.0.0"/' "$ROOT/pkg/manifest.json"
"$SKYNET" install "$ROOT/pkg" --root "$ROOT" --id openrobot/holder --kind policy \
  --version 2.0.0 --digest 0000000000000000000000000000000000000000000000000000000000000000 \
  > "$ROOT/out" 2>&1 || true
SECOND=$(sed -n 's/.*hashes to \([0-9a-f]\{64\}\).*/\1/p' "$ROOT/out")
"$SKYNET" install "$ROOT/pkg" --root "$ROOT" --id openrobot/holder --kind policy \
  --version 2.0.0 --digest "$SECOND" > "$ROOT/out" 2>&1
sed -i "s/\"digest\":\"$DIGEST\"/\"digest\":\"$SECOND\"/;s/\"version\":\"1.0.0\"/\"version\":\"2.0.0\"/" \
  "$ROOT/data/state/desired.json"
"$SKYNETD" once --root "$ROOT" --user > "$ROOT/out" 2>&1 || true
CURRENT=$(readlink "$ROOT/data/packages/active/openrobot/holder/current" | sed 's#.*/##')
[ "$CURRENT" = "$SECOND" ] && pass "upgraded to the new digest" || fail "still on $CURRENT"
PREV=$(readlink "$ROOT/data/packages/active/openrobot/holder/prev.1" | sed 's#.*/##')
[ "$PREV" = "$DIGEST" ] && pass "and the previous version is pinned for rollback" || fail "prev.1 is $PREV"
[ -d "$ROOT/data/packages/blobs/$DIGEST" ] \
  && pass "its blob survived collection (§37.3)" || fail "the rollback target was collected"

say "9. A skill starts its policy, on the ordinary path (§54.4)"
mkdir -p "$ROOT/skillpkg/bin"
cat > "$ROOT/skillpkg/bin/run" <<PROGRAM
#!/bin/sh
# The skynetp-in-a-blob pattern: the package carries its entrypoint, the robot carries the
# policy host, and the rendezvous (SKYNET_RING_SOCKET) arrives in the environment when a
# runtime offers one. No runtime runs in this script — no skynetk, no driver — so the hold
# backend publishes into a ring that is private to the process, for a bounded 60 seconds,
# and exits cleanly. The touch is the whole of the proof below: a file only the unit's own
# process can write, so its presence means the daemon really started this package on this
# machine rather than reporting that it did. The 60-second bound is the other half of the
# honesty: a demo that hangs on a failure would leave a stray unit on every robot it ran on.
touch "$ROOT/skill-ran" 2>/dev/null || true
exec "$BIN/skynetp" run --hz 10 --cycles 600 --backend hold --quiet
PROGRAM
chmod +x "$ROOT/skillpkg/bin/run"
cat > "$ROOT/skillpkg/manifest.json" <<'MANIFEST'
{"budget":{"cpu_cores":0.5,"ram_mb":64},"expected_duration_seconds":120,"id":"acme/wave","isolation":"sandboxed","kind":"skill","runtime":{"entrypoint":"bin/run"},"schema_version":1,"version":"1.0.0"}
MANIFEST
"$SKYNET" install "$ROOT/skillpkg" --root "$ROOT" --id acme/wave --kind skill \
  --version 1.0.0 --digest 0000000000000000000000000000000000000000000000000000000000000000 \
  > "$ROOT/out" 2>&1 || true
SKILLDIGEST=$(sed -n 's/.*hashes to \([0-9a-f]\{64\}\).*/\1/p' "$ROOT/out")
[ -n "$SKILLDIGEST" ] || { echo "could not read the skill digest from the refusal"; exit 1; }
"$SKYNET" install "$ROOT/skillpkg" --root "$ROOT" --id acme/wave --kind skill \
  --version 1.0.0 --digest "$SKILLDIGEST" > "$ROOT/out" 2>&1
want "the skill package stages" "staged acme/wave" "$ROOT/out"

# Desired state names it as installed but not running: the Hub wants the skill present and
# the invocation decides when it runs. Without this entry a pass would collect the
# activation (§37.3) before the skill ever starts — and `"state":"installed"` never starts
# it, because anything but `"running"` parses as stopped.
cat > "$ROOT/data/state/desired.json" <<DESIRED
{"generation":43,"os":{"channel":"stable","version":""},
 "packages":[{"id":"openrobot/holder","version":"2.0.0","digest":"$SECOND",
              "kind":"policy","state":"running","grants":["manipulation.grasp"]},
             {"id":"acme/wave","version":"1.0.0","digest":"$SKILLDIGEST",
              "kind":"skill","state":"installed","grants":[]}]}
DESIRED
"$SKYNETD" once --root "$ROOT" --user > "$ROOT/out" 2>&1 || true
want "the skill package is activated" "activate_package acme/wave" "$ROOT/out"
if grep -q "start_package acme/wave" "$ROOT/out"; then
  fail "desired state says installed, not running, and the skill started anyway"
else
  pass "and desired state alone does not start it"
fi

start_daemon
"$SKYNET" skill run acme/wave $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
want "the invocation is accepted, not started" "Accepted, not started" "$ROOT/out"

# idle → preconditions → acquiring → running is three passes at a one-second interval. Poll
# for the phase rather than sleeping a fixed span: a fixed sleep is a race on a loaded
# machine and a wait on a fast one.
i=0
while [ $i -lt 40 ]; do
  "$SKYNET" skills $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1 || true
  if grep "acme/wave" "$ROOT/out" | grep -q "running"; then break; fi
  i=$((i + 1)); sleep 0.5
done
if grep "acme/wave" "$ROOT/out" | grep -q "running"; then
  pass "the invocation reaches running"
else
  fail "the invocation never reached running:"; sed 's/^/      /' "$ROOT/out"
fi

# Steady running is not asserted here, and the reason is the design rather than the
# script: this robot has no kernel — no skynetk runs in this script — so the daemon's
# operating state is `safe`, and a skill that stayed in `running` on a robot that cannot
# command would be the interlock failing, not the demo working. What is asserted instead
# is both halves of the honest outcome: the unit ran, and the lifecycle refused to command
# through it. (The spare clause that keeps the reconciler from killing skill units has its
# proof in the suite, where a kernel-less robot does not confuse the question.)
if command -v systemd-run >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  # The trace file the entrypoint touches: presence means the daemon really started this
  # package on this machine. Poll rather than sleep — systemd starts units in milliseconds
  # and the 60-second `--cycles` bound means a failure cannot strand the wait.
  i=0
  while [ $i -lt 40 ]; do
    [ -f "$ROOT/skill-ran" ] && break
    i=$((i + 1)); sleep 0.5
  done
  [ -f "$ROOT/skill-ran" ] \
    && pass "its unit ran on this robot" \
    || fail "the unit never ran — the start was reported, not performed"
  i=0
  while [ $i -lt 40 ]; do
    "$SKYNET" skills $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1 || true
    if grep "acme/wave" "$ROOT/out" | grep -q "terminal"; then break; fi
    i=$((i + 1)); sleep 0.5
  done
  if grep "acme/wave" "$ROOT/out" | grep -q "terminal"; then
    pass "and the invocation lands in terminal"
  else
    fail "the invocation never landed:"; sed 's/^/      /' "$ROOT/out"
  fi
  want "with no kernel it refuses to command, and names the reason" "robot_stopped" "$ROOT/out"
  [ -n "$(ls -A "$ROOT/data/state/outbound" 2>/dev/null)" ] \
    && pass "and the episode it recorded is queued (§34.4, §54.4)" \
    || fail "terminal, but the outbound queue holds no episode"
else
  # No systemd here, so the start is refused loudly rather than run — the same contract §3
  # asserts for a desired package. The lifecycle then reads a skill in `running` with nothing
  # running as a skill that finished, and lands in `terminal` on its own.
  grep -q "would not start" "$ROOT/daemon.log" \
    && pass "the start fails, saying what it could not reach" \
    || fail "no loud refusal in the daemon log"
  [ -f "$ROOT/skill-ran" ] \
    && fail "refused, yet something ran anyway" \
    || pass "and nothing ran"
  i=0
  while [ $i -lt 40 ]; do
    "$SKYNET" skills $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1 || true
    if grep "acme/wave" "$ROOT/out" | grep -q "terminal"; then break; fi
    i=$((i + 1)); sleep 0.5
  done
  if grep "acme/wave" "$ROOT/out" | grep -q "terminal"; then
    pass "and the invocation lands in terminal anyway"
  else
    fail "the invocation never landed:"; sed 's/^/      /' "$ROOT/out"
  fi
fi
stop_daemon

# ── §25 and §27: the loop turns, and the daemon reads it off §13's ring ──────
#
# A robot of its own, and not because isolation is tidy. Everything above runs against a robot with
# **no embodiment descriptor**, so `shape::supervising` answers `NoBody` before it reads a manifest
# and the ring is events-only — which is why every check above sees "no control loop". Sizing a
# ring for a body needs a descriptor *and* a package that declares an action space, and giving the
# robot above one would change what §27 derives underneath forty checks written about a robot with
# nothing commanding it.
#
# What this section is for: `skynetk` records cycles, `skynetd` reads them, and two of §27's eight
# conditions stop being defaults. Both halves have been true in the suite for two rounds against a
# ring one process opened. This is the first time they are two processes on one recording, which is
# §26.2's whole subject, and the first time the count on the route is a count a robot produced.
say "10. The kernel turns, and §27 stops deciding from one condition (§25, §26.2, §27)"

CC="${CC:-}"
if [ -z "$CC" ]; then
  for candidate in cc gcc clang; do
    if command -v "$candidate" >/dev/null 2>&1; then CC="$candidate"; break; fi
  done
fi

if [ -z "$CC" ]; then
  # Reported, never skipped silently — §15's rule, and the same shape as the systemd step above.
  printf '  \033[2m- no C compiler (tried cc, gcc, clang), so the mock robot could not be built and\033[0m\n'
  printf '  \033[2m  this section did not run. Set CC.\033[0m\n'
else
  K="$ROOT/kernel"
  rm -rf "$K" && mkdir -p "$K/data/state" "$K/pkg/bin"
  if "$CC" -std=c11 -O2 -fPIC -shared -I bindings/c -o "$K/mock_robot.so" \
      drivers/mock/mock_robot.c -lm 2>"$K/cc.log"; then
    pass "the mock robot compiles ($CC)"
  else
    fail "the mock robot did not compile:"; sed 's/^/      /' "$K/cc.log"
  fi

  # The robot's own descriptor (§5), which on a real machine §50 writes at the factory. Three
  # revolute joints, matching the body `drivers/mock` presents — and matching is the point: the
  # daemon sizes the ring from *this* file and the loop sizes its records from the *driver*, and
  # `skynet-kernel-host` compares the two with `!=` and refuses a mismatch by name. A descriptor
  # that disagreed with the driver would fail loudly here rather than record cycles about the
  # wrong machine.
  cat > "$K/data/state/embodiment.json" <<'DESCRIPTOR'
{"schema_version":1,"robot_slug":"mock-arm","digest":null,"verified":false,
 "base":"fixed","dof":3,
 "joints":[
  {"name":"J1","role":"right_arm.shoulder.pitch","type":"revolute",
   "limits":{"position_min_rad":-3.14,"position_max_rad":3.14,"velocity_max_rad_s":2,"effort_max_nm":80},
   "control_modes":["position","torque"]},
  {"name":"J2","role":"right_arm.elbow.flex","type":"revolute",
   "limits":{"position_min_rad":-2.6,"position_max_rad":2.6,"velocity_max_rad_s":2,"effort_max_nm":60},
   "control_modes":["position","torque"]},
  {"name":"J3","role":"right_arm.wrist.roll","type":"continuous",
   "limits":{"position_min_rad":-6.28,"position_max_rad":6.28,"velocity_max_rad_s":4,"effort_max_nm":20},
   "control_modes":["position"]}],
 "end_effectors":[],"sensors":[],
 "control":{"modes":["position","torque"],"rate_hz_max":1000,"rate_hz_min":10,"command_latency_ms":1},
 "envelope":null,"solvers":[]}
DESCRIPTOR
  echo yes > "$K/data/state/safe-state"

  # A **commanding** package: one whose manifest declares a `source_embodiment` (§6). That block is
  # the only thing that makes it commanding — `kind: policy` does not, which is why the holder above
  # sizes nothing. Its three action channels bind to J1/J2/J3 by exact joint name, so §6.2's plan
  # resolves `direct` and its `bindings` are the channel count the ring is sized from.
  cat > "$K/pkg/bin/run" <<'PROGRAM'
#!/bin/sh
while true; do sleep 5; done
PROGRAM
  chmod +x "$K/pkg/bin/run"
  cat > "$K/pkg/manifest.json" <<'MANIFEST'
{"budget":{"cpu_cores":0.5,"ram_mb":64},"digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","id":"openrobot/reach","isolation":"sandboxed","kind":"policy","runtime":{"entrypoint":"bin/run"},"schema_version":1,"source_embodiment":{"robot_slug":"mock-arm","dof":3,"control_mode":"position","control_hz":200,"action_channels":[{"index":0,"kind":"joint_position","joint":"J1","unit":"rad","peak":3.14},{"index":1,"kind":"joint_position","joint":"J2","unit":"rad","peak":2.6},{"index":2,"kind":"joint_position","joint":"J3","unit":"rad","peak":6.28}],"observations":[]},"version":"1.0.0"}
MANIFEST

  "$SKYNET" install "$K/pkg" --root "$K" --id openrobot/reach --kind policy --version 1.0.0 \
    --digest 0000000000000000000000000000000000000000000000000000000000000000 > "$K/out" 2>&1 || true
  KDIGEST=$(sed -n 's/.*hashes to \([0-9a-f]\{64\}\).*/\1/p' "$K/out")
  [ -n "$KDIGEST" ] || { echo "could not read the policy digest from the refusal"; exit 1; }
  "$SKYNET" install "$K/pkg" --root "$K" --id openrobot/reach --kind policy --version 1.0.0 \
    --digest "$KDIGEST" > "$K/out" 2>&1
  want "the commanding package stages" "staged openrobot/reach" "$K/out"

  # `installed`, not `running`: this section is about the black box and §27, and starting the
  # package would ask systemd for a unit that has nothing to do with either.
  cat > "$K/data/state/desired.json" <<DESIRED
{"generation":1,"os":{"channel":"stable","version":""},
 "packages":[{"id":"openrobot/reach","version":"1.0.0","digest":"$KDIGEST",
              "kind":"policy","state":"installed","grants":[]}]}
DESIRED
  "$SKYNETD" once --root "$K" --user > "$K/first" 2>&1 || true
  want "and is activated" "activate_package openrobot/reach" "$K/first"

  # The credential before the daemon, because the daemon reads them at start.
  KTOKEN=$("$SKYNETD" credential add --root "$K" --label kernel --role operator 2>/dev/null)

  rm -f "$K/run/skynetd.sock"
  "$SKYNETD" --root "$K" --user --interval 500 > "$K/d.log" 2>&1 &
  KDAEMON=$!
  track "$KDAEMON"
  i=0
  while [ $i -lt 60 ]; do
    "$SKYNET" status --socket "$K/run/skynetd.sock" >/dev/null 2>&1 && break
    i=$((i + 1)); sleep 0.1
  done
  KSOCK="--socket $K/run/skynetd.sock"

  # §26.3, and the ordering a robot actually boots in: the supervisor starts first and therefore
  # *creates* the recording, so it has to size it for a body it will never touch. Both numbers come
  # off the robot's own provisioning — joints from the descriptor, channels from the transport plan.
  want "the daemon sizes the ring from the descriptor and the plan (§26.3, §80)" \
    "3 joints and 3 channels" "$K/d.log"

  # Before the loop, and this is the distinction the whole condition is built on. The ring here
  # **can** hold a control cycle — the daemon just sized it for three channels — so silence from it
  # is evidence rather than an absence, and §27's `kernel` is therefore *observed* and *down*. On
  # every other robot in this script the same silence is `Unobservable`, because a ring with no
  # action channels says nothing about a kernel. Same reading, opposite meaning, and the ring's
  # shape is what separates them.
  "$SKYNET" state $KSOCK --token "$KTOKEN" > "$K/before" 2>&1
  want "before the loop the kernel is observed and down, not unobserved" \
    "no control cycle was recorded" "$K/before"
  want "and the RHAL is lost, because nothing is reading the body" "rhal        lost" "$K/before"
  want "which is what holds the robot safe (§27)" "rhal_lost" "$K/before"

  # The loop. A second writer on the ring the daemon created: §26.2's shared sequence counter is
  # what makes that safe, and a refusal here is the lock working rather than a flake.
  "$SKYNETK" run --driver "$K/mock_robot.so" --tier host --velocity-max 2.0 --jerk 0.1 \
    --watchdog-ms 50 --hz 200 --cycles 6000 --channels J1,J2,J3 \
    --blackbox "$K/data/blackbox" > "$K/k.log" 2>&1 &
  KLOOP=$!
  track "$KLOOP"

  # Poll for the condition rather than sleeping a fixed span: the daemon re-reads the ring once an
  # interval, and a fixed sleep is a race on a loaded machine and a wait on a fast one.
  #
  # Poll for the **state**, never for the count. `observed` is true the moment the ring can hold a
  # cycle, so a poll that waited for "three of §27's eight conditions" would be satisfied before
  # the loop had written anything — and every assertion after it would then be sampling a robot
  # whose kernel had not started. The first draft did exactly that and produced a `state` document
  # reporting a live RHAL beside a silent kernel, which §27 says can never be true of one instant.
  i=0
  while [ $i -lt 60 ]; do
    "$SKYNET" state $KSOCK --token "$KTOKEN" > "$K/after" 2>&1 || true
    grep -q "control cycles were recorded" "$K/after" && break
    i=$((i + 1)); sleep 0.25
  done

  if grep -q "already has a writer" "$K/k.log"; then
    fail "the loop was refused the ring:"; sed 's/^/      /' "$K/k.log"
  else
    pass "the loop attaches to the ring the daemon created (§26.2)"
  fi

  # **All eight**, and this is the line that says §27's sequence is finished rather than advanced.
  # Four come off the same newest cycle as the RHAL — §24's flags for `stop` and `homing`, the
  # kernel's guard verdict for `guard_hold` — so a ring that can hold a cycle makes them observable
  # at the moment the loop writes into it. `guards` needs no ring at all: §7.6's partition resolves
  # off the descriptor this package staged. `post` is this daemon's own, and `policy` is the store's.
  #
  # `homing` is the newest, and it is the only one that depends on the **driver** rather than on
  # the ring being readable. A driver that had not been updated would leave §24's field zero and
  # this count would still be eight — `unreported` is an observation, which is the whole point of
  # the two-bit encoding — so the line after it asserts the answer rather than only the count.
  want "and §27 now counts all eight observed conditions" \
    "eight of §27's eight conditions" "$K/after"
  # The mock reports `SKR_HOMING_NOT_REQUIRED`: a simulated body's joint positions *are* the body,
  # so there is no encoder zero to lose and no homing routine for an operator to run. Reporting
  # `complete` would claim one had been run.
  want "and the homing state is the driver's answer rather than a default" \
    "needs no homing" "$K/after"
  want "the kernel is live, with the cycles that say so" "control cycles were recorded" "$K/after"
  want "and the RHAL is present, off the newest cycle" "read the body and reported it present" "$K/after"
  # The guard hold, on a loop nothing is holding: the guards ran and none of them withheld the
  # command. Asserting the *negative* is worth a line because the failure it guards against is a
  # console that renders "held" for a robot that is running, and because it is the sentence a
  # reader gets on every robot that has not tripped anything.
  want "and no guard is holding the command" "none of them held the command" "$K/after"
  # §27's guard set, off §7.6's partition rather than the ring. This body declares no IMU and no
  # person detector, so the assertion is the *positive* one — guards are unavailable and the
  # condition says which and why — rather than a hopeful "all available" that would prove nothing.
  want "and the guard set is partitioned, naming what each guard wants" \
    "unavailable:" "$K/after"
  # §27's policy, off the store rather than off §25.2's lease table. A commanding package is
  # installed and activated by this point, so the assertion is that the daemon says so — the
  # sentence `no_policy` would have been false here, and was, on every robot until this round.
  want "and the policy condition names the package rather than reporting nothing to run" \
    "openrobot/reach" "$K/after"
  # Both off one interval and one set of records. A robot reporting a present driver beside a
  # silent kernel would be describing a pair that was true at no instant, which is the reason
  # §27 reads them together rather than on two routes.
  grep -q "no control cycle was recorded" "$K/after" \
    && { fail "the kernel and the RHAL disagree about the same interval:"; sed 's/^/      /' "$K/after"; } \
    || pass "and the two readings agree about the interval they share (§27)"

  # The half that matters most, and the reason this is not merely a nicer status line. §27 makes
  # `SAFE` **sticky**: it takes a named human clearance to leave, never a condition improving. A
  # robot that started in `safe` with `rhal_lost` and quietly began commanding because a driver
  # started answering would be the single worst defect this machine could have.
  if grep -q "safe" "$K/after"; then
    pass "and the robot is still safe: a sticky state is not released by a better reading (§27)"
  else
    fail "the robot left safe without a clearance:"; sed 's/^/      /' "$K/after"
  fi

  # And the daemon's own view of the recording, which is where §74's counters read from.
  "$SKYNET" blackbox $KSOCK --token "$KTOKEN" > "$K/bb" 2>&1
  if grep -q "no control loop" "$K/bb"; then
    fail "the index still says the cycle channels are empty:"; sed 's/^/      /' "$K/bb"
  else
    pass "the index no longer says the cycle channels are empty"
  fi
  CYCLES=$(sed -n 's/^cycles *\([0-9]*\).*/\1/p' "$K/bb" | head -1)
  if [ -n "$CYCLES" ] && [ "$CYCLES" -gt 0 ]; then
    pass "and it counts $CYCLES cycles a different process wrote (§26.2)"
  else
    fail "the index counts no cycles:"; sed 's/^/      /' "$K/bb"
  fi

  kill "$KLOOP" 2>/dev/null || true
  wait "$KLOOP" 2>/dev/null || true

  # ── §27's stop, off §24's flags on the same records ───────────────────────
  #
  # The mock driver latches its estop from the first read when it is opened with `fault=estop`, so
  # this is the chain being open on a real driver rather than a flag a test wrote. What it proves is
  # the decision of 2026-09-07 end to end: an open chain is `estop` and never `operator` — §24 gives
  # the chain one bit and a light curtain opens the same chain as the button — and it never becomes
  # a *completed* stop, because `stop_settled` is not observable from that bit.
  "$SKYNETK" run --driver "$K/mock_robot.so" --config "fault=estop" --tier host \
    --velocity-max 2.0 --jerk 0.1 --watchdog-ms 50 --hz 200 --cycles 6000 --channels J1,J2,J3 \
    --blackbox "$K/data/blackbox" > "$K/k2.log" 2>&1 &
  KSTOP=$!
  track "$KSTOP"

  i=0
  while [ $i -lt 90 ]; do
    "$SKYNET" state $KSOCK --token "$KTOKEN" > "$K/estop" 2>&1 || true
    grep -q "chain is open" "$K/estop" && break
    i=$((i + 1)); sleep 0.25
  done

  want "the open chain is observed off §24's flags" "chain is open on the newest cycle" "$K/estop"
  want "and says what it cannot know about it" "not knowable from here" "$K/estop"
  # The robot does **not** move to `stopping`, and asserting that is the point rather than an
  # inconvenience. It was already `safe` — nothing had read its body when the daemon started — and
  # §27 makes `SAFE` sticky: it takes a named human clearance to leave, never a condition changing.
  # A stop arriving at a robot that is already safe cannot make it less safe, and the first draft of
  # this check asserted `stopping_estop` because it had reasoned about the rule and not run it.
  want "and the robot stays safe: a stop cannot make a safe robot less safe (§27)" \
    "safe_sticky" "$K/estop"
  # The half that is the whole trade: nothing may ever report this as a *completed* stop, because
  # nothing here watched the body come to rest.
  grep -q "stop_complete" "$K/estop" \
    && { fail "an estop was reported as a completed stop:"; sed 's/^/      /' "$K/estop"; } \
    || pass "and never as a completed stop — nothing watched the body come to rest (§27)"

  kill "$KSTOP" 2>/dev/null || true
  wait "$KSTOP" 2>/dev/null || true
  kill "$KDAEMON" 2>/dev/null || true
  wait "$KDAEMON" 2>/dev/null || true
fi


say "11. A window replays against the limits it was recorded under (§13, §83.5)"
# §13's argument is that a recording turns "the robot did something strange on Tuesday" into a
# regression test. That only holds if somebody can put the window back through the Safety Kernel,
# and until 2026-09-09 nothing could: the reader existed, `replay()` existed, and nothing joined
# them.
#
# This is the whole chain in one place and none of it is a double: a real driver, a real policy in
# its own process, a real ring shared between them, a real MCAP seal, and the file read back and
# re-evaluated by the same `evaluate_cycle` the loop ran. What it asserts is the only thing worth
# asserting about a replay — that it reaches the verdicts the recording holds — plus the two
# failures that would make a green result meaningless.
if [ -z "$CC" ]; then
  printf '  \033[2m- no C compiler, so no mock robot and no window to replay. Set CC.\033[0m\n'
else
  R="$ROOT/replay"
  rm -rf "$R" && mkdir -p "$R/bb"
  "$CC" -std=c11 -O2 -fPIC -shared -I bindings/c -o "$R/mock.so" \
      drivers/mock/mock_robot.c -lm 2>"$R/cc.log" \
    && pass "the mock robot compiles" \
    || { fail "the mock robot did not compile:"; sed 's/^/      /' "$R/cc.log"; }

  # A short socket path. `sun_path` is 108 bytes and $ROOT can be anywhere.
  RSOCK="$(mktemp -u /tmp/skynet-replay-XXXXXX).sock"
  "$SKYNETK" run --driver "$R/mock.so" --tier host --velocity-max 2.0 --jerk 0.1 \
    --watchdog-ms 200 --hz 100 --cycles 150 --channels J1,J2,J3 \
    --blackbox "$R/bb" --policy-socket "$RSOCK" > "$R/k.log" 2>&1 &
  RLOOP=$!
  track "$RLOOP"

  # Wait for the socket rather than sleeping a fixed span: on a loaded runner a fixed sleep is a
  # race, and a policy that connects to nothing produces a window with no commands in it — which
  # this section would then report as a replay that refused, for a reason that is about the runner.
  i=0
  while [ $i -lt 60 ] && [ ! -S "$RSOCK" ]; do i=$((i + 1)); sleep 0.25; done

  "$BIN/skynetp" run --hz 100 --cycles 140 --channels 3 --backend trajectory \
    --setpoints "0:0.1,0.2,0.3;40:0.5,0.6,0.7;90:1.9,1.9,1.9" \
    --ring-socket "$RSOCK" > "$R/p.log" 2>&1 || true
  wait "$RLOOP" 2>/dev/null || true
  rm -f "$RSOCK"

  WINDOW=$(ls "$R/bb/sealed/"*.mcap 2>/dev/null | head -1)
  if [ -z "$WINDOW" ]; then
    fail "the loop sealed no window:"; sed 's/^/      /' "$R/k.log"
  else
    pass "the loop sealed a window on the way out"

    # The limits beside it, without which nothing off this bench could ever be replayed: `skynetd`
    # resolves its own from a descriptor and a plan, which are not what this loop ran under.
    [ -f "${WINDOW%.mcap}.limits.json" ] \
      && pass "and wrote the limits it ran under beside it" \
      || fail "no limits document beside ${WINDOW}"
    grep -q '"watchdog_ms"' "${WINDOW%.mcap}.limits.json" \
      && pass "including the envelope, which is where the watchdog's window lives" \
      || fail "the limits document carries no envelope, so the watchdog cannot be reconstructed"

    # `|| RC=$?` rather than a bare call: this script runs under `set -e` and this command
    # deliberately exits non-zero to say *the window did not reproduce*, which is an answer rather
    # than a failure. Without the guard the first divergence check below would kill the run before
    # printing anything, which is exactly what it did.
    RC=0; "$SKYNET" blackbox replay "$WINDOW" > "$R/replay.out" 2>&1 || RC=$?
    [ "$RC" -eq 0 ] \
      && pass "and the window replays: every evaluated cycle reproduces (exit 0)" \
      || { fail "the window did not replay (exit $RC):"; sed 's/^/      /' "$R/replay.out"; }
    want "it says how many cycles it evaluated" "evaluated cycles reproduced" "$R/replay.out"
    # §25: the loop returns before the kernel when nothing was posted, so those cycles carry no
    # verdict. Asserted rather than assumed — a policy that connects late is the ordinary case on a
    # bench, and a replay that evaluated those cycles reported the watchdog tripping on every one.
    want "and counts the cycles nobody posted a command on (§25)" "nothing posted" "$R/replay.out"

    # Load-bearing, proven by breaking it. A window read against a tier that was not in force must
    # diverge loudly rather than agree quietly — that is what makes a green replay mean something.
    sed 's/"tier":"host"/"tier":"guest"/; s/"enforcement":"enforcing"/"enforcement":"advisory"/' \
      "${WINDOW%.mcap}.limits.json" > "$R/wrong.json"
    RC=0; "$SKYNET" blackbox replay "$WINDOW" --limits "$R/wrong.json" > "$R/wrong.out" 2>&1 || RC=$?
    [ "$RC" -eq 2 ] \
      && pass "and the wrong tier diverges rather than agreeing quietly (exit 2)" \
      || { fail "a replay against the wrong tier answered $RC:"; sed 's/^/      /' "$R/wrong.out"; }
    want "naming the field that disagreed" "advisory" "$R/wrong.out"

    # And the refusal, which is the other half of not lying: a document with no envelope cannot
    # reconstruct the watchdog, and the watchdog decided every cycle in this window.
    sed 's/,"envelope":{[^}]*}//' "${WINDOW%.mcap}.limits.json" > "$R/noenv.json"
    "$SKYNET" blackbox replay "$WINDOW" --limits "$R/noenv.json" > "$R/noenv.out" 2>&1 || true
    want "a document that cannot reconstruct an active guard is refused, not guessed at" \
      "envelope.watchdog_ms" "$R/noenv.out"
  fi
fi

printf '\n'
if [ "$FAILED" -eq 0 ]; then
  printf '\033[32mall checks passed\033[0m'
  [ "$STARTED" -eq 1 ] || printf ' \033[2m(the systemd launch step did not run here)\033[0m'
  printf '\n'
  exit 0
fi
printf '\033[31m%s check(s) failed\033[0m\n' "$FAILED"
exit 1
