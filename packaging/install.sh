#!/usr/bin/env bash
#
# Put Skynet OS on a machine (§28, §33.1).
#
# The two unit files beside this script have always described a complete installation — binaries at
# /usr/bin, environment at /etc/skynet, the §28 layout under a root — and nothing created any of it.
# A robot was something you assembled by hand from a session log. This is that assembly, written
# down once, idempotent, and checkable afterwards.
#
#   ./packaging/install.sh --driver /usr/lib/skynet/libskynet_mock.so --descriptor body.json
#   ./packaging/install.sh --check
#   ./packaging/install.sh --uninstall
#
# # What it deliberately does not do
#
# **It does not start the robot.** `--enable` arms the units for the next boot and `--start` runs
# them now; neither is the default, because an installer that begins commanding a machine the moment
# it finishes copying files is one nobody can run while standing next to a robot. The last thing it
# prints is the command to start, so the decision is a person's and it is one line away.
#
# **It does not invent a driver.** §28 gives the descriptor a pinned path and §33.1 forbids the
# supervisor loading a driver to ask what the body is, so both arrive here as arguments. There is no
# default and no probe: a robot that installed with a guessed body is a robot whose guards are
# clamping to somebody else's joint ratings.
#
# **It does not choose a token.** §39.2 has no default password at any point in the lifecycle. The
# admin credential is issued by `skynetd credential add`, which takes its randomness from
# /dev/urandom and refuses rather than falling back, and the token is printed here exactly once.
#
# **It never deletes /data.** `--uninstall` removes what it installed. A robot's black box, identity
# and desired state are evidence, and an uninstaller that took them with it would destroy the record
# of whatever prompted the uninstall. `--purge` is the flag that says otherwise, and it asks.

set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$HERE/.." && pwd)"

# ── defaults ─────────────────────────────────────────────────────────────────
FROM="$REPO/target/release"
# A release bundle carries the binaries at bin/ and no sources; see packaging/bundle.sh.
if [ -x "$REPO/bin/skynetd" ] && [ ! -f "$REPO/Cargo.toml" ]; then FROM="$REPO/bin"; fi
PREFIX="/usr"
ROOT="/"
DRIVER=""
DRIVER_CONFIG=""
DESCRIPTOR=""
TIER="guest"
HZ="100"
LABEL=""
LAN=""
INTERVAL_MS="1000"
RT_CPU="3"
GENERAL_CPUS="0-2"
ACTION="install"
DO_ENABLE="no"
DO_START="no"
PURGE="no"
LOG_FILE=""
LOGGING="no"

BINARIES=(skynetd skynetk skynet skynetp)
UNITS=(skynetk.service skynetd.service)

# ── output ───────────────────────────────────────────────────────────────────
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
  R=$'\e[31m'; G=$'\e[32m'; Y=$'\e[33m'; D=$'\e[2m'; B=$'\e[1m'; Z=$'\e[0m'
else
  R=""; G=""; Y=""; D=""; B=""; Z=""
fi

# One reader for the `KEY=value` lines in the environment files this script writes, because the
# question "does this installation already have an admin token" is asked in two places — once to
# decide whether to issue one and once to carry it forward — and two spellings of it is how a robot
# ends up with a credential store and a blank token.
env_value() {
  [ -f "$2" ] || return 0
  sed -n "s/^$1=//p" "$2" | head -1
}

# Every line goes through `emit`: the terminal as-is, the log file without color codes. The
# file is best-effort (`|| true`) and the terminal is not — a full disk must not turn a failure
# report into a second failure, and the terminal is the copy a person is watching.
emit() {
  printf '%s\n' "$1" >&2
  if [ "$LOGGING" = "yes" ]; then
    local clean="$1"
    # The six are assigned together, all or nothing, so one guard covers all six — and an empty
    # pattern in `${var//pattern/}` is a no-op only by luck, which is why the guard exists.
    if [ -n "$R$G$Y$D$B$Z" ]; then
      clean="${clean//$R/}"; clean="${clean//$G/}"; clean="${clean//$Y/}"
      clean="${clean//$D/}"; clean="${clean//$B/}"; clean="${clean//$Z/}"
    fi
    printf '%s\n' "$clean" >> "$LOG_FILE" || true
  fi
}

# A finished step, numbered and timed: `[3] (+45ms)`. Numbered without a total, because the
# install path branches — a re-run issues no credential, `--enable` adds a step — and a total
# that lies on some paths is worse than none. The PC wizard parses these lines for progress, so
# the shape `✓ [N] (+Nms)` is a contract, not decoration.
STEP=0
LAST_MS="$(date +%s%3N)"
ok() {
  STEP=$((STEP + 1))
  local now dt
  now="$(date +%s%3N)"
  dt=$((now - LAST_MS)); LAST_MS=$now
  emit "  ${G}✓${Z} [$STEP] (+${dt}ms) $*"
}

DIED="no"
die()  { DIED="yes"; emit ""; emit "  ${R}error${Z} $*"; emit ""; exit 1; }
note() { emit "  $*"; }
warn() { emit "  ${Y}!${Z} $*"; }

# A command whose failure ends the run, with a sentence rather than `set -e`'s silence — and
# the command's own words carried into it, because a failure without them ends where the
# debugging starts. Success is output-neutral: these commands are silent when they work, and
# chatter passes through as a note when they are not.
run() {
  local what="$1"; shift
  local out
  if out="$("$@" 2>&1)"; then
    if [ -n "$out" ]; then note "$out"; fi
    return 0
  fi
  if [ -n "$out" ]; then what="$what: $out"; fi
  die "$what"
}

# The backstop behind `die` and `run`: any other nonzero exit — a bare `install` failing, a
# `${2:?...}` tripping — still gets a sentence, with the step it died after. `DIED` keeps this
# quiet on the paths that already spoke.
on_exit() {
  local code=$?
  if [ "$code" -ne 0 ] && [ "$DIED" != "yes" ]; then
    DIED="yes"
    local where="before its first step"
    if [ "$STEP" -gt 0 ]; then where="after step $STEP"; fi
    emit ""
    emit "  ${R}error${Z} the installer stopped early (exit $code) $where: a command failed before it could say why."
    if [ "$LOGGING" = "yes" ]; then
      emit "  everything it said is in $LOG_FILE."
    else
      emit "  re-run with --log-file <path> to keep everything it said."
    fi
    emit ""
  fi
}
trap 'on_exit' EXIT

usage() {
  cat >&2 <<'USAGE'
skynet install — put Skynet OS on this machine (§28)

  install.sh --driver <path> --descriptor <file> [options]
  install.sh --check
  install.sh --uninstall [--purge]

  --driver <path>       The RHAL shared object this robot's body is behind. Required:
                        §33.1 forbids the supervisor loading one to find out, so it is
                        named here and nowhere else.
  --descriptor <file>   The body (§5), written to <root>/data/state/embodiment.json.
                        `skynet driver check <lib> --descriptor body.json` produces one
                        from the driver itself, which is how a bench robot is provisioned.
  --driver-config <s>   Passed verbatim to the driver's open(). Default empty.

  --from <dir>          Where the built binaries are. Default target/release.
  --prefix <dir>        Where they go. Default /usr.
  --root <dir>          SKYNET_ROOT — the tree /data and /run live under. Default /.
  --tier guest|host|certified      Default guest.
  --hz <n>              Control rate. Default 100.
  --label <name>        The name an mDNS browse shows (§82). Default: this host's name.
  --lan <addr>          The on-robot console address (§32.2). §34.2 refuses a routable
                        one. Omitted means no console on the LAN and nothing advertised.
  --interval <ms>       Reconcile interval. Default 1000.
  --rt-cpu <n>          The isolated core the control loop gets (§40.2). Default 3.
  --general-cpus <set>  The cores everything else may use. Default 0-2.

  --log-file <path>   Mirror every line to this file as well as the terminal, without
                        color codes. The file is truncated at the start of the run, so it
                        always describes the latest one.

  --enable              Arm the units for the next boot.
  --start               Start them now. Implies --enable.
  --check               Verify an existing installation and exit. Nothing is written.
  --uninstall           Remove what this script installed. Never /data.
  --purge               With --uninstall: remove /data too. Asks first.

  Exit is 0 only when every postcondition holds. --check is the same set of assertions
  run against an installation somebody else made.
USAGE
}

# ── arguments ────────────────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --from)          FROM="${2:?--from needs a directory}"; shift 2 ;;
    --prefix)        PREFIX="${2:?--prefix needs a directory}"; shift 2 ;;
    --root)          ROOT="${2:?--root needs a directory}"; shift 2 ;;
    --driver)        DRIVER="${2:?--driver needs a path}"; shift 2 ;;
    --driver-config) DRIVER_CONFIG="${2:?--driver-config needs a string}"; shift 2 ;;
    --descriptor)    DESCRIPTOR="${2:?--descriptor needs a file}"; shift 2 ;;
    --tier)          TIER="${2:?--tier needs a value}"; shift 2 ;;
    --hz)            HZ="${2:?--hz needs a number}"; shift 2 ;;
    --label)         LABEL="${2:?--label needs a name}"; shift 2 ;;
    --lan)           LAN="${2:?--lan needs an address}"; shift 2 ;;
    --interval)      INTERVAL_MS="${2:?--interval needs a number}"; shift 2 ;;
    --rt-cpu)        RT_CPU="${2:?--rt-cpu needs a number}"; shift 2 ;;
    --general-cpus)  GENERAL_CPUS="${2:?--general-cpus needs a set}"; shift 2 ;;
    --log-file)      LOG_FILE="${2:?--log-file needs a path}"; shift 2 ;;
    --enable)        DO_ENABLE="yes"; shift ;;
    --start)         DO_START="yes"; DO_ENABLE="yes"; shift ;;
    --check)         ACTION="check"; shift ;;
    --uninstall)     ACTION="uninstall"; shift ;;
    --purge)         PURGE="yes"; shift ;;
    -h|--help)       usage; exit 0 ;;
    *)               usage; die "\`$1\` is not an option." ;;
  esac
done

# The log is validated before anything else runs — including `require_host` — so a bad path
# fails as a bad path rather than as a missing systemd. Truncated, not appended: see --log-file.
if [ -n "$LOG_FILE" ]; then
  # A subshell, because the `2>/dev/null` has to be in place before the failing redirection is
  # attempted: on the same command it suppresses nothing and bash's own "No such file" leaks past
  # the sentence below.
  ( : > "$LOG_FILE" ) 2>/dev/null || die "cannot write the log to $LOG_FILE."
  LOGGING="yes"
  note "logging this run to $LOG_FILE"
fi

ETC="/etc/skynet"
UNIT_DIR="/lib/systemd/system"
# An `if` and not `[ -d ... ] && UNIT_DIR=...`, here and at the two other guards in this file.
# Under `set -e` a bare `&&` list whose left side is false *is* a failed command, so on a
# distribution that has only /lib/systemd/system this script would have exited 1 before printing
# anything at all — the quietest possible failure, in the one tool whose whole job is to say what
# it did.
if [ -d /usr/lib/systemd/system ]; then UNIT_DIR="/usr/lib/systemd/system"; fi
# Where the resolved half of each unit goes (see `install_dropins`). Under /etc rather than beside
# the unit: /etc/systemd/system overrides /usr/lib/systemd/system, which is the direction a local
# installation is supposed to win in, and it leaves the unit file on the robot byte-identical to
# the one in this repository — so `systemctl cat` shows the argument and the machine's answer to it
# as two separate, readable things.
DROPIN_DIR="/etc/systemd/system"
LIB="$PREFIX/lib/skynet"
DOC="$PREFIX/share/doc/skynet"
DATA="${ROOT%/}/data"
RUN="${ROOT%/}/run"

# The units name /usr/bin explicitly, so a --prefix that puts the binaries elsewhere would install
# a robot whose units point at nothing. Caught here rather than at the first boot.
if [ "$ACTION" = "install" ] && [ "$PREFIX" != "/usr" ]; then
  die "the unit files name /usr/bin/skynetd and /usr/bin/skynetk literally, so --prefix $PREFIX
  would install units that point at nothing. Either install under /usr, or edit the units and
  this check together."
fi

# ── the host has to be one these units can run on ────────────────────────────
require_host() {
  [ "$(uname -s)" = "Linux" ] || die "Skynet OS is a Linux operating system (§23.5 asks for
  PREEMPT_RT). This is $(uname -s)."
  command -v systemctl >/dev/null 2>&1 || die "systemctl is not on this machine. The two unit
  files are the installation: §33.1's ordering guarantee — the control loop starts before the
  supervisor and survives its restart — is expressed in them and nowhere else."
  [ "$(ps -p 1 -o comm= 2>/dev/null || true)" = "systemd" ] || die "systemd is not PID 1 here, so
  the units would install and never run. On WSL, set systemd=true in /etc/wsl.conf and restart the
  distribution."
}

need_write() {
  for path in "$@"; do
    local probe="$path"
    while [ ! -e "$probe" ] && [ "$probe" != "/" ]; do probe="$(dirname "$probe")"; done
    [ -w "$probe" ] || die "$path is not writable by $(id -un). Re-run with sudo."
  done
}

# ── install ──────────────────────────────────────────────────────────────────
do_install() {
  require_host

  [ -n "$DRIVER" ] || { usage; die "--driver is required. There is no default and no probe: §33.1
  forbids the supervisor loading a driver to discover the body, so a robot installed without one
  is a robot with nothing that can write to an actuator. For a bench with no hardware, build
  drivers/mock and pass the shared object it produces."; }
  [ -f "$DRIVER" ] || die "no driver at $DRIVER"

  [ -n "$DESCRIPTOR" ] || { usage; die "--descriptor is required. §28 pins the body at
  state/embodiment.json, and a robot with no descriptor records a zero embodiment digest, cannot
  answer the capability half of an admission decision, and reports both pose guards unavailable.
  \`skynet driver check $DRIVER --descriptor body.json\` writes one from the driver itself."; }
  [ -f "$DESCRIPTOR" ] || die "no descriptor at $DESCRIPTOR"

  case "$TIER" in guest|host|certified) ;; *) die "--tier is guest, host or certified, not \`$TIER\`" ;; esac

  for binary in "${BINARIES[@]}"; do
    [ -f "$FROM/$binary" ] || die "no $binary in $FROM. Build them first:
      cargo build --release --workspace"
  done

  need_write "$PREFIX/bin" "$ETC" "$UNIT_DIR" "$DROPIN_DIR" "$DATA" "$RUN"

  note ""
  note "${B}Skynet OS${Z} ${D}→ $PREFIX, $ETC, $UNIT_DIR, root $ROOT${Z}"
  note ""

  # Binaries. `install` rather than `cp` so the mode is set in the same call that writes the file,
  # and a partially-copied binary is never briefly executable.
  install -d -m 0755 "$PREFIX/bin" "$LIB" "$DOC"
  for binary in "${BINARIES[@]}"; do
    install -m 0755 "$FROM/$binary" "$PREFIX/bin/$binary"
  done
  ok "binaries: ${BINARIES[*]}"

  local driver_target="$LIB/$(basename "$DRIVER")"
  install -m 0755 "$DRIVER" "$driver_target"
  ok "driver: $driver_target"

  for document in skynet-os-architecture.md; do
    if [ -f "$REPO/docs/$document" ]; then
      install -m 0644 "$REPO/docs/$document" "$DOC/$document"
    fi
  done

  # §28's layout. Modes matter: `state` holds credentials and trusted keys, `identity` holds a
  # device key, and both are 0700 rather than 0755 so that a read of the tree is not a read of the
  # credential store.
  install -d -m 0755 "$DATA" "$DATA/packages" "$DATA/packages/blobs" "$DATA/packages/active" \
                     "$DATA/blackbox" "$RUN"
  install -d -m 0700 "$DATA/state" "$DATA/identity"
  install -d -m 0755 "$DATA/state/profiles" "$DATA/state/calibration" "$DATA/state/os" \
                     "$DATA/state/outbound" "$DATA/state/entitlements"
  ok "layout: $DATA (packages, blackbox, state, identity) and $RUN"

  # The descriptor, at the pinned path. Copied rather than linked: §28 wants one file that a robot
  # owns, and a symlink to somebody's home directory is a body that disappears on a reinstall.
  install -m 0644 "$DESCRIPTOR" "$DATA/state/embodiment.json"
  ok "body: $DATA/state/embodiment.json"

  if [ -z "$LABEL" ]; then LABEL="$(hostname 2>/dev/null || echo skynet)"; fi

  # §39.2: the credential is issued by the daemon, from /dev/urandom, and printed once. Re-running
  # this script must not invalidate the token an operator wrote down — and the test for that is
  # whether the **environment file** already carries one, not whether the credential store exists.
  #
  # The difference between those two questions is a robot nobody can talk to. `--uninstall` keeps
  # /data on purpose, because a sealed black box window is evidence about whatever prompted the
  # uninstall; it does not keep /etc, because /etc is what it installed. So down-then-up finds a
  # credential store and no environment file, and asking the store issued nothing and then wrote
  # `SKYNET_ADMIN_TOKEN=` — an empty value, which is a supervisor whose ExecStop cannot authenticate
  # and an operator with no way in, on an installation reporting every postcondition met.
  #
  # Issuing here is safe because `skynetd credential add` **appends**: §39.2's store is a list, a
  # second admin credential invalidates no other, and a token written down on the first install goes
  # on working. The case that must never happen is re-issuing while a token is still in the
  # environment file, and that is the one this skips.
  local token=""
  if [ -n "$(env_value SKYNET_ADMIN_TOKEN "$ETC/skynetd.env")" ]; then
    warn "this installation already carries an admin token; none issued."
    warn "SKYNET_ADMIN_TOKEN in $ETC/skynetd.env is left as it was."
  else
    token="$("$PREFIX/bin/skynetd" credential add --root "$ROOT" --label install --role admin)"
    [ -n "$token" ] || die "no credential was issued, so this robot has no admin token and
    ExecStop cannot ask it to shut down cleanly. §39.2 refuses a weak token rather than falling
    back, so the cause is /dev/urandom."
    ok "admin credential issued"
  fi

  write_env "$token"
  install_units

  note ""
  if [ -n "$token" ]; then
    note "${B}The admin token, printed once and stored nowhere but the env file:${Z}"
    note ""
    note "    $token"
    note ""
    note "${D}Nothing keeps it but a salted hash (§39.2). Write it down now.${Z}"
    note ""
  fi

  verify quiet || die "the installation did not verify. Re-run with --check to see which
  postcondition failed."
  ok "every postcondition holds"

  if [ "$DO_ENABLE" = "yes" ]; then
    run "enabling ${UNITS[*]} for the next boot failed" systemctl enable "${UNITS[@]}"
    ok "units enabled for the next boot"
  fi
  if [ "$DO_START" = "yes" ]; then
    run "starting ${UNITS[*]} failed" systemctl start "${UNITS[@]}"
    ok "units started"
  fi

  note ""
  if [ "$DO_START" != "yes" ]; then
    note "${B}Nothing is running.${Z} This installer does not start a robot; that is a decision"
    note "for somebody who can see the machine. When the workspace is clear:"
    note ""
    note "    sudo systemctl start skynetk skynetd"
    note "    skynet status"
    note ""
  else
    note "    skynet status"
    note ""
  fi
}

# ── the environment the units read ───────────────────────────────────────────
write_env() {
  local token="$1"
  install -d -m 0755 "$ETC"

  # 0600 on both: one carries an admin token and the other names the driver a robot loads. Written
  # with `install` so the mode is set as the file is created rather than after it exists.
  local driver_target="$LIB/$(basename "$DRIVER")"

  local existing_token="$token"
  if [ -z "$existing_token" ]; then
    existing_token="$(env_value SKYNET_ADMIN_TOKEN "$ETC/skynetd.env")"
  fi

  # The whole argument, not the address. A unit file has no way to omit one — `${VARIABLE}` is
  # substituted as a single word even when it is empty — so `--lan ${SKYNET_CONSOLE_ADDRESS}` with
  # no console configured passed `--lan ""`, which the supervisor correctly refuses as not an
  # address, on every robot installed without --lan. That is the default. See the note on the
  # ExecStart line in skynetd.service.
  local console=""
  if [ -n "$LAN" ]; then console="--lan $LAN"; fi

  local tmp
  tmp="$(mktemp)"
  cat > "$tmp" <<EOF
# Written by packaging/install.sh. The supervisor's half (§9, §33).
#
# systemd reads this verbatim: no quoting, no expansion, no comments after a value.
SKYNET_ROOT=$ROOT
SKYNET_SOCKET=${RUN}/skynetd.sock
# Empty means no console on the LAN (§32.2) and nothing advertised (§82), and empty here has to
# mean *no argument* rather than an empty one — which is why this is the flag and not the address.
SKYNET_CONSOLE=$console
SKYNET_LABEL=$LABEL
SKYNET_INTERVAL_MS=$INTERVAL_MS
SKYNET_GENERAL_CPUS=$GENERAL_CPUS
# §39.2: the only copy of this token outside an operator's notes. It authenticates ExecStop, which
# is how a robot withdraws its mDNS name and seals the black box window before systemd signals it.
SKYNET_ADMIN_TOKEN=$existing_token
EOF
  install -m 0600 "$tmp" "$ETC/skynetd.env"
  rm -f "$tmp"

  tmp="$(mktemp)"
  cat > "$tmp" <<EOF
# Written by packaging/install.sh. The control loop's half (§25).
SKYNET_ROOT=$ROOT
SKYNET_DRIVER=$driver_target
SKYNET_DRIVER_CONFIG=$DRIVER_CONFIG
SKYNET_TIER=$TIER
SKYNET_HZ=$HZ
SKYNET_RT_CPU=$RT_CPU
EOF
  install -m 0600 "$tmp" "$ETC/skynetk.env"
  rm -f "$tmp"
  ok "environment: $ETC/skynetd.env, $ETC/skynetk.env (0600)"
}

# ── the half of a unit systemd will not expand a variable into ───────────────
#
# systemd substitutes `${VARIABLE}` from the environment file into the `Exec*` lines and **nowhere
# else**. Both units were written as though it did, and the first host they were installed on is
# where that was found out. The two failures are not the same shape:
#
#   `ReadWritePaths=${SKYNET_ROOT}`  is dropped as "not absolute", leaving the sandbox naming only
#                                    `/run/skynet` — a directory nothing here has ever created — so
#                                    every start is a hard `226/NAMESPACE`. Loud, and therefore
#                                    cheap.
#   `CPUAffinity=${SKYNET_RT_CPU}`   is dropped as an unparseable CPU range and **the unit starts
#                                    anyway**. §40.2's isolated core is then a sentence in a
#                                    comment: the loop runs wherever the scheduler puts it, the
#                                    supervisor is free to land on the core the loop needs, and
#                                    nothing anywhere says otherwise.
#
# The values are known here — they are this script's own arguments — so this is where they are
# resolved. The unit files keep the argument and this keeps the machine's answer, and `verify` asks
# systemd which of the two it actually parsed.
install_dropins() {
  local tmp
  for unit in "${UNITS[@]}"; do
    install -d -m 0755 "$DROPIN_DIR/$unit.d"
  done

  # The loop writes the black box and binds the policy socket §25 hands to a policy runtime; it
  # reads everything else.
  tmp="$(mktemp)"
  cat > "$tmp" <<EOF
# Written by packaging/install.sh from --rt-cpu and --root. Change those and re-run; an edit here
# is overwritten by the next install.
[Service]
CPUAffinity=$RT_CPU
ReadWritePaths=$RUN $DATA/blackbox
EOF
  install -m 0644 "$tmp" "$DROPIN_DIR/skynetk.service.d/10-install.conf"
  rm -f "$tmp"

  # The supervisor owns the whole §28 tree and the socket in it. Named as two paths rather than as
  # the root itself: with --root / the old `ReadWritePaths=${SKYNET_ROOT}` would have remounted the
  # entire filesystem writable, which is `ProtectSystem=strict` spelled out and then undone.
  tmp="$(mktemp)"
  cat > "$tmp" <<EOF
# Written by packaging/install.sh from --general-cpus and --root. Change those and re-run; an edit
# here is overwritten by the next install.
[Service]
CPUAffinity=$GENERAL_CPUS
ReadWritePaths=$RUN $DATA
EOF
  install -m 0644 "$tmp" "$DROPIN_DIR/skynetd.service.d/10-install.conf"
  rm -f "$tmp"
  ok "cpusets and writable paths: $DROPIN_DIR/{skynetk,skynetd}.service.d/10-install.conf"
}

install_units() {
  for unit in "${UNITS[@]}"; do
    install -m 0644 "$HERE/systemd/$unit" "$UNIT_DIR/$unit"
  done
  install_dropins
  run "systemctl daemon-reload failed" systemctl daemon-reload
  ok "units: $UNIT_DIR/{${UNITS[0]},${UNITS[1]}}"
}

# ── verify ───────────────────────────────────────────────────────────────────
#
# The gate. Every assertion here is a postcondition of `install`, and running it against an
# installation somebody else made answers the same question — which is the point of it being a
# separate mode rather than a tail of the install.
verify() {
  local quiet="${1:-}"
  FAILED=0

  # A predicate and a sentence, run directly rather than through `eval`. The earlier draft built
  # shell out of strings and had to escape a `stat` inside a quoted string inside a quoted string;
  # what that produces is a check that is hard to read and, when it is wrong, silently passes.
  check() {
    local what="$1"; shift
    if "$@" >/dev/null 2>&1; then
      [ "$quiet" = "quiet" ] || ok "$what"
    else
      emit "  ${R}FAIL${Z} $what"
      FAILED=1
    fi
  }
  mode_is() { [ "$(stat -c %a "$2" 2>/dev/null)" = "$1" ]; }
  looks_like_json() {
    [ -s "$1" ] || return 1
    # The first non-blank character. A descriptor is one object, and a file that begins with
    # anything else is the case §28 calls *malformed* rather than absent — a generator emitting
    # something wrong, which implies different work from a provisioning step that never ran.
    [ "$(tr -d '[:space:]' < "$1" | cut -c1)" = "{" ]
  }
  names_a_value() { [ -n "$(env_value "$1" "$2")" ]; }
  names_a_file() {
    local value
    value="$(env_value "$1" "$2")"
    [ -n "$value" ] && [ -f "$value" ]
  }

  # The two below ask **systemd** what it parsed rather than asking the unit file what it says, and
  # that distinction is the whole reason they exist. A directive systemd could not parse is not an
  # error: it is dropped, logged once at the level nobody reads, and the unit starts without it. So
  # a unit file that names a cpuset is not evidence that the process is pinned to one, and reading
  # the file back would have agreed with the file every time (§6.4 — a check over the input cannot
  # see a defect in what the input produced).
  affinity_parsed() {
    # Empty is what a dropped `CPUAffinity=` leaves behind, and an empty mask is not a state
    # systemd reports for a unit that has one — so emptiness is the assertion. The value itself is
    # not compared: systemd normalises the set it prints, and a check that fails on `0-2` vs `0 1 2`
    # is a check somebody turns off.
    [ -n "$(systemctl show -p CPUAffinity --value "$1" 2>/dev/null)" ]
  }
  rw_paths_exist() {
    # Under ProtectSystem=strict every writable path has to be named, and a name that is not there
    # is not a warning — the namespace cannot be built and the unit dies at `226/NAMESPACE` before
    # its binary is reached. This catches that at install time, where the message can say which
    # path, rather than at the first start on a robot.
    local paths path
    paths="$(systemctl show -p ReadWritePaths --value "$1" 2>/dev/null)"
    [ -n "$paths" ] || return 1
    # Unquoted on purpose: systemd prints the set space-separated, and none of §28's paths may
    # contain a space — `install -d` would have made a different directory than the one meant.
    for path in $paths; do
      [ -d "$path" ] || return 1
    done
  }
  notify_type() {
    # `Type=notify` is what makes `READY=1` and the watchdog real. Under any other type the
    # daemon's notifications are accepted and ignored — a hung loop nobody restarts, wearing a
    # unit file that says otherwise. That was the state of this file until the watchdog was
    # wired: the daemon pinged and systemd was not watching.
    [ "$(systemctl show -p Type --value "$1" 2>/dev/null)" = "notify" ]
  }
  watchdog_is() {
    # WatchdogUSec is what systemd parsed, in microseconds: 120000000 is WatchdogSec=120s.
    # Compared exactly, because a watchdog at the wrong period is a hung loop restarted late
    # or a slow pass killed early, and neither reads as what it is in the journal.
    [ "$(systemctl show -p WatchdogUSec --value "$1" 2>/dev/null)" = "$2" ]
  }
  watchdog_absent() {
    # The deliberate asymmetry, pinned: skynetk must NOT be watchdogged. A hung control loop
    # holds — restarting it would re-open the driver and command hardware again on the
    # strength of a timer (§33.5). Zero or unset is the assertion either way.
    local usec
    usec="$(systemctl show -p WatchdogUSec --value "$1" 2>/dev/null)"
    [ -z "$usec" ] || [ "$usec" = "0" ]
  }
  exec_mentions() {
    # ExecStart as systemd parsed it, so a flag dropped by a unit edit fails here rather than
    # as a kernel that dies unwitnessed on the next panic.
    systemctl show -p ExecStart --value "$1" 2>/dev/null | grep -q -- "$2"
  }

  for binary in "${BINARIES[@]}"; do
    check "$PREFIX/bin/$binary is installed and executable" test -x "$PREFIX/bin/$binary"
  done

  check "$ETC/skynetd.env exists" test -f "$ETC/skynetd.env"
  check "$ETC/skynetk.env exists" test -f "$ETC/skynetk.env"
  # The mode is the assertion, not the existence: an env file carrying an admin token that anybody
  # on the machine can read is §39.2 undone by a umask.
  check "$ETC/skynetd.env is 0600" mode_is 600 "$ETC/skynetd.env"
  check "$ETC/skynetk.env is 0600" mode_is 600 "$ETC/skynetk.env"
  check "the driver named in skynetk.env is there" names_a_file SKYNET_DRIVER "$ETC/skynetk.env"
  # Not "a credential store exists" — that is the line below, and the two can disagree. This is the
  # one an operator feels: an empty value here is a robot that is provisioned, running, and
  # unreachable, including by its own unit's ExecStop.
  check "skynetd.env carries an admin token (§39.2)" \
    names_a_value SKYNET_ADMIN_TOKEN "$ETC/skynetd.env"

  check "$DATA/state/embodiment.json exists (§28)" test -f "$DATA/state/embodiment.json"
  # Not a full decode — that is the daemon's, and it keeps absent and malformed apart by name. This
  # is the cheap half, and it catches the common accident: an empty file, or an error message
  # somebody redirected into the path.
  check "the descriptor is a JSON object" looks_like_json "$DATA/state/embodiment.json"

  check "$DATA/packages/blobs exists" test -d "$DATA/packages/blobs"
  check "$DATA/packages/active exists" test -d "$DATA/packages/active"
  check "$DATA/blackbox exists" test -d "$DATA/blackbox"
  check "$DATA/state is 0700" mode_is 700 "$DATA/state"
  check "$DATA/identity is 0700" mode_is 700 "$DATA/identity"
  check "this robot is provisioned (§39.2)" test -f "$DATA/state/credentials.json"

  for unit in "${UNITS[@]}"; do
    check "$unit is installed" test -f "$UNIT_DIR/$unit"
    check "systemd knows $unit" systemctl cat "$unit"
    check "$unit carries this machine's resolved half" \
      test -f "$DROPIN_DIR/$unit.d/10-install.conf"
    check "systemd parsed a cpuset for $unit (§40.2)" affinity_parsed "$unit"
    check "every writable path $unit names is there" rw_paths_exist "$unit"
  done

  # The watchdog pair, asked of systemd rather than of the files for the reason above. The
  # supervisor's hang is restarted (reconciliation is idempotent); the control loop's is held
  # (restarting would re-open the driver — §33.5). Each direction is asserted, so neither can
  # drift: a missing WatchdogSec on skynetd is a hung loop nobody restarts, and one added to
  # skynetk is a hung loop restarted into unknown hardware state.
  check "skynetd is Type=notify (READY=1 and the watchdog are real)" \
    notify_type skynetd.service
  check "skynetd is watchdogged at 120s" watchdog_is skynetd.service 120000000
  check "skynetk is NOT watchdogged (a hung loop holds, §33.5)" \
    watchdog_absent skynetk.service
  # Without the flag the kernel's panics leave only the journal; with it the supervisor
  # takes the report into its black box and onto `/health`.
  check "skynetk reports its crashes (ExecStart carries --crash-file)" \
    exec_mentions skynetk.service --crash-file

  return $FAILED
}

# ── the machine, as §23.5 needs it ───────────────────────────────────────────
#
# Readings, not postconditions: `verify` asserts what `install` wrote, and install writes
# neither the swap configuration nor the kernel — so these report (`ok`/`warn`) and never fail.
# A `check()` here would blame the installation for the machine it is on.
report_machine() {
  # Swap, from the kernel's own accounting: `SwapTotal` in /proc/meminfo is every byte the
  # machine would page to, and §23.5 wants none. Unreadable is reported rather than zero —
  # F22's lesson in `DRY-COVERAGE-PLAN.md`: a probe that cannot run must not read as the good
  # answer.
  local swap_kb=""
  if [ -r /proc/meminfo ]; then
    swap_kb="$(awk '/^SwapTotal:/ {print $2}' /proc/meminfo)"
  fi
  case "$swap_kb" in
    ""|*[!0-9]*)
      warn "swap could not be read from /proc/meminfo — §23.5 wants no swap, and this machine cannot say."
      ;;
    0)
      ok "no swap active (§23.5)"
      ;;
    *)
      warn "swap: ${swap_kb} kB active — §23.5 wants no swap on a robot. \`swapoff -a\` now, and the fstab entry for the next boot."
      ;;
  esac

  # The RT kernel, from the one bit that says so. F57: this script mentioned §23.5 asking for
  # PREEMPT_RT without checking for it; this is the check, and it is still an advisory — the
  # refusal is `docs/issues/who-checks-the-real-time-kernel.md`, undecided.
  local rt=""
  if [ -r /sys/kernel/realtime ]; then
    rt="$(tr -d '[:space:]' < /sys/kernel/realtime)"
  fi
  if [ "$rt" = "1" ]; then
    ok "PREEMPT_RT kernel (§23.5)"
  elif [ "$rt" = "0" ]; then
    warn "stock kernel — §23.5 wants PREEMPT_RT (reported, not refused)"
  else
    warn "/sys/kernel/realtime could not be read — §23.5 wants PREEMPT_RT, and this machine cannot say."
  fi
}

do_check() {
  require_host
  note ""
  note "${B}Skynet OS${Z} ${D}— checking the installation at $PREFIX, $ETC, root $ROOT${Z}"
  note ""
  report_machine
  note ""
  if verify; then
    note ""
    ok "every postcondition holds"
    note ""
    return 0
  fi
  note ""
  die "this installation is incomplete. Re-run install.sh to repair it; it is idempotent and will
  not re-issue a credential that already exists."
}

# ── uninstall ────────────────────────────────────────────────────────────────
do_uninstall() {
  require_host
  note ""
  note "${B}Removing Skynet OS${Z}"
  note ""

  systemctl stop "${UNITS[@]}" 2>/dev/null || true
  systemctl disable "${UNITS[@]}" >/dev/null 2>&1 || true
  for unit in "${UNITS[@]}"; do
    rm -f "$UNIT_DIR/$unit" "$DROPIN_DIR/$unit.d/10-install.conf"
    # Only the file this script wrote, and the directory only if that was all it held: a drop-in an
    # operator put beside it is theirs, and an uninstaller that takes somebody else's configuration
    # with it is one nobody runs twice.
    rmdir "$DROPIN_DIR/$unit.d" 2>/dev/null || true
  done
  systemctl daemon-reload
  ok "units stopped, disabled and removed"

  for binary in "${BINARIES[@]}"; do rm -f "$PREFIX/bin/$binary"; done
  rm -rf "$LIB" "$DOC"
  ok "binaries and driver removed"

  rm -f "$ETC/skynetd.env" "$ETC/skynetk.env"
  rmdir "$ETC" 2>/dev/null || true
  ok "environment removed"

  if [ "$PURGE" = "yes" ]; then
    note ""
    warn "--purge removes $DATA: the black box, this robot's identity, its credentials,"
    warn "its grants and its desired state. A sealed window is evidence about whatever"
    warn "happened, and this is not reversible."
    printf '  type the word REMOVE to confirm: ' >&2
    local answer=""
    read -r answer || true
    if [ "$answer" = "REMOVE" ]; then
      rm -rf "$DATA"
      ok "$DATA removed"
    else
      note "  nothing under $DATA was touched."
    fi
  else
    note ""
    note "${D}$DATA was left alone: it holds the black box, the identity and the desired state.${Z}"
    note "${D}--purge removes it, and asks first.${Z}"
  fi
  note ""
}

case "$ACTION" in
  install)   do_install ;;
  check)     do_check ;;
  uninstall) do_uninstall ;;
esac
