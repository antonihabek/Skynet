#!/bin/sh
# §23.4's sandbox, attempted from inside, in the manager a robot runs packages under.
#
#   sudo SKYNET_BIN=target/debug sh crates/skynetd-io/sandbox.sh
#
# `e2e.sh` runs the daemon in a user manager, where every unit runs as the developer — so a refusal
# there proves little about the sandbox, because the developer could not have done most of the
# forbidden things either. This runs `skynetd` in the **system** manager, as root, as a robot does,
# and installs a package through the ordinary path — `skynet install`, desired state, one reconcile —
# whose entry point tries every route F146 found open (DRY-COVERAGE-PLAN.md) and then takes §25's
# rings from a running `skynetk` and commands the mock arm. Every route must be closed and the rings
# must still arrive, and the daemon's own `safety.policy.isolation` must pass against the unit.
#
# The control is the same probe under §59's seven directives alone, which is what every package ran
# under before this: the routes open. A probe that reported every route closed on a machine where
# they were open would be reporting on itself.
#
# Then a skill (section 7): invoked by an operator, sealed the same way, commanding the mock arm's
# elbow, with preconditions on the elbow's recorded position that show the OS read it before and saw
# it moved after (DRY-COVERAGE-PLAN.md 9.3, 9.4) — and, with 9.7, judged `success` by §54.3 over that
# same recorded position, beside a control whose criterion is unreachable and is judged `failure`.
#
# Needs root, systemd as PID 1, python3 and a C compiler. It creates the group `skynet-ring`, as
# `packaging/install.sh` does, and a device node `/dev/skynet-probe`, and removes the node again.

set -eu

NO_COLOR=1
export NO_COLOR
# The loop is turned at `--tier host` on a machine that is not PREEMPT_RT, which backs no host claim
# and is not what this checks. Open for the run, in one place, as `e2e.sh` does.
SKYNETK_UNCHECKED_TIER=1
export SKYNETK_UNCHECKED_TIER

# Not under /tmp: a sandboxed unit has a /tmp of its own, and the launcher refuses a store there.
ROOT="${ROOT:-/var/lib/skynet-sandbox-check}"
BIN="${SKYNET_BIN:-}"
FAILED=0
GROUP=skynet-ring
PACKAGE=openrobot/probe
UNIT="skynet:openrobot:probe.service"

say()  { printf '\n\033[1m%s\033[0m\n' "$1"; }
pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED=$((FAILED + 1)); }
want() { # want <description> <expected substring> <file>
  if grep -q -- "$2" "$3"; then pass "$1"; else fail "$1 - expected '$2' in:"; sed 's/^/      /' "$3"; fi
}

[ "$(id -u)" = "0" ] || { echo "this runs units in the system manager, as a robot does, and needs root"; exit 1; }
[ -d /run/systemd/system ] || { echo "systemd is not PID 1 here, so there is no system manager to run units in"; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "the probe is a python3 program, and there is no python3"; exit 1; }
CC="${CC:-}"
if [ -z "$CC" ]; then
  for candidate in cc gcc clang; do
    if command -v "$candidate" >/dev/null 2>&1; then CC="$candidate"; break; fi
  done
fi
[ -n "$CC" ] || { echo "no C compiler (tried cc, gcc, clang) for the mock robot. Set CC."; exit 1; }
[ -n "$BIN" ] || { echo "set SKYNET_BIN to the directory holding skynetd, skynet, skynetk and skynetp"; exit 1; }
case "$BIN" in
  /*) ;;
  *) BIN="$(cd "$BIN" && pwd)" ;;
esac
for binary in skynetd skynet skynetk skynetp; do
  [ -x "$BIN/$binary" ] || { echo "no $binary at $BIN/$binary"; exit 1; }
done

LOOP=skynet-sandbox-loop.service
DAEMON=skynet-sandbox-daemon.service
cleanup() {
  systemctl stop "$UNIT" "$LOOP" "$DAEMON" skynet-sandbox-listen.service \
    "skynet:openrobot:elbow.service" "skynet:openrobot:elbow-control.service" \
    "skynet:openrobot:svc.service" >/dev/null 2>&1 || true
  rm -f /dev/skynet-probe
}
trap cleanup EXIT
cleanup
# The unit's name is the package's, so its journal holds every earlier run of this script too.
SINCE="$(date '+%Y-%m-%d %H:%M:%S')"
journal() { journalctl -u "$UNIT" --since "$SINCE" -o cat --no-pager 2>/dev/null; }

rm -rf "$ROOT"
mkdir -p "$ROOT/pkg/bin" "$ROOT/data/state" "$ROOT/run"
chmod 0700 "$ROOT/data/state"

say "1. The machine, as install.sh leaves it"
getent group "$GROUP" >/dev/null 2>&1 || groupadd --system "$GROUP"
pass "the group §25's socket is shared with exists: $(getent group "$GROUP")"
"$CC" -std=c11 -O2 -fPIC -shared -I bindings/c -o "$ROOT/mock_robot.so" drivers/mock/mock_robot.c -lm
pass "the mock robot compiles ($CC)"
# A device node anybody may open, so that a refusal is the sandbox's and not the node's mode.
mknod -m 0666 /dev/skynet-probe c 1 3
pass "a device node with mode 0666 exists: /dev/skynet-probe"

cat > "$ROOT/data/state/embodiment.json" <<'DESCRIPTOR'
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
 "envelope":{"watchdog_ms":100},
 "watchdog":{"soc_wdt_ms":250,"drive_command_timeout_ms":500},
 "solvers":[]}
DESCRIPTOR
# §46.3's nesting, declared so POST can pass it: the kernel measures its own worst case at start
# (9.7, ~1 ms at this loop's 200 Hz on the host tier) and the three declared windows nest outside
# it with room a loaded machine cannot eat — 100 inside 250 inside 500. Without them the nesting
# check is `unavailable`, `may_run` is false, and the robot never leaves `post`.
echo yes > "$ROOT/data/state/safe-state"

# Something listening on a controller port, so a refused connection is the sandbox's refusal and
# not an empty port's.
systemd-run --unit=skynet-sandbox-listen.service --collect python3 -c \
  "import socket,time; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',30004)); s.listen(); time.sleep(600)" >/dev/null

say "2. The probe package"
# Every route to an actuator F146 found open, tried from inside the unit, and then the one route
# that must stay open: §25's rings, taken over the rendezvous and written into. The loop's pid is
# read from a file at run time rather than written into the package, because the loop starts after
# the package is activated.
cat > "$ROOT/pkg/bin/probe.py" <<PROBE
import os, socket

def probe(name, action):
    try:
        action()
        print("PROBE open   " + name, flush=True)
    except Exception as error:
        print("PROBE closed " + name + " (" + type(error).__name__ + ": " + str(error) + ")", flush=True)

def write_state():
    with open("$ROOT/data/state/desired.json", "a") as handle:
        handle.write("")

def tcp():
    socket.create_connection(("127.0.0.1", 30004), timeout=2).close()

def can():
    socket.socket(socket.AF_CAN, socket.SOCK_RAW, socket.CAN_RAW).close()

def device():
    os.close(os.open("/dev/skynet-probe", os.O_RDWR))

def root_socket():
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.connect("$ROOT/run/skynetd.sock")
    client.close()

def signal_loop():
    with open("$ROOT/run/loop.pid") as handle:
        os.kill(int(handle.read()), 0)

print("PROBE uid " + str(os.getuid()), flush=True)
probe("write the robot's state", write_state)
probe("tcp to a controller port", tcp)
probe("a CAN socket", can)
probe("a device node", device)
probe("a root-owned socket", root_socket)
probe("a signal to the control loop", signal_loop)
PROBE
cat > "$ROOT/pkg/bin/run" <<PROGRAM
#!/bin/sh
python3 bin/probe.py
exec "$BIN/skynetp" run --hz 50 --cycles 500 --backend approach --target 0.0,0.4,0.0 --gain 0.05
PROGRAM
chmod +x "$ROOT/pkg/bin/run"
cat > "$ROOT/pkg/manifest.json" <<'MANIFEST'
{"budget":{"cpu_cores":0.5,"ram_mb":128},"digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","id":"openrobot/probe","isolation":"sandboxed","kind":"policy","runtime":{"entrypoint":"bin/run"},"schema_version":1,"source_embodiment":{"robot_slug":"mock-arm","dof":3,"control_mode":"position","control_hz":50,"action_channels":[{"index":0,"kind":"joint_position","joint":"J1","unit":"rad","peak":3.14},{"index":1,"kind":"joint_position","joint":"J2","unit":"rad","peak":2.6},{"index":2,"kind":"joint_position","joint":"J3","unit":"rad","peak":6.28}],"observations":[]},"version":"1.0.0"}
MANIFEST

"$BIN/skynet" install "$ROOT/pkg" --root "$ROOT" --id "$PACKAGE" --kind policy --version 1.0.0 \
  --digest 0000000000000000000000000000000000000000000000000000000000000000 > "$ROOT/out" 2>&1 || true
DIGEST=$(sed -n 's/.*hashes to \([0-9a-f]\{64\}\).*/\1/p' "$ROOT/out")
[ -n "$DIGEST" ] || { echo "could not read the probe's digest:"; cat "$ROOT/out"; exit 1; }
"$BIN/skynet" install "$ROOT/pkg" --root "$ROOT" --id "$PACKAGE" --kind policy --version 1.0.0 \
  --digest "$DIGEST" > "$ROOT/out" 2>&1
want "the probe stages through \`skynet install\`" "staged $PACKAGE" "$ROOT/out"

desired() { # desired <generation> <state>
  cat > "$ROOT/data/state/desired.json" <<DESIRED
{"generation":$1,"os":{"channel":"stable","version":""},
 "packages":[{"id":"$PACKAGE","version":"1.0.0","digest":"$DIGEST","kind":"policy","state":"$2","grants":[]}]}
DESIRED
}
desired 1 installed
TOKEN=$("$BIN/skynetd" credential add --root "$ROOT" --label sandbox --role operator 2>/dev/null)

say "3. A robot's boot, in its order, and then the package"
# Three steps, and the order is §33.1's and §26.3's rather than this script's convenience.
#
# * The package is **activated** before anything turns, so the recording is sized for the action
#   space it declares.
# * The **loop** starts before the supervisor, creates that recording and writes a cycle into it
#   every pass. A supervisor that started first would find no cycles, read `rhal_lost`, and hold the
#   robot in `SAFE` — which is sticky, and nothing on this edition clears (F149) — so no package
#   would ever start.
# * The **supervisor** attaches to the recording, and the package is started last, once there is a
#   supervisor socket for the probe to try.
"$BIN/skynetd" once --root "$ROOT" > "$ROOT/once" 2>&1 || true
want "one pass activates it" "activate_package $PACKAGE" "$ROOT/once"
# That pass recorded itself in a black box it created before the package was active, so sized for
# no action channels — and a loop cannot write a cycle into a ring shaped for none. A robot never
# runs this pass; the recording goes with it, and the loop creates the one a robot boots with.
rm -rf "$ROOT/data/blackbox"

start_loop() {
  systemd-run --unit="$LOOP" --collect -E SKYNETK_UNCHECKED_TIER=1 \
    "$BIN/skynetk" run --driver "$ROOT/mock_robot.so" --tier host --velocity-max 2.0 --jerk 0.1 \
    --watchdog-ms 50 --hz 200 --channels J1,J2,J3 --blackbox "$ROOT/data/blackbox" \
    --policy-socket "$ROOT/run/policy.sock" --policy-socket-group "$GROUP" >/dev/null
  i=0
  while [ ! -S "$ROOT/run/policy.sock" ] && [ $i -lt 100 ]; do i=$((i + 1)); sleep 0.1; done
}
start_loop
ls -l "$ROOT/run/policy.sock" > "$ROOT/sock" 2>&1 || true
want "the loop's socket belongs to the group, mode 0660" "srw-rw---- 1 root $GROUP" "$ROOT/sock"
systemctl show "$LOOP" -p MainPID --value > "$ROOT/run/loop.pid"
chmod 0644 "$ROOT/run/loop.pid"
# The loop binds the socket and then creates the recording, so the recording is waited for too: a
# supervisor arriving first would create one of its own and the loop would be writing elsewhere.
i=0
while [ ! -s "$ROOT/data/blackbox/ring" ] && [ $i -lt 100 ]; do i=$((i + 1)); sleep 0.1; done
[ -s "$ROOT/data/blackbox/ring" ] && pass "the loop created the recording, sized for the arm" \
  || fail "the loop never created its recording"

# Running, before the supervisor starts rather than after: a running `skynetd` reads the desired
# state from disk when it starts and when the Hub hands it a new one, never because the file
# changed under it — so an edit made now would be seen by nothing. It binds its socket before its
# first pass, so the probe still starts with a supervisor socket there to try.
desired 2 running
systemd-run --unit="$DAEMON" --collect "$BIN/skynetd" --root "$ROOT" --interval 500 >/dev/null
i=0
while [ $i -lt 100 ]; do
  "$BIN/skynet" status --socket "$ROOT/run/skynetd.sock" >/dev/null 2>&1 && break
  i=$((i + 1)); sleep 0.1
done
[ -S "$ROOT/run/skynetd.sock" ] && pass "the supervisor answers on its socket" \
  || fail "the supervisor never answered, so the probe's attempt on its socket proves nothing"
# Polled until the supervisor has judged the robot, which it does one interval in rather than at
# start (F151): it read §27 before opening the recording, took every unobservable condition for a
# lost RHAL, and booted a robot whose loop had been turning into a `SAFE` nothing clears.
i=0
while [ $i -lt 60 ]; do
  "$BIN/skynet" state --socket "$ROOT/run/skynetd.sock" --token "$TOKEN" > "$ROOT/state" 2>&1 || true
  grep -q "control cycles were recorded" "$ROOT/state" && break
  i=$((i + 1)); sleep 0.1
done
want "the supervisor sees the loop turning" "control cycles were recorded" "$ROOT/state"
if head -1 "$ROOT/state" | grep -q "^safe"; then
  fail "the robot is SAFE, which nothing on this edition clears (F149, F151):"; sed 's/^/      /' "$ROOT/state"
else
  pass "the robot is not SAFE: the supervisor judged the interval it watched (F151)"
fi

i=0
while [ $i -lt 200 ]; do
  journal | grep -q "^PROBE closed a signal\|^PROBE open   a signal" && break
  i=$((i + 1)); sleep 0.1
done
systemctl show "$UNIT" -p DynamicUser,SupplementaryGroups,PrivateNetwork,RestrictAddressFamilies,PrivateDevices,ProtectSystem > "$ROOT/unit" 2>&1 || true
want "the probe runs as a user of its own" "DynamicUser=yes" "$ROOT/unit"
want "in the ring's group, because it is the commanding package" "SupplementaryGroups=$GROUP" "$ROOT/unit"
want "with no network" "PrivateNetwork=yes" "$ROOT/unit"
want "no socket family but AF_UNIX" "RestrictAddressFamilies=AF_UNIX" "$ROOT/unit"
want "no device node" "PrivateDevices=yes" "$ROOT/unit"
want "and a read-only tree" "ProtectSystem=strict" "$ROOT/unit"

# §15's own test, run by the robot on itself while the probe is running.
"$BIN/skynet" conformance --socket "$ROOT/run/skynetd.sock" --token "$TOKEN" > "$ROOT/conformance" 2>&1 || true
grep -A3 "safety.policy.isolation" "$ROOT/conformance" > "$ROOT/isolation" || true
want "and the robot's own safety.policy.isolation passes against it" "pass" "$ROOT/isolation"
want "having asked the kernel for the user and the network namespace too" "a network namespace of its own" "$ROOT/conformance"

i=0
while [ $i -lt 100 ]; do
  journal > "$ROOT/journal" || true
  grep -q "skynetp: .* turn(s)" "$ROOT/journal" && break
  i=$((i + 1)); sleep 0.2
done

say "4. Every route F146 found open is closed"
if grep -q "^PROBE uid 0$" "$ROOT/journal"; then
  fail "the probe ran as root"
else
  pass "the probe ran as $(sed -n 's/^PROBE uid /uid /p' "$ROOT/journal" | head -1)"
fi
for route in "write the robot's state" "tcp to a controller port" "a CAN socket" "a device node" \
             "a root-owned socket" "a signal to the control loop"; do
  if grep -q "^PROBE closed $route" "$ROOT/journal"; then
    pass "closed: $route — $(grep "^PROBE closed $route" "$ROOT/journal" | head -1 | sed 's/.*(\(.*\))$/\1/')"
  else
    fail "not closed: $route"; grep "PROBE .*$route" "$ROOT/journal" | sed 's/^/      /'
  fi
done

say "5. And the one route that must stay open"
want "the probe took §25's rings and commanded the arm" "commanded" "$ROOT/journal"
if grep -q "may not connect to it" "$ROOT/journal"; then
  fail "the probe was refused the rings:"; grep "skynetp" "$ROOT/journal" | sed 's/^/      /'
fi

say "6. The control: the same probe under §59's seven directives"
cp "$ROOT/data/state/desired.json" "$ROOT/desired.before"
systemd-run --wait --pipe --collect --quiet -p NoNewPrivileges=yes -p ProtectSystem=yes \
  -p ProtectKernelTunables=yes -p ProtectKernelModules=yes -p ProtectControlGroups=yes \
  -p RestrictSUIDSGID=yes -p CapabilityBoundingSet= --working-directory="$ROOT/pkg" \
  python3 bin/probe.py > "$ROOT/control" 2>&1 || true
OPEN=$(grep -c "^PROBE open" "$ROOT/control" || true)
if [ "$OPEN" -ge 4 ]; then
  pass "under the old confinement $OPEN routes are open, so the probe can tell open from closed"
else
  fail "the control found only $OPEN routes open, so step 4 proves nothing:"; sed 's/^/      /' "$ROOT/control"
fi
# The control opened the robot's desired state for writing and wrote nothing, which is the whole of
# the route: a probe that changed it would have changed the robot. Held to that.
cmp -s "$ROOT/desired.before" "$ROOT/data/state/desired.json" \
  || fail "the control changed the robot's desired state"

say "6b. §96's switch, attempted from inside a service"
# The policy probe above is sealed by kind. A service is sealed by *default* and granted its
# network through the console switch — so the attempt is run three times: sealed, granted, and
# sealed again, with the unit's own journal as the witness each time. The listener on 30004 is
# still up from section 1, so a refused connection is the sandbox's and an open one is real.
SVC=openrobot/svc
SVC_UNIT="skynet:openrobot:svc.service"
mkdir -p "$ROOT/svcpkg/bin"
cat > "$ROOT/svcpkg/bin/svcprobe.py" <<PROBE
import socket
try:
    socket.create_connection(("127.0.0.1", 30004), timeout=2).close()
    print("PROBE open   tcp to a controller port", flush=True)
except Exception as error:
    print("PROBE closed tcp to a controller port (" + type(error).__name__ + ")", flush=True)
PROBE
cat > "$ROOT/svcpkg/bin/run" <<PROGRAM
#!/bin/sh
python3 bin/svcprobe.py
exec sleep 300
PROGRAM
chmod +x "$ROOT/svcpkg/bin/run"
cat > "$ROOT/svcpkg/manifest.json" <<'MANIFEST'
{"budget":{"cpu_cores":0.5,"ram_mb":64},"id":"openrobot/svc","isolation":"sandboxed","kind":"service","runtime":{"entrypoint":"bin/run"},"schema_version":1,"version":"1.0.0"}
MANIFEST
"$BIN/skynet" install "$ROOT/svcpkg" --root "$ROOT" --id "$SVC" --kind service --version 1.0.0 \
  --digest 0000000000000000000000000000000000000000000000000000000000000000 > "$ROOT/out" 2>&1 || true
SVC_DIGEST=$(sed -n 's/.*hashes to \([0-9a-f]\{64\}\).*/\1/p' "$ROOT/out")
[ -n "$SVC_DIGEST" ] || { echo "could not read the service's digest:"; cat "$ROOT/out"; exit 1; }
"$BIN/skynet" install "$ROOT/svcpkg" --root "$ROOT" --id "$SVC" --kind service --version 1.0.0 \
  --digest "$SVC_DIGEST" > "$ROOT/out" 2>&1
want "the service stages through \`skynet install\`" "staged $SVC" "$ROOT/out"
# The running daemon reads the desired state at start, never because the file changed — so it is
# stopped and started again over the new document, the way section 7 restarts it over its own.
# The probe is retired to `installed`: its run is over, and a restart would only re-prove it.
systemctl stop "$DAEMON" >/dev/null 2>&1 || true
cat > "$ROOT/data/state/desired.json" <<DESIRED
{"generation":3,"os":{"channel":"stable","version":""},
 "packages":[{"id":"$PACKAGE","version":"1.0.0","digest":"$DIGEST","kind":"policy","state":"installed","grants":[]},
             {"id":"$SVC","version":"1.0.0","digest":"$SVC_DIGEST","kind":"service","state":"running","grants":[]}]}
DESIRED
SINCE_SVC="$(date '+%Y-%m-%d %H:%M:%S')"
systemd-run --unit="$DAEMON" --collect "$BIN/skynetd" --root "$ROOT" --interval 500 >/dev/null
i=0
while [ $i -lt 100 ]; do
  "$BIN/skynet" status --socket "$ROOT/run/skynetd.sock" >/dev/null 2>&1 && break
  i=$((i + 1)); sleep 0.1
done
svc_journal() { journalctl -u "$SVC_UNIT" --since "$SINCE_SVC" -o cat --no-pager 2>/dev/null; }
i=0
while [ $i -lt 200 ]; do
  svc_journal | grep -q "^PROBE \(open\|closed\) tcp" && break
  i=$((i + 1)); sleep 0.1
done
svc_journal > "$ROOT/svc.journal" || true
want "a service starts sealed: the attempt from inside is refused" \
  "PROBE closed tcp to a controller port" "$ROOT/svc.journal"
systemctl show "$SVC_UNIT" -p PrivateNetwork,RestrictAddressFamilies > "$ROOT/svc.unit" 2>&1 || true
want "and the unit carries the sealed sandbox's words" "PrivateNetwork=yes" "$ROOT/svc.unit"
# The switch, over the daemon's own socket: the choice lands in the desired state, the grant
# converges, and the unit restarts with IP families — attempted again from inside.
curl -s --unix-socket "$ROOT/run/skynetd.sock" -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"enabled": true}' \
  http://robot/packages/openrobot:svc/network > "$ROOT/out" 2>&1 || true
want "flipping the switch is applied" '"outcome":"applied"' "$ROOT/out"
i=0
while [ $i -lt 200 ]; do
  svc_journal | grep -q "^PROBE open   tcp" && break
  i=$((i + 1)); sleep 0.1
done
svc_journal > "$ROOT/svc.journal" || true
want "the restarted service reaches the controller port" \
  "PROBE open   tcp to a controller port" "$ROOT/svc.journal"
systemctl show "$SVC_UNIT" -p PrivateNetwork,RestrictAddressFamilies > "$ROOT/svc.unit" 2>&1 || true
grep -qx "RestrictAddressFamilies=AF_INET AF_INET6 AF_NETLINK AF_UNIX" "$ROOT/svc.unit" \
  && pass "under the service sandbox: IP families, nothing beneath them" \
  || { fail "the grant did not reach the unit:"; sed 's/^/      /' "$ROOT/svc.unit"; }
curl -s --unix-socket "$ROOT/run/skynetd.sock" -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"enabled": false}' \
  http://robot/packages/openrobot:svc/network > "$ROOT/out" 2>&1 || true
want "switching off is applied" '"outcome":"applied"' "$ROOT/out"
i=0
while [ $i -lt 200 ]; do
  [ "$(svc_journal | grep -c "^PROBE closed tcp")" -ge 2 ] && break
  i=$((i + 1)); sleep 0.1
done
svc_journal > "$ROOT/svc.journal" || true
[ "$(grep -c "^PROBE closed tcp" "$ROOT/svc.journal")" -ge 2 ] \
  && pass "and off seals it again: a second refusal from a restarted unit" \
  || { fail "no second refusal after switching off:"; sed 's/^/      /' "$ROOT/svc.journal"; }

say "7. A skill moves the elbow, from inside the sandbox (9.4)"
# The probe was a policy the desired state ran. This is a **skill**: invoked by an operator, taken
# through §54.4's ladder, and started in the same sealed sandbox with the same one route to the arm.
# Its entry point commands the elbow toward 0.4 rad through §25's rings and `skynetk`.
#
# The evidence is the robot's own §54.2 context rather than anything this script reads off the
# ring, and it is two preconditions on the elbow's recorded position (F147):
#
# * `robot.state.joints.J2` exists — the daemon read the body off the newest cycle, so a skill can
#   name a joint at all. Until 9.3 this path was absent on every robot, and a precondition naming it
#   refused every skill.
# * `robot.state.joints.J2 < 0.05` — the elbow starts near zero. The first invocation passes it.
#   The second is refused by it, and only by it, which is the robot saying the elbow moved. The
#   kernel's rate and jerk ceilings hold the elbow to a few hundredths of a radian per second here,
#   so the skill is given a two-second interval to move it by more than that.
#
# And 9.7's half, which 9.4 could not show: §54.3 judging the skill over the same recorded
# position. The kernel runs §48.2's five checks at start and records them in §13's ring, the
# daemon's POST passes, and the robot leaves `post` — so the first invocation runs to completion
# and is judged `success` on its one criterion, `robot.state.joints.J2 >= 0.05`, the elbow moved.
# The control is a second skill with the same entry point and an unreachable criterion, `J2 > 10`
# past the joint's ±2.6 rad limits: the same motion, judged `failure`. A judge that stamped every
# skill `success` would pass the first and fail the second.
SKILL=openrobot/elbow
SKILL_UNIT="skynet:openrobot:elbow.service"
CONTROL=openrobot/elbow-control
CONTROL_UNIT="skynet:openrobot:elbow-control.service"
# A fresh arm, because the probe above drove the elbow most of the way to 0.4 rad and the first
# precondition would then refuse the skill for a motion it did not make. The mock's joints live in
# the loop's process, so restarting the loop is what a power cycle is to a real arm.
systemctl stop "$DAEMON" "$UNIT" "$LOOP" >/dev/null 2>&1 || true
rm -f "$ROOT/run/policy.sock"
start_loop
mkdir -p "$ROOT/skill/bin"
cat > "$ROOT/skill/bin/run" <<PROGRAM
#!/bin/sh
exec "$BIN/skynetp" run --hz 50 --cycles 150 --backend approach --target 0.0,0.4,0.0 --gain 0.05
PROGRAM
chmod +x "$ROOT/skill/bin/run"
cat > "$ROOT/skill/manifest.json" <<'MANIFEST'
{"budget":{"cpu_cores":0.5,"ram_mb":128},"digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","expected_duration_seconds":30,"id":"openrobot/elbow","isolation":"sandboxed","kind":"skill","preconditions":[{"statement":"the elbow is being measured","predicate":{"op":"exists","path":"robot.state.joints.J2"}},{"statement":"the elbow starts near zero","predicate":{"op":"lt","path":"robot.state.joints.J2","value":0.05}}],"success_criteria":[{"statement":"the elbow moved","predicate":{"op":"gte","path":"robot.state.joints.J2","value":0.05}}],"runtime":{"entrypoint":"bin/run"},"schema_version":1,"source_embodiment":{"robot_slug":"mock-arm","dof":3,"control_mode":"position","control_hz":50,"action_channels":[{"index":0,"kind":"joint_position","joint":"J1","unit":"rad","peak":3.14},{"index":1,"kind":"joint_position","joint":"J2","unit":"rad","peak":2.6},{"index":2,"kind":"joint_position","joint":"J3","unit":"rad","peak":6.28}],"observations":[]},"version":"1.0.0"}
MANIFEST
"$BIN/skynet" install "$ROOT/skill" --root "$ROOT" --id "$SKILL" --kind skill --version 1.0.0 \
  --digest 0000000000000000000000000000000000000000000000000000000000000000 > "$ROOT/out" 2>&1 || true
SKILL_DIGEST=$(sed -n 's/.*hashes to \([0-9a-f]\{64\}\).*/\1/p' "$ROOT/out")
[ -n "$SKILL_DIGEST" ] || { echo "could not read the skill's digest:"; cat "$ROOT/out"; exit 1; }
"$BIN/skynet" install "$ROOT/skill" --root "$ROOT" --id "$SKILL" --kind skill --version 1.0.0 \
  --digest "$SKILL_DIGEST" > "$ROOT/out" 2>&1
want "the skill stages through \`skynet install\`" "staged $SKILL" "$ROOT/out"
# The skills present and the probe gone: one commanding package, because the daemon does not choose
# between two (§26.3's `Ambiguous`). `installed`, because an invocation decides when a skill runs.
cat > "$ROOT/data/state/desired.json" <<DESIRED
{"generation":4,"os":{"channel":"stable","version":""},
 "packages":[{"id":"$SKILL","version":"1.0.0","digest":"$SKILL_DIGEST","kind":"skill","state":"installed","grants":[]}]}
DESIRED
"$BIN/skynetd" once --root "$ROOT" > "$ROOT/once" 2>&1 || true
want "one pass activates the skill" "activate_package $SKILL " "$ROOT/once"

SINCE_SKILL="$(date '+%Y-%m-%d %H:%M:%S')"
systemd-run --unit="$DAEMON" --collect "$BIN/skynetd" --root "$ROOT" --interval 2000 >/dev/null
i=0
while [ $i -lt 60 ]; do
  "$BIN/skynet" state --socket "$ROOT/run/skynetd.sock" --token "$TOKEN" > "$ROOT/state" 2>&1 || true
  grep -q "control cycles were recorded" "$ROOT/state" && break
  i=$((i + 1)); sleep 0.1
done

skills_until_terminal() { # skills_until_terminal <file> <package> <how many invocations>
  i=0
  # Thirty seconds, because a skill that runs to completion needs its own runtime (three seconds
  # here) on top of the ladder's two-second passes — and the package followed by a space, because
  # the row holds the package beside the phase and `$SKILL` is a prefix of `$CONTROL`.
  while [ $i -lt 120 ]; do
    "$BIN/skynet" skills --socket "$ROOT/run/skynetd.sock" --token "$TOKEN" > "$1" 2>&1 || true
    [ "$(grep "$2 " "$1" | grep -c "terminal")" -ge "$3" ] && return 0
    i=$((i + 1)); sleep 0.25
  done
  return 1
}

"$BIN/skynet" skill run "$SKILL" --socket "$ROOT/run/skynetd.sock" --token "$TOKEN" > "$ROOT/out" 2>&1
want "the invocation is accepted" "Accepted, not started" "$ROOT/out"
skills_until_terminal "$ROOT/first" "$SKILL" 1 || { fail "the first invocation never landed:"; sed 's/^/      /' "$ROOT/first"; }
if grep -q "preconditions_refused" "$ROOT/first"; then
  fail "the first invocation was refused at entry, so the elbow was never read or never near zero:"
  sed 's/^/      /' "$ROOT/first"
else
  pass "the first invocation passed both preconditions: the elbow was read, and near zero"
fi
want "and §54.3 judged it success over the elbow's recorded position" "outcome success" "$ROOT/first"
want "for §54.3's reason, not the interlock's" "judged" "$ROOT/first"
journalctl -u "$SKILL_UNIT" --since "$SINCE_SKILL" -o cat --no-pager > "$ROOT/skill.journal" 2>/dev/null || true
want "its process took §25's rings from inside the sandbox" \
  "ring received from $ROOT/run/policy.sock" "$ROOT/skill.journal"
systemctl show "$SKILL_UNIT" -p DynamicUser,SupplementaryGroups,PrivateNetwork > "$ROOT/skill.unit" 2>&1 || true

# Two passes, so the newest cycle the daemon reads is one the kernel wrote after the skill stopped.
sleep 4.5
"$BIN/skynet" state --socket "$ROOT/run/skynetd.sock" --token "$TOKEN" > "$ROOT/state.after" 2>&1 || true
# The console's reading of the same joint, beside the precondition's.
J2=$(sed -n 's/^  body .*J2 \([-0-9.]*\).*/\1/p' "$ROOT/state.after")
if [ -n "$J2" ] && awk "BEGIN { exit !($J2 >= 0.05) }"; then
  pass "the console reads the elbow at $J2 rad, off the newest recorded cycle"
else
  fail "the console does not read the elbow past 0.05 rad:"; sed 's/^/      /' "$ROOT/state.after"
fi
"$BIN/skynet" skill run "$SKILL" --socket "$ROOT/run/skynetd.sock" --token "$TOKEN" > "$ROOT/out" 2>&1
skills_until_terminal "$ROOT/second" "$SKILL" 2 || { fail "the second invocation never landed:"; sed 's/^/      /' "$ROOT/second"; }
# The listing holds both invocations, newest first: the second one's row and its detail lines.
awk '/^inv-/ { n++ } n == 1' "$ROOT/second" > "$ROOT/second.row"
want "the second invocation is refused at entry" "preconditions_refused" "$ROOT/second.row"
want "by the elbow's position — it is measured, and it is no longer near zero" \
  "the elbow starts near zero" "$ROOT/second.row"
if grep -q "the elbow is being measured" "$ROOT/second.row"; then
  fail "refused because the elbow could not be read, which says nothing about whether it moved"
fi
# The control, which has to be the same skill in every respect but its criterion — and that includes
# its action space. §26.3 counts every installed package that declares one, so the control cannot be
# installed beside the skill (two commanding packages is `Ambiguous`, and the daemon chooses neither),
# and it cannot declare none either: the lease then grants it no channels and `skynetp` refuses to
# start, "--target names 3 channels and the lease grants 0". The first draft of this script did the
# second, ran for the first time under a manager on 2026-10-06, and *was judged `failure`* — by a
# process that never took the rings, so the verdict said nothing about the judge
# (DRY-COVERAGE-PLAN.md F290). So the skill is retired first: the daemon is stopped, desired state
# names only the control, one pass removes the skill and activates the control, and the daemon comes
# back. The kernel's loop is not touched, so the elbow is where the skill left it.
systemctl stop "$DAEMON" "$SKILL_UNIT" >/dev/null 2>&1 || true
mkdir -p "$ROOT/skillctl/bin"
cat > "$ROOT/skillctl/bin/run" <<PROGRAM
#!/bin/sh
exec "$BIN/skynetp" run --hz 50 --cycles 150 --backend approach --target 0.0,0.4,0.0 --gain 0.05
PROGRAM
chmod +x "$ROOT/skillctl/bin/run"
cat > "$ROOT/skillctl/manifest.json" <<'MANIFEST'
{"budget":{"cpu_cores":0.5,"ram_mb":128},"digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","expected_duration_seconds":30,"id":"openrobot/elbow-control","isolation":"sandboxed","kind":"skill","preconditions":[{"statement":"the elbow is being measured","predicate":{"op":"exists","path":"robot.state.joints.J2"}}],"success_criteria":[{"statement":"the elbow passed ten radians","predicate":{"op":"gt","path":"robot.state.joints.J2","value":10}}],"runtime":{"entrypoint":"bin/run"},"schema_version":1,"source_embodiment":{"robot_slug":"mock-arm","dof":3,"control_mode":"position","control_hz":50,"action_channels":[{"index":0,"kind":"joint_position","joint":"J1","unit":"rad","peak":3.14},{"index":1,"kind":"joint_position","joint":"J2","unit":"rad","peak":2.6},{"index":2,"kind":"joint_position","joint":"J3","unit":"rad","peak":6.28}],"observations":[]},"version":"1.0.0"}
MANIFEST
"$BIN/skynet" install "$ROOT/skillctl" --root "$ROOT" --id "$CONTROL" --kind skill --version 1.0.0 \
  --digest 0000000000000000000000000000000000000000000000000000000000000000 > "$ROOT/out" 2>&1 || true
CONTROL_DIGEST=$(sed -n 's/.*hashes to \([0-9a-f]\{64\}\).*/\1/p' "$ROOT/out")
[ -n "$CONTROL_DIGEST" ] || { echo "could not read the control's digest:"; cat "$ROOT/out"; exit 1; }
"$BIN/skynet" install "$ROOT/skillctl" --root "$ROOT" --id "$CONTROL" --kind skill --version 1.0.0 \
  --digest "$CONTROL_DIGEST" > "$ROOT/out" 2>&1
want "the control stages through \`skynet install\`" "staged $CONTROL" "$ROOT/out"
cat > "$ROOT/data/state/desired.json" <<DESIRED
{"generation":5,"os":{"channel":"stable","version":""},
 "packages":[{"id":"$CONTROL","version":"1.0.0","digest":"$CONTROL_DIGEST","kind":"skill","state":"installed","grants":[]}]}
DESIRED
"$BIN/skynetd" once --root "$ROOT" > "$ROOT/once" 2>&1 || true
want "one pass retires the skill" "remove_package $SKILL" "$ROOT/once"
want "and activates the control" "activate_package $CONTROL" "$ROOT/once"
SINCE_CONTROL="$(date '+%Y-%m-%d %H:%M:%S')"
systemd-run --unit="$DAEMON" --collect "$BIN/skynetd" --root "$ROOT" --interval 2000 >/dev/null
i=0
while [ $i -lt 60 ]; do
  "$BIN/skynet" state --socket "$ROOT/run/skynetd.sock" --token "$TOKEN" > "$ROOT/state" 2>&1 || true
  grep -q "control cycles were recorded" "$ROOT/state" && break
  i=$((i + 1)); sleep 0.1
done

# The control, last: the same entry point driving the same elbow, so the only difference between
# its run and the first invocation's is the criterion. Ten radians is past the joint's ±2.6 rad
# limits and past the 2.6 peak its sibling declares, so no motion completes it.
"$BIN/skynet" skill run "$CONTROL" --socket "$ROOT/run/skynetd.sock" --token "$TOKEN" > "$ROOT/out" 2>&1
want "the control invocation is accepted" "Accepted, not started" "$ROOT/out"
skills_until_terminal "$ROOT/control" "$CONTROL" 1 || { fail "the control never landed:"; sed 's/^/      /' "$ROOT/control"; }
awk '/^inv-/ { n++ } n == 1' "$ROOT/control" > "$ROOT/control.row"
if grep -q "preconditions_refused" "$ROOT/control.row"; then
  fail "the control was refused at entry, so its failure would say nothing about the judge:"
  sed 's/^/      /' "$ROOT/control.row"
else
  pass "the control ran the same motion to completion"
fi
want "and was judged failure on the unreachable criterion" "outcome failure" "$ROOT/control.row"
want "for §54.3's reason, not the interlock's" "judged" "$ROOT/control.row"
journalctl -u "$CONTROL_UNIT" --since "$SINCE_CONTROL" -o cat --no-pager > "$ROOT/control.journal" 2>/dev/null || true
want "its process took §25's rings too, so the failure is the criterion's" \
  "ring received from $ROOT/run/policy.sock" "$ROOT/control.journal"
systemctl stop "$SKILL_UNIT" "$CONTROL_UNIT" >/dev/null 2>&1 || true

printf '\n'
if [ "$FAILED" -eq 0 ]; then
  printf '\033[32mall checks passed\033[0m\n'
  exit 0
fi
printf '\033[31m%s check(s) failed\033[0m\n' "$FAILED"
exit 1
