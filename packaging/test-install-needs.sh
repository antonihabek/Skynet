#!/usr/bin/env bash
#
# What `install.sh` gives a driver, measured on a real systemd (DRY-COVERAGE-PLAN.md F185, items
# 12.B.1 and 12.B.5).
#
#   bash packaging/test-install-needs.sh --docker   # in a disposable privileged container
#   sudo bash packaging/test-install-needs.sh       # here: needs root and systemd as PID 1
#
# # Why this is not a unit test
#
# The two claims are about what systemd *does* with a directive, and the installer's whole history
# is directives that were written, ignored, and reported as fine (the CPUAffinity note in
# install.sh). So every assertion here asks the running system: a transient unit is started with the
# properties the installed drop-in produced — read back from `systemctl show`, not from the file —
# and a process in it tries to open the sockets. The mock robot has no network, `driver check` runs
# outside the unit, and nothing else in the repository exercises the loop's namespace.
#
# What it does **not** prove: that a driver reaches a robot. It proves the sandbox lets through
# exactly what the driver's `transport` names, and that `install.sh --check` notices when it does
# not. Nothing here touches a physical machine, and the kernel it runs on is whatever the container
# shares with its host.
#
# `--docker` builds `skynet-systemd-test:24.04` from packaging/test/systemd.Dockerfile (Ubuntu with
# systemd and python3, nothing of Skynet's), mounts this checkout read-only, and removes the
# container afterwards. Without it the script installs units on the machine it runs on, exactly as
# `virtual-robot.sh up` does, and `--uninstall`s them at the end.

set -u

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$HERE/.." && pwd)"
FAILED=0
pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED=$((FAILED + 1)); }
note() { printf '  %s\n' "$*"; }
step() { printf '\n  \033[1m%s\033[0m\n' "$*"; }

# ── --docker: build the image, run this script inside it, remove it ──────────
if [ "${1:-}" = "--docker" ]; then
  command -v docker >/dev/null 2>&1 || { fail "docker is not on this machine."; exit 1; }
  for built in target/release/skynetk drivers/mock/libskynet_mock.so drivers/python-host/libskynet_python_host.so; do
    [ -e "$REPO/$built" ] || { fail "$built is not built. cargo build --release --workspace; make -C drivers/mock lib; make -C drivers/python-host"; exit 1; }
  done
  # Built once and kept: the image is Ubuntu plus systemd and holds nothing of ours, so there is
  # nothing to go stale, and a build needs the network that a test run should not.
  docker image inspect skynet-systemd-test:24.04 >/dev/null 2>&1 \
    || docker build -q -t skynet-systemd-test:24.04 -f "$HERE/test/systemd.Dockerfile" "$HERE/test" >/dev/null \
    || { fail "the systemd test image did not build."; exit 1; }
  NAME="skynet-install-needs-$$"
  docker run -d --name "$NAME" --privileged --cgroupns=host -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
    --tmpfs /run --tmpfs /run/lock -v "$REPO:/repo:ro" skynet-systemd-test:24.04 >/dev/null \
    || { fail "the container would not start."; exit 1; }
  trap 'docker stop "$NAME" >/dev/null 2>&1; docker container rm "$NAME" >/dev/null 2>&1' EXIT
  for _ in $(seq 1 50); do
    state="$(docker exec "$NAME" systemctl is-system-running 2>/dev/null || true)"
    case "$state" in running|degraded) break ;; esac
    sleep 0.2
  done
  docker exec "$NAME" bash /repo/packaging/test-install-needs.sh
  exit $?
fi

# ── the machine ──────────────────────────────────────────────────────────────
[ "$(id -u)" = "0" ] || { fail "this installs units and starts transient ones: it needs root (or --docker)."; exit 1; }
[ "$(ps -p 1 -o comm= 2>/dev/null || true)" = "systemd" ] || { fail "systemd is not PID 1 here. Use --docker."; exit 1; }
command -v python3 >/dev/null 2>&1 || { fail "python3 is needed: the probe is a Python process in a transient unit."; exit 1; }
command -v ip >/dev/null 2>&1 || { fail "ip (iproute2) is needed to give the loopback two addresses."; exit 1; }

INSTALL="${INSTALL_SH:-$HERE/install.sh}"
BIN="$REPO/target/release"
MOCK="$REPO/drivers/mock/libskynet_mock.so"
PYHOST="$REPO/drivers/python-host/libskynet_python_host.so"
for need in "$BIN/skynetd" "$BIN/skynetk" "$BIN/skynet" "$BIN/skynetp" "$MOCK" "$PYHOST"; do
  [ -e "$need" ] || { fail "$need is missing. Build first (see --docker's message)."; exit 1; }
done
T="$(mktemp -d)"
LISTENER=""
cleanup() {
  [ -n "$LISTENER" ] && kill "$LISTENER" 2>/dev/null
  systemctl stop skynetk.service skynetd.service >/dev/null 2>&1
  bash "$INSTALL" --uninstall >/dev/null 2>&1
  ip addr del 10.77.0.1/32 dev lo 2>/dev/null
  ip addr del 10.77.0.2/32 dev lo 2>/dev/null
  rm -rf "$T"
}
trap cleanup EXIT

# The descriptor is the mock's own, from the driver, as virtual-robot.sh does it.
"$BIN/skynet" driver check "$MOCK" --descriptor "$T/body.json" >/dev/null 2>&1
[ -s "$T/body.json" ] || { fail "the mock body would not describe itself."; exit 1; }

# ── the world the probes look at ─────────────────────────────────────────────
# Two addresses on the loopback, both with a listener, in the *host* namespace: an installed unit is
# allowed to reach the first and not the second, and one in a private namespace can reach neither.
ip addr add 10.77.0.1/32 dev lo
ip addr add 10.77.0.2/32 dev lo
cat > "$T/listener.py" <<'PY'
import socket, threading, time
def serve(host):
    s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((host, 9000)); s.listen(8)
    while True:
        c, _ = s.accept(); c.close()
for h in ("10.77.0.1", "10.77.0.2"):
    threading.Thread(target=serve, args=(h,), daemon=True).start()
time.sleep(3600)
PY
python3 "$T/listener.py" &
LISTENER=$!
sleep 0.5

cat > "$T/probe.py" <<'PY'
import errno, json, socket
def tcp(host):
    # systemd's IPAddressDeny is a cgroup BPF filter that *drops* the packet, so a denied connect
    # is a timeout and not an error; a socket() the address-family list refuses is an error.
    s = None
    try:
        s = socket.socket(); s.settimeout(1)
        s.connect((host, 9000)); return "connected"
    except socket.timeout:
        return "timeout"
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))
    finally:
        if s is not None:
            s.close()
def family(name):
    try:
        if name == "can":
            socket.socket(socket.AF_CAN, socket.SOCK_RAW, socket.CAN_RAW).close()
        elif name == "packet":
            socket.socket(socket.AF_PACKET, socket.SOCK_RAW).close()
        elif name == "inet":
            socket.socket(socket.AF_INET, socket.SOCK_DGRAM).close()
        return "opened"
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))
print(json.dumps({"tcp1": tcp("10.77.0.1"), "tcp2": tcp("10.77.0.2"),
                  "can": family("can"), "packet": family("packet"), "inet": family("inet")}))
PY

# ── helpers ──────────────────────────────────────────────────────────────────
install_robot() { bash "$INSTALL" --driver "$1" --descriptor "$T/body.json" --rt-cpu 0 --general-cpus 0 "${@:2}" > "$T/install.log" 2>&1; }
check_robot() { bash "$INSTALL" --check > "$T/check.log" 2>&1; }
uninstall_robot() { bash "$INSTALL" --uninstall > /dev/null 2>&1; }

# The properties the installed unit resolved to, as systemd reports them, replayed on a transient
# unit that runs the probe. `-p` on systemd-run is the same directive the drop-in carries.
probe_as_unit() {
  local args=() name value
  for name in PrivateNetwork RestrictAddressFamilies IPAddressAllow IPAddressDeny; do
    value="$(systemctl show -p "$name" --value skynetk.service)"
    [ -n "$value" ] && args+=(-p "$name=$value")
  done
  systemd-run --quiet --wait --collect --pipe "${args[@]}" python3 "$T/probe.py" 2>/dev/null
}
# The same probe with no sandbox at all: what the kernel and the loopback permit. A family the
# kernel does not have is not the sandbox's doing, and this is how the two are told apart.
probe_unrestricted() { systemd-run --quiet --wait --collect --pipe python3 "$T/probe.py" 2>/dev/null; }
field() { python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"; }

expect() { # expect <what> <got> <want>
  if [ "$2" = "$3" ]; then pass "$1 ($2)"; else fail "$1: got $2, wanted $3"; fi
}

BASE="$(probe_unrestricted)"
step "The world, unrestricted"
note "$BASE"
expect "the unrestricted probe reaches 10.77.0.1" "$(echo "$BASE" | field tcp1)" connected
expect "the unrestricted probe reaches 10.77.0.2" "$(echo "$BASE" | field tcp2)" connected
CAN_HERE="$(echo "$BASE" | field can)"
PACKET_HERE="$(echo "$BASE" | field packet)"

# ── 1. the unit as shipped: F185 ─────────────────────────────────────────────
step "1. The unit as shipped — no --driver-transport (F185: measured, not assumed)"
install_robot "$MOCK" || { fail "the install failed: $(tail -3 "$T/install.log")"; }
if check_robot; then pass "install.sh --check passes, and asserts the private namespace"; else fail "--check failed: $(grep FAIL "$T/check.log")"; fi
expect "systemd parsed PrivateNetwork" "$(systemctl show -p PrivateNetwork --value skynetk.service)" yes
# F186: the two watchdog assertions compared a runtime property that is `infinity` for every stopped
# service, so they failed on every first install. They pass now — and must still be able to fail, in
# both directions (§33.5: the supervisor is watchdogged, the control loop is not).
printf '[Service]\nWatchdogSec=30s\n' > /etc/systemd/system/skynetk.service.d/30-test-watchdog.conf
systemctl daemon-reload
if check_robot; then fail "--check passed with a watchdog put on the control loop"; else pass "--check fails when skynetk is watchdogged ($(grep -c 'NOT watchdogged' "$T/check.log") assertion names it)"; fi
rm -f /etc/systemd/system/skynetk.service.d/30-test-watchdog.conf
printf '[Service]\nWatchdogSec=60s\n' > /etc/systemd/system/skynetd.service.d/30-test-watchdog.conf
systemctl daemon-reload
if check_robot; then fail "--check passed with skynetd's watchdog at 60s"; else pass "--check fails when skynetd's watchdog is not 120s"; fi
rm -f /etc/systemd/system/skynetd.service.d/30-test-watchdog.conf
systemctl daemon-reload
if check_robot; then pass "--check passes again once both are put back"; else fail "--check still fails: $(grep FAIL "$T/check.log")"; fi
P="$(probe_as_unit)"; note "$P"
expect "a driver in the shipped unit cannot reach a controller (10.77.0.1)" "$(echo "$P" | field tcp1)" ENETUNREACH
expect "…nor open an IP socket that goes anywhere: the namespace holds only a downed lo" "$(echo "$P" | field tcp2)" ENETUNREACH
uninstall_robot

# ── 2. ip, one controller ────────────────────────────────────────────────────
step "2. --driver-transport ip --driver-address 10.77.0.1"
install_robot "$MOCK" --driver-transport ip --driver-address 10.77.0.1 || fail "the install failed: $(tail -3 "$T/install.log")"
if check_robot; then pass "install.sh --check passes"; else fail "--check failed: $(grep FAIL "$T/check.log")"; fi
expect "systemd parsed PrivateNetwork" "$(systemctl show -p PrivateNetwork --value skynetk.service)" no
note "families: $(systemctl show -p RestrictAddressFamilies --value skynetk.service)"
note "allow: $(systemctl show -p IPAddressAllow --value skynetk.service)   deny: $(systemctl show -p IPAddressDeny --value skynetk.service)"
P="$(probe_as_unit)"; note "$P"
expect "the controller named is reachable" "$(echo "$P" | field tcp1)" connected
expect "another address on the same network is not (packets dropped: a timeout)" "$(echo "$P" | field tcp2)" timeout
expect "a CAN socket cannot be opened" "$(echo "$P" | field can)" EAFNOSUPPORT
expect "a raw packet socket cannot be opened" "$(echo "$P" | field packet)" EAFNOSUPPORT
expect "an IP socket can" "$(echo "$P" | field inet)" opened
# The negative control: --check must notice when the network is not what the install asked for.
sed -i 's/^PrivateNetwork=no/PrivateNetwork=yes/' /etc/systemd/system/skynetk.service.d/10-install.conf
systemctl daemon-reload
if check_robot; then fail "--check passed with PrivateNetwork put back to yes: the assertion reads the file, not systemd"; else pass "--check fails when the unit is closed again ($(grep -c FAIL "$T/check.log") failing)"; fi
uninstall_robot

# ── 3. the keywords and a CIDR ───────────────────────────────────────────────
step "3. a CIDR, and any"
install_robot "$MOCK" --driver-transport ip --driver-address 10.77.0.0/24 || fail "the install failed: $(tail -3 "$T/install.log")"
if check_robot; then pass "install.sh --check passes for a CIDR"; else fail "--check failed: $(grep FAIL "$T/check.log")"; fi
P="$(probe_as_unit)"; note "$P"
expect "both addresses in the CIDR are reachable" "$(echo "$P" | field tcp1)/$(echo "$P" | field tcp2)" connected/connected
uninstall_robot
install_robot "$MOCK" --driver-transport ip --driver-address any || fail "the install failed: $(tail -3 "$T/install.log")"
if check_robot; then pass "install.sh --check passes for any"; else fail "--check failed: $(grep FAIL "$T/check.log")"; fi
P="$(probe_as_unit)"
expect "any leaves the address space open" "$(echo "$P" | field tcp2)" connected
uninstall_robot

# ── 4. can and raw ───────────────────────────────────────────────────────────
step "4. --driver-transport can, and raw: the device families, and no IP"
install_robot "$MOCK" --driver-transport can || fail "the install failed: $(tail -3 "$T/install.log")"
if check_robot; then pass "install.sh --check passes for can"; else fail "--check failed: $(grep FAIL "$T/check.log")"; fi
P="$(probe_as_unit)"; note "$P"
expect "can: no IP socket is even opened (the family list has no AF_INET)" "$(echo "$P" | field tcp1)" EAFNOSUPPORT
if [ "$CAN_HERE" = "opened" ]; then
  expect "can: a CAN socket opens" "$(echo "$P" | field can)" opened
else
  note "this kernel has no CAN sockets ($CAN_HERE unrestricted), so what the unit allows cannot be told from what it forbids: skipped"
fi
expect "can: a raw packet socket does not" "$(echo "$P" | field packet)" EAFNOSUPPORT
uninstall_robot
install_robot "$MOCK" --driver-transport raw || fail "the install failed: $(tail -3 "$T/install.log")"
if check_robot; then pass "install.sh --check passes for raw"; else fail "--check failed: $(grep FAIL "$T/check.log")"; fi
P="$(probe_as_unit)"; note "$P"
expect "raw: no IP socket is even opened (the family list has no AF_INET)" "$(echo "$P" | field tcp1)" EAFNOSUPPORT
if [ "$PACKET_HERE" = "opened" ]; then
  expect "raw: a packet socket opens" "$(echo "$P" | field packet)" opened
else
  note "packet sockets are not available here ($PACKET_HERE unrestricted): skipped"
fi
expect "raw: a CAN socket does not" "$(echo "$P" | field can)" EAFNOSUPPORT
uninstall_robot

# ── 5. the refusals, each before anything is written ─────────────────────────
step "5. Refusals name the cause and the flag, and write nothing"
refuses() { # refuses <what> <text the message must contain> <install args…>
  local what="$1" text="$2"; shift 2
  if bash "$INSTALL" --driver "$MOCK" --descriptor "$T/body.json" "$@" > "$T/refuse.log" 2>&1; then
    fail "$what was accepted"
  elif grep -q -- "$text" "$T/refuse.log" && [ ! -e /etc/skynet/skynetk.env ]; then
    pass "$what is refused, and nothing was installed"
  else
    fail "$what: refused, but not as intended: $(grep -m1 error "$T/refuse.log")"
  fi
}
refuses "ip with no address" "needs --driver-address" --driver-transport ip
refuses "an address with no ip transport" "only means something with --driver-transport ip" --driver-address 10.77.0.1
refuses "an unknown transport" "is not ip, can or raw" --driver-transport wifi
refuses "a host name in place of an address" "Host names are not accepted" --driver-transport ip --driver-address robot.local
refuses "any beside another address" "cannot be combined" --driver-transport ip --driver-address any,10.77.0.1
refuses "a Python module on a non-Python driver" "libskynet_python_host.so" --python-module skynet_ur --python-payload "$T"
refuses "a Python payload with no module" "needs --python-module" --python-payload "$T"

# ── 6. a Python driver, end to end ───────────────────────────────────────────
step "6. A Python driver: payload, interpreter and the module importing under the unit's own path"
mkdir -p "$T/payload/skynet_lerobot"
cp -R "$REPO/drivers/lerobot/skynet_lerobot/." "$T/payload/skynet_lerobot/"
find "$T/payload" -name __pycache__ -prune -exec rm -rf {} +
cp "$REPO/bindings/python/skynet_rhal.py" "$T/payload/"
refuses "a payload with no skynet_rhal.py" "skynet_rhal.py" --driver "$PYHOST" --python-module skynet_lerobot --python-payload "$REPO/drivers/lerobot"
# A distribution the interpreter has: a minimal dist-info, so the test needs no package index.
DISTINFO=/usr/lib/python3/dist-packages/skynet_test_dist-1.2.3.dist-info
mkdir -p "$DISTINFO"
printf 'Metadata-Version: 2.1\nName: skynet-test-dist\nVersion: 1.2.3\n' > "$DISTINFO/METADATA"
refuses "a distribution the interpreter lacks" "pip install" --driver "$PYHOST" --python-module skynet_lerobot --python-payload "$T/payload" --python-require no-such-distribution-x
refuses "a distribution at the wrong version" "needs skynet-test-dist==9.9.9" --driver "$PYHOST" --python-module skynet_lerobot --python-payload "$T/payload" --python-require skynet-test-dist==9.9.9
if bash "$INSTALL" --driver "$PYHOST" --descriptor "$T/body.json" --rt-cpu 0 --general-cpus 0 \
     --python-module skynet_lerobot --python-payload "$T/payload" --python-require skynet-test-dist==1.2.3 > "$T/install.log" 2>&1; then
  pass "the install is accepted, with a requirement the interpreter meets (skynet-test-dist==1.2.3)"
else
  fail "the install failed: $(grep -m1 error "$T/install.log")"
fi
if check_robot; then pass "install.sh --check passes, the import included"; else fail "--check failed: $(grep FAIL "$T/check.log")"; fi
expect "the payload is where the unit's PYTHONPATH points" "$(test -f /usr/lib/skynet/python/skynet_lerobot/__main__.py && test -f /usr/lib/skynet/python/skynet_rhal.py && echo yes)" yes
expect "the payload is root's and read-only to everyone else" "$(stat -c %U:%a /usr/lib/skynet/python/skynet_rhal.py)" root:644
# The end: the loop starts, hosts the Python child, and the child says it is the fake. The fake is
# switched on by a drop-in of this test's own (a bench robot's environment, not the installer's).
mkdir -p /etc/systemd/system/skynetk.service.d
printf '[Service]\nEnvironment=SKYNET_LEROBOT_FAKE=1\n' > /etc/systemd/system/skynetk.service.d/20-test-fake.conf
systemctl daemon-reload
systemctl start skynetk.service
for _ in $(seq 1 40); do
  systemctl is-active --quiet skynetk.service && break
  sleep 0.25
done
sleep 2
expect "skynetk is active with a Python driver behind it" "$(systemctl is-active skynetk.service)" active
if journalctl -u skynetk.service --no-pager 2>/dev/null | grep -q "SKYNET_LEROBOT_FAKE is set"; then
  pass "the Python child ran inside the unit (its own stderr line is in the journal)"
else
  fail "no sign of the Python child in the journal: $(journalctl -u skynetk.service --no-pager -n 6 2>/dev/null | tr '\n' '|')"
fi
systemctl stop skynetk.service
rm -f /etc/systemd/system/skynetk.service.d/20-test-fake.conf
rm -rf "$DISTINFO"
uninstall_robot

# ── done ─────────────────────────────────────────────────────────────────────
echo
if [ "$FAILED" -eq 0 ]; then
  printf '  \033[32mall install-needs checks passed\033[0m\n\n'
  exit 0
fi
printf '  \033[31m%s install-needs check(s) failed\033[0m\n\n' "$FAILED"
exit 1
