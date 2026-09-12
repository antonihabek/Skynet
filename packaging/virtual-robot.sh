#!/usr/bin/env bash
#
# Bring up a complete Skynet robot on a machine with no robot attached.
#
#   sudo ./packaging/virtual-robot.sh up
#   ./packaging/virtual-robot.sh status
#   sudo ./packaging/virtual-robot.sh down
#
# # This is not a simulation of the operating system
#
# Everything here is the real thing: `install.sh` puts the real binaries at /usr/bin, systemd starts
# the real units in §33.1's order, the real Safety Kernel evaluates the real eight guards, and the
# real supervisor reconciles real desired state. The only substitution is at the bottom of the
# stack — the RHAL driver is `drivers/mock`, a body that answers the ABI and has no hardware behind
# it.
#
# That substitution is a **supported configuration** rather than a workaround. §35.3 caps a
# simulated body at guest tier and rules that no simulated run is bench evidence; the mock declares
# `simulator: true`, so the cap applies by mechanism rather than by anybody remembering. What this
# script produces is a robot whose ceiling is guest and which says so on every surface.
#
# # What it therefore proves, and what it cannot
#
# It proves the parts that are software, and exactly the ten in `do_smoke`: that the OS installs,
# that the units start in §33.1's order and stay up, that the loop runs and records cycles, that the
# body is the one that was installed, that §27's operating state and §7.6's limits resolve against
# it, that §15's runtime tests run, and that the black box has a window. Every one of those is a
# thing that had never been done outside a test before this script existed.
#
# It does **not** yet cover the package path. `ps` answering is an empty list — nothing here admits
# a package, plans its transport, starts it, or records an invocation, and an earlier draft of this
# comment claimed all four. Those need a signed `.sky` to install, which is the next thing this
# script should grow and is why §15's `package.signature.required` still fails on a real robot.
#
# It proves **nothing about a physical machine**. No joint moves, no encoder reports, no torque is
# applied, and the latency of a real fieldbus is absent. §15 keeps that distinction by tier and this
# script does not blur it: what comes out is guest-tier evidence, which is exactly what it is.

set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$HERE/.." && pwd)"

ROOT="${SKYNET_ROOT:-/}"
DATA="${ROOT%/}/data"
RUN="${ROOT%/}/run"
SOCKET="$RUN/skynetd.sock"
BUILD="$REPO/target/release"
MOCK_DIR="$REPO/drivers/mock"
UNITS=(skynetk.service skynetd.service)

if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
  R=$'\e[31m'; G=$'\e[32m'; Y=$'\e[33m'; D=$'\e[2m'; B=$'\e[1m'; Z=$'\e[0m'
else
  R=""; G=""; Y=""; D=""; B=""; Z=""
fi
die()  { printf '\n  %serror%s %s\n\n' "$R" "$Z" "$*" >&2; exit 1; }
note() { printf '  %s\n' "$*" >&2; }
step() { printf '\n  %s%s%s\n' "$B" "$*" "$Z" >&2; }
ok()   { printf '  %s✓%s %s\n' "$G" "$Z" "$*" >&2; }
warn() { printf '  %s!%s %s\n' "$Y" "$Z" "$*" >&2; }

usage() {
  cat >&2 <<'USAGE'
skynet virtual-robot — a complete robot with no hardware (§35.3, guest tier)

  virtual-robot.sh up        build, install, provision and start. Needs root.
  virtual-robot.sh status    what the robot says about itself. Needs no root.
  virtual-robot.sh smoke     run the assertions against a robot that is already up.
  virtual-robot.sh down      stop and uninstall. Needs root. Leaves /data.

  SKYNET_ROOT   the tree /data and /run live under. Default /.

  The body is drivers/mock: three revolute joints, roles declared, no envelope, and
  `simulator: true` — so §35.3 caps this robot at guest tier by mechanism.
USAGE
}

need_root() {
  [ "$(id -u)" = "0" ] || die "this needs root: it installs into /usr/bin and /etc and drives
  systemd. Re-run with sudo."
}

need_linux() {
  [ "$(uname -s)" = "Linux" ] || die "Skynet OS is a Linux operating system. This is $(uname -s).
  On Windows, run this inside WSL with systemd enabled, or in a Linux VM."
  [ "$(ps -p 1 -o comm= 2>/dev/null || true)" = "systemd" ] || die "systemd is not PID 1 here.
  §33.1's ordering guarantee lives in the unit files, so a host without systemd can run the
  binaries by hand but cannot run this. On WSL: put

      [boot]
      systemd=true

  in /etc/wsl.conf and run \`wsl --shutdown\` from Windows."
}

# ── build ────────────────────────────────────────────────────────────────────
do_build() {
  step "Building"
  command -v cargo >/dev/null 2>&1 || die "cargo is not installed. The four binaries are Rust."
  ( cd "$REPO" && cargo build --release --workspace ) >&2
  ok "skynetd, skynetk, skynet, skynetp"

  # The body. `lib` rather than `all`: what a robot loads is the shared object, and the selftest
  # beside it is for a driver author rather than for this.
  command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || \
    die "no C compiler. The mock body is one C11 file with no dependencies."
  ( cd "$MOCK_DIR" && make lib ) >&2
  ok "the mock body: $MOCK_DIR/libskynet_mock.so"
}

# ── up ───────────────────────────────────────────────────────────────────────
do_up() {
  need_linux
  need_root
  do_build

  local driver="$MOCK_DIR/libskynet_mock.so"
  [ -f "$driver" ] || die "the mock body did not build at $driver"

  step "Describing the body"
  # From the driver itself, which is the only party that knows. This also runs the sim half of SCTS
  # against it, so a body that cannot answer the ABI is refused here rather than at the first cycle.
  local body="$REPO/target/mock-body.json"
  "$BUILD/skynet" driver check "$driver" --descriptor "$body" >&2 || \
    die "the mock body did not pass the driver-facing suite, so it is not a body to install."
  ok "$body"

  step "Installing"
  "$HERE/install.sh" \
    --from "$BUILD" \
    --driver "$driver" \
    --descriptor "$body" \
    --tier guest \
    --hz 100 \
    --label "virtual-robot" \
    --root "$ROOT" >&2

  step "Starting"
  # §33.1's order, and systemd holds it: skynetk before skynetd, neither requiring the other.
  systemctl start "${UNITS[@]}"
  wait_for_robot
  ok "skynetk and skynetd are running"

  do_smoke
}

# Wait until the robot **answers**, not until a file exists.
#
# This waited on `[ -S "$SOCKET" ]`, and a Unix socket outlives the process that bound it: after a
# `down`, the path is still on disk with nothing behind it. The wait then returned immediately, all
# ten assertions ran against a supervisor that had not finished starting, and the result depended on
# whether this machine had ever had a robot on it before — a bring-up that passes on a clean host
# and fails on a re-run is worse than one that fails on both.
#
# §39.1 makes `status` the one route that needs no credential, so this can ask the robot the same
# question the first assertion asks without having read a token yet.
wait_for_robot() {
  local waited=0
  until "$BUILD/skynet" status --socket "$SOCKET" >/dev/null 2>&1; do
    sleep 0.2
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
      systemctl status "${UNITS[@]}" --no-pager >&2 || true
      die "the supervisor did not answer on $SOCKET within 20 seconds. The unit status is above;
      \`journalctl -u skynetd -n 50\` has the rest."
    fi
  done
}

# ── smoke ────────────────────────────────────────────────────────────────────
#
# The assertions. Each one is a sentence about the robot that was not true of this machine before
# `up` ran, and every one is read back **from the robot** rather than from what the installer
# believes it wrote.
do_smoke() {
  local token
  token="$(sed -n 's/^SKYNET_ADMIN_TOKEN=//p' /etc/skynet/skynetd.env 2>/dev/null | head -1 || true)"
  local skynet="$BUILD/skynet"
  [ -x "$skynet" ] || skynet="skynet"

  local failed=0
  assert() {
    local what="$1"; shift
    if "$@" >/dev/null 2>&1; then
      ok "$what"
    else
      printf '  %sFAIL%s %s\n' "$R" "$Z" "$what" >&2
      failed=1
    fi
  }
  says() {
    # A route answered, and its output contains the word. `--token` is passed to every call: the
    # anonymous routes ignore it and the rest need it, and branching on which is which here would be
    # this script keeping a second copy of §39.1's table.
    local route="$1" word="$2"; shift 2
    "$skynet" "$route" --socket "$SOCKET" --token "$token" 2>&1 | grep -qi -- "$word"
  }

  # The sentence is "answers", so the check has to be an answer. `test -S` was the file, and a
  # socket file with nothing behind it is exactly the state a stopped robot leaves on disk.
  socket_answers() {
    [ -S "$SOCKET" ] || return 1
    "$skynet" status --socket "$SOCKET" --token "$token" >/dev/null 2>&1
  }

  step "What the robot says about itself"
  assert "the local socket answers (§33.7)" socket_answers
  assert "status reports a running supervisor" says status skynetd
  assert "the control loop is up" systemctl is-active --quiet skynetk.service
  assert "the supervisor is up" systemctl is-active --quiet skynetd.service
  assert "§27's operating state is reported" says state state
  assert "the body is the mock arm" says status mock-arm
  assert "the packages route answers" says ps ""
  assert "§7.6's limits resolve for this body" says limits ""
  assert "§15's runtime tests run on this robot" says conformance ""
  assert "the black box has a window" says blackbox ""

  step "The tier this evidence supports"
  if says status guest; then
    ok "guest — §35.3's cap, because the body declares simulator: true"
  else
    warn "the status line did not say guest; read it below rather than trusting this script"
  fi

  "$skynet" status --socket "$SOCKET" --token "$token" >&2 || true

  if [ "$failed" -ne 0 ]; then
    die "at least one assertion failed above. \`journalctl -u skynetd -u skynetk -n 80\` is the
    next thing to read."
  fi

  step "This machine is now a robot"
  note ""
  note "  ${D}skynet status --socket $SOCKET${Z}"
  note "  ${D}skynet conformance --socket $SOCKET --token \$TOKEN${Z}"
  note "  ${D}journalctl -u skynetd -u skynetk -f${Z}"
  note ""
  note "  ${D}Guest tier, and no joint has moved: the body is drivers/mock. §35.3 rules${Z}"
  note "  ${D}that no simulated run is bench evidence, and this robot declares itself one.${Z}"
  note ""
}

do_status() {
  [ -S "$SOCKET" ] || die "nothing is listening on $SOCKET. \`virtual-robot.sh up\` starts one."
  local token
  token="$(sed -n 's/^SKYNET_ADMIN_TOKEN=//p' /etc/skynet/skynetd.env 2>/dev/null | head -1 || true)"
  local skynet="$BUILD/skynet"
  [ -x "$skynet" ] || skynet="skynet"
  "$skynet" status --socket "$SOCKET" --token "$token"
  "$skynet" state --socket "$SOCKET" --token "$token" || true
}

do_down() {
  need_linux
  need_root
  step "Stopping"
  systemctl stop "${UNITS[@]}" 2>/dev/null || true
  ok "units stopped"
  "$HERE/install.sh" --uninstall --root "$ROOT" >&2
  note ""
  note "  ${D}$DATA was left alone. It holds the black box this run produced.${Z}"
  note ""
}

case "${1:-}" in
  up)     do_up ;;
  smoke)  do_smoke ;;
  status) do_status ;;
  down)   do_down ;;
  -h|--help|"") usage; exit 0 ;;
  *)      usage; die "\`$1\` is not a command." ;;
esac
