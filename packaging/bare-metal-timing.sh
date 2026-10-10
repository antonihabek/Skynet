#!/usr/bin/env bash
#
# 12.C.5's one run on a machine nobody else is scheduling: boot an ordinary PC from an Ubuntu 24.04 live USB (no install), get this checkout onto it, and run
#
#   bash packaging/bare-metal-timing.sh            # builds, fetches the two kernels, runs `rt-vm.sh timing`, writes ~/skynet-timing-<host>-<date>.txt
#   bash packaging/bare-metal-timing.sh --check    # only says whether this machine can give the measurement, and what is missing
#
# Why a PC and not WSL, a cloud VM or a hosted runner: `rt-vm.sh timing` boots a guest and measures its loop's intervals, and a guest on a host that is itself a guest (WSL2 under Hyper-V, a
# cloud VM, GitHub's runners) has its hypervisor's scheduler under the clock, so the number is the hypervisor's and not the kernel's (F211). On a PC booted from a live USB there is no hypervisor
# above the guest but QEMU's own KVM, which is what the run is for. The script refuses when the CPU reports one (`hypervisor` in /proc/cpuinfo), and `FORCE=1` runs it anyway and says so in the report.
#
# Needs: Ubuntu 24.04 (live USB is enough), a network for the first run (apt, rustup, the two kernels), about 6 GB of free RAM-disk or disk, and a user who may open /dev/kvm (the live session's `ubuntu`
# user can, after `sudo usermod -aG kvm $USER` and a new shell; the script does that for you when it can use sudo).
#
# The result is a text file: nothing in it is a pass or a fail until a person has read it against the budgets `rt-vm.sh timing` prints, and the plan's 12.C.5 entry is where it goes.

set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$HERE/.." && pwd)"
WORK="${RTVM_DIR:-$HOME/rt-vm}"
CHECK_ONLY=0
[ "${1:-}" = "--check" ] && CHECK_ONLY=1

problems=()
say() { printf '%s\n' "$*"; }

say "== this machine"
say "   $(uname -srm)"
say "   cpu: $(grep -m1 'model name' /proc/cpuinfo | sed 's/.*: //')"
say "   cores: $(nproc)"

if grep -qw hypervisor /proc/cpuinfo 2>/dev/null; then
  if [ "${FORCE:-}" = "1" ]; then
    say "   a hypervisor is above this machine (FORCE=1: the report will say so, and its numbers are the host-side budget and not a finding about the kernel)"
  else
    problems+=("the CPU reports a hypervisor above it (WSL2, a cloud VM or a hosted runner): the run would measure that hypervisor's scheduler. Boot a PC from a live USB, or FORCE=1 to run it anyway")
  fi
else
  say "   no hypervisor above it"
fi

if [ ! -e /dev/kvm ]; then
  problems+=("/dev/kvm does not exist: virtualisation is off in the firmware (Intel VT-x / AMD-V / SVM), or this is a machine without it")
elif [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
  problems+=("/dev/kvm is not readable and writable by $USER: sudo usermod -aG kvm $USER, then log in again (or run this under 'sg kvm')")
else
  say "   /dev/kvm: usable"
fi

for tool in cargo make gcc curl apt-get; do
  command -v "$tool" >/dev/null 2>&1 || problems+=("'$tool' is not installed (the script installs build-essential, curl and rustup when it can use sudo)")
done

if [ "${#problems[@]}" -gt 0 ]; then
  say ""
  say "== not ready:"
  for p in "${problems[@]}"; do say "   - $p"; done
  [ "$CHECK_ONLY" = 1 ] && exit 1
fi

[ "$CHECK_ONLY" = 1 ] && { say ""; say "== ready"; exit 0; }

if grep -qw hypervisor /proc/cpuinfo 2>/dev/null && [ "${FORCE:-}" != "1" ]; then
  say "== refusing: a hypervisor is above this machine (see above)"; exit 1
fi

say ""
say "== tools"
if ! command -v cargo >/dev/null 2>&1; then
  sudo apt-get update -qq
  sudo apt-get install -y -qq build-essential curl ca-certificates pkg-config
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi
if [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
  sudo usermod -aG kvm "$USER" || true
  say "   added $USER to the kvm group: running the rest under it"
  exec sg kvm -c "bash '$0' $*"
fi

say ""
say "== build (the loop and the mock body the guest runs)"
cargo build --manifest-path "$REPO/Cargo.toml" --release -q -p skynet-kernel-host
make -C "$REPO/drivers/mock" >/dev/null

say ""
say "== the two kernels and QEMU (apt-get download into $WORK; nothing is installed system-wide)"
bash "$HERE/rt-vm.sh" fetch "$WORK"

OUT="$HOME/skynet-timing-$(hostname)-$(date +%Y%m%d-%H%M%S).txt"
{
  say "machine: $(hostname), $(uname -srm)"
  say "cpu: $(grep -m1 'model name' /proc/cpuinfo | sed 's/.*: //') ($(nproc) threads)"
  say "hypervisor above it: $(grep -qw hypervisor /proc/cpuinfo && echo yes || echo no)"
  say "commit: $(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)$(git -C "$REPO" diff --quiet 2>/dev/null || echo ' + uncommitted changes')"
  say "date: $(date -u +%FT%TZ)"
  say ""
  bash "$HERE/rt-vm.sh" timing "$WORK"
} 2>&1 | tee "$OUT"

say ""
say "== written: $OUT"
say "   copy it off the live USB (it is in RAM), and give it to the plan's 12.C.5 entry."
