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

# Start the daemon and wait until it actually answers.
#
# Waiting for the socket *file* is not enough, and finding that out here is the point: a Unix socket
# outlives the process that made it, so a killed daemon leaves a path that looks live to every
# client. `skynetd` unlinks it on bind; a client cannot, and gets `connection refused`.
start_daemon() {
  rm -f "$ROOT/run/skynetd.sock"
  "$SKYNETD" --root "$ROOT" --user --interval 1000 > "$ROOT/daemon.log" 2>&1 &
  DAEMON=$!
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
  cargo build -p skynetd-io -p skynet-cli >/dev/null
  BIN="$(cargo metadata --format-version 1 --no-deps 2>/dev/null | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')/debug"
fi
SKYNETD="$BIN/skynetd"
SKYNET="$BIN/skynet"
[ -x "$SKYNETD" ] || { echo "no skynetd at $SKYNETD"; exit 1; }

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
  STARTED=1
else
  want "the start fails, saying it could not reach systemd" "start_package openrobot/holder → failed" "$ROOT/out"
  pass "no systemd here, so the launch step is reported rather than skipped"
  STARTED=0
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
trap 'kill "$DAEMON" 2>/dev/null || true' EXIT
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

"$SKYNET" status $SOCK --token "$TOKEN" > "$ROOT/out" 2>&1
grep -q "not provisioned" "$ROOT/out" && fail "still reported unprovisioned"   || pass "the robot no longer reports itself unprovisioned"

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
"$SKYNETD" --root "$ROOT/mdns" --user --interval 1000 --lan 127.0.0.1:8099 \n  --advertise skynet-e2e > "$ROOT/mdns.log" 2>&1 &
MDNS=$!
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

printf '\n'
if [ "$FAILED" -eq 0 ]; then
  printf '\033[32mall checks passed\033[0m'
  [ "$STARTED" -eq 1 ] || printf ' \033[2m(the systemd launch step did not run here)\033[0m'
  printf '\n'
  exit 0
fi
printf '\033[31m%s check(s) failed\033[0m\n' "$FAILED"
exit 1
