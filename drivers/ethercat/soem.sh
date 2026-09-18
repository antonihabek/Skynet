#!/bin/sh
# SOEM, an independent EtherCAT master, against this repository's slave stack.
#
# `DRY-COVERAGE-PLAN.md` F48: SOEM 2.x is GPLv3-or-commercial, so this Apache-2.0 driver cannot link
# it — and its one unrestricted role is an **independent master**, run as a separate program against
# these slaves over an interface. That is the sharpest test this row has, because everything else in
# `make selftest` is this repository's master agreeing with this repository's slaves, and a master
# and a slave written by one author agree whether or not either agrees with ETG.1000.
#
#   make check-soem SOEM=~/src/SOEM/build
#
# What runs, inside a network namespace this script makes with `unshare -rn` — so no root, no NIC,
# and nothing outside the namespace can see a frame:
#
#   ecat1   `python -m skynet_ethercat.wire` serves a ring of simulated drives
#   ecat0   SOEM's own `slaveinfo -map` and `simple_ng`, unmodified, as their authors ship them;
#           then this repository's master over `RawSocketSegment`, which had never run before
#
# Needs: Linux, `ip` (iproute2), unprivileged user namespaces, python3, and a SOEM build directory.
# SOEM is not in this repository and is not downloaded by it; build it yourself (cmake -B build).
set -eu

if [ -z "${SOEM:-}" ]; then
  echo "::error::SOEM is not set. Point it at a SOEM build directory:"
  echo ""
  echo "  git clone --branch v2.0.0 https://github.com/OpenEtherCATsociety/SOEM.git"
  echo "  cmake -S SOEM -B SOEM/build && cmake --build SOEM/build"
  echo "  make check-soem SOEM=\$PWD/SOEM/build"
  echo ""
  echo "It is not fetched for you on purpose: SOEM is GPLv3-or-commercial, and this repository"
  echo "does not distribute it — it runs it, as a separate program, against these slaves."
  exit 1
fi
SLAVEINFO="$SOEM/samples/slaveinfo/slaveinfo"
SIMPLE="$SOEM/samples/simple_ng/simple_ng"
for tool in "$SLAVEINFO" "$SIMPLE"; do
  if [ ! -x "$tool" ]; then
    echo "::error::$tool is not an executable. SOEM=$SOEM should be the build directory, with"
    echo "samples/slaveinfo/slaveinfo and samples/simple_ng/simple_ng in it."
    exit 1
  fi
done

# Re-executed inside a user and network namespace, the way `drivers/python-host/loop.sh` does for
# §11: `-r` maps this user to root inside, which is what makes creating a veth pair and opening a
# raw socket on it unprivileged, and `-n` means neither end is a real interface.
if [ "${SKYNET_SOEM_REEXEC:-}" != "1" ]; then
  if ! command -v unshare >/dev/null 2>&1 || ! unshare -rn /bin/true 2>/dev/null; then
    echo "::error::no unprivileged user and network namespace here, so there is nowhere to put a"
    echo "veth pair without root. Reported as not run rather than run on a real interface."
    exit 1
  fi
  SKYNET_SOEM_REEXEC=1 exec unshare -rn /bin/sh "$0" "$@"
fi

cd "$(dirname "$0")"
DRIVES=${DRIVES:-3}
PYTHON=${PYTHON:-python3}
SECONDS_OF_CYCLES=${SECONDS_OF_CYCLES:-8}
OUT=${TMPDIR:-/tmp}/skynet-soem
export PYTHONPATH="../../bindings/python:../canopen:."

if ! command -v ip >/dev/null 2>&1; then
  echo "::error::\`ip\` (iproute2) is not installed, and it is what makes the veth pair."
  exit 1
fi
ip link add ecat0 type veth peer name ecat1
ip link set ecat0 up
ip link set ecat1 up

"$PYTHON" -m skynet_ethercat.wire --interface ecat1 --drives "$DRIVES" > "$OUT.wire.txt" 2>&1 &
SERVER=$!
trap 'kill "$SERVER" 2>/dev/null || true' EXIT
waited=0
until grep -q '^serving' "$OUT.wire.txt" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ] || ! kill -0 "$SERVER" 2>/dev/null; then
    echo "::error::the slave stack never started serving on ecat1. It said:"
    cat "$OUT.wire.txt"
    exit 1
  fi
  sleep 0.1
done

echo "== SOEM slaveinfo: the SII, the CoE mapping and Safe-Op, read by a master nobody here wrote"
timeout 60 "$SLAVEINFO" ecat0 -map > "$OUT.slaveinfo.txt" 2>&1 || true
cat "$OUT.slaveinfo.txt"

# `simple_ng` runs ten thousand cycles five milliseconds apart and exits 0 whatever happened, so its
# exit code is not the answer — its output is. It is stopped after a bounded interval and read.
echo ""
echo "== SOEM simple_ng: Op, and process data for ${SECONDS_OF_CYCLES} s"
timeout "$SECONDS_OF_CYCLES" "$SIMPLE" ecat0 > "$OUT.simple.raw" 2>&1 || true
tr '\r' '\n' < "$OUT.simple.raw" > "$OUT.simple.txt"
grep -v '^Iteration' "$OUT.simple.txt" || true
grep '^Iteration' "$OUT.simple.txt" | head -3 || true

echo ""
echo "== this repository's master, over RawSocketSegment on the same wire"
"$PYTHON" - "$DRIVES" > "$OUT.master.txt" 2>&1 <<'MASTER' || true
import struct, sys
from skynet_canopen import cia402
from skynet_ethercat.master import EtherCatMaster
from skynet_ethercat.segment import RawSocketSegment

drives = int(sys.argv[1])
master = EtherCatMaster(RawSocketSegment("ecat0"))
master.bring_up(drives)
print(f"brought up {drives} drive(s) to Op over a raw socket")
resolution = master.sdo_upload(0, 0x608F, 1, 4, False)
print(f"0x608F:1 over the mailbox: {resolution}")
outputs = bytearray(master.logical_length)
for word in (0x06, 0x07, 0x0F):
    for index in range(drives):
        struct.pack_into("<Hi", outputs, master.output_at[index], word, 0)
    image = master.cycle(bytes(outputs))
states = [cia402.state_of(struct.unpack_from("<Hi", image, master.input_at[i])[0]) for i in range(drives)]
print("states after the walk: " + ", ".join(state.value for state in states))
for index in range(drives):
    struct.pack_into("<Hi", outputs, master.output_at[index], 0x0F, 40000 + 1000 * index)
for _ in range(6):
    image = master.cycle(bytes(outputs))
positions = [struct.unpack_from("<Hi", image, master.input_at[i])[1] for i in range(drives)]
print("positions read back: " + " ".join(str(p) for p in positions))
MASTER
cat "$OUT.master.txt"

kill "$SERVER" 2>/dev/null || true
wait "$SERVER" 2>/dev/null || true
trap - EXIT
echo ""
cat "$OUT.wire.txt"
echo ""

fail=0
say() {
  echo "::error::$1"
  fail=1
}
count() {
  grep -c -- "$1" "$2" || true
}

# ── slaveinfo ─────────────────────────────────────────────────────────────────
expected_wkc=$((DRIVES * 3))
grep -q "^$DRIVES slaves found and configured\." "$OUT.slaveinfo.txt" ||
  say "SOEM did not find and configure $DRIVES slave(s)."
grep -q "^Calculated workcounter $expected_wkc\$" "$OUT.slaveinfo.txt" ||
  say "SOEM did not compute a working counter of $expected_wkc — outputs x2 + inputs, per ETG.1000.4."
if grep -q "Not all slaves reached safe operational state" "$OUT.slaveinfo.txt"; then
  say "a slave did not reach Safe-Op under SOEM; the AL status codes above say why."
fi
[ "$(count ' State: 4$' "$OUT.slaveinfo.txt")" = "$DRIVES" ] ||
  say "not every slave reports Safe-Op (state 4) to SOEM."
[ "$(count ' Name:skynet-ethercat CiA 402 (simulated)$' "$OUT.slaveinfo.txt")" = "$DRIVES" ] ||
  say "SOEM did not read the SII device name, whole, from every slave."
[ "$(count ' Man: 00000000 ID: 00000402 Rev: 00000001$' "$OUT.slaveinfo.txt")" = "$DRIVES" ] ||
  say "SOEM did not read the SII identity words from every slave."
[ "$(count ' Output size: 48bits$' "$OUT.slaveinfo.txt")" = "$DRIVES" ] ||
  say "SOEM did not size every slave's outputs at 48 bits from the CoE mapping."
[ "$(count ' Input size: 48bits$' "$OUT.slaveinfo.txt")" = "$DRIVES" ] ||
  say "SOEM did not size every slave's inputs at 48 bits from the CoE mapping."
[ "$(count ' MBX protocols : 04$' "$OUT.slaveinfo.txt")" = "$DRIVES" ] ||
  say "SOEM did not read CoE, and only CoE, as every slave's mailbox protocol."
[ "$(count ' Has DC: 0$' "$OUT.slaveinfo.txt")" = "$DRIVES" ] ||
  say "a slave reports distributed clocks, which this stack does not implement and must not claim."
for object in '0x6040:0x00 0x10' '0x607A:0x00 0x20' '0x6041:0x00 0x10' '0x6064:0x00 0x20'; do
  [ "$(count "$object" "$OUT.slaveinfo.txt")" = "$DRIVES" ] ||
    say "SOEM's CoE PDO mapping does not list $object on every slave."
done
[ "$(count ' Ps:080d .* Ty:01 Act:01$' "$OUT.slaveinfo.txt")" = "$DRIVES" ] ||
  say "SOEM did not map every slave's mailbox status register (0x080D) through an FMMU."
[ "$(count ' Activeports:1.0.0.0$' "$OUT.slaveinfo.txt")" = "1" ] ||
  say "DL status did not show SOEM exactly one end of the line."

# ── simple_ng ─────────────────────────────────────────────────────────────────
grep -q "^Finding autoconfig slaves\.\.\. $DRIVES slaves found" "$OUT.simple.txt" ||
  say "simple_ng did not find $DRIVES slave(s)."
grep -q "all slaves are now operational" "$OUT.simple.txt" ||
  say "simple_ng could not bring every slave to Op."
# Complete lines only: the last one is usually cut off mid-line where `timeout` stopped the program.
cycles=$(count '^Iteration .* usec  WKC' "$OUT.simple.txt")
right=$(count "^Iteration .* WKC $expected_wkc  O:" "$OUT.simple.txt")
# SOEM prints `WKC -1 wrong` for a frame that did not come back within its 2 ms, which is this
# process answering late — the jitter this row says it cannot measure — and any other wrong count
# for a frame that came back with the wrong number in it, which is a defect.
lost=$(count "^Iteration .* WKC -1 wrong" "$OUT.simple.txt")
wrong=$(grep '^Iteration' "$OUT.simple.txt" | grep 'wrong (expected' | grep -vc 'WKC -1 wrong' || true)
echo "simple_ng: $cycles cycle(s) — $right with working counter $expected_wkc, $lost not back within SOEM's 2 ms, $wrong wrong"
[ "$wrong" = "0" ] ||
  say "$wrong cycle(s) came back with a working counter other than $expected_wkc."
[ "$right" -ge 100 ] ||
  say "fewer than 100 cycles came back right ($right), so the cyclic half was not shown."
# The drives' own words, decoded out of SOEM's process image: 0x0050 is `switch on disabled` with
# voltage present, the state `skynet_canopen.cia402` says a drive powers up in — read by a master
# that has never heard of that module.
image=$(grep "^Iteration .* WKC $expected_wkc  O:" "$OUT.simple.txt" | head -1 | sed 's/.* I: //; s/  T:.*//')
expected_image=$(i=0; while [ "$i" -lt "$DRIVES" ]; do printf '50 00 00 00 00 00 '; i=$((i + 1)); done)
[ "$image " = "$expected_image" ] ||
  say "SOEM's input image is [$image], not every drive's statusword 0x0050 and position 0."

# ── this repository's master, on a real socket ────────────────────────────────
grep -q "^brought up $DRIVES drive(s) to Op over a raw socket" "$OUT.master.txt" ||
  say "this repository's master could not bring the ring up over RawSocketSegment."
grep -q '^0x608F:1 over the mailbox: 65536$' "$OUT.master.txt" ||
  say "an SDO read over a raw socket did not return the drive's encoder resolution."
expected_positions=$(i=0; out=""; while [ "$i" -lt "$DRIVES" ]; do out="$out $((40000 + 1000 * i))"; i=$((i + 1)); done; echo "$out")
grep -q "^positions read back:$expected_positions\$" "$OUT.master.txt" ||
  say "the setpoints this master sent over the wire did not come back as the drives' positions."

if [ "$fail" -ne 0 ]; then
  echo ""
  echo "SOEM and this slave stack disagree. The output above is what to read."
  exit 1
fi
echo ""
echo "An independent master agrees with this slave stack: SOEM read every slave's SII, sized its"
echo "process data from the CoE mapping, mapped its mailbox status, brought the ring to Op and ran"
echo "$right cycle(s) at the working counter ETG.1000.4 predicts. This repository's own master did"
echo "the same over a raw socket — RawSocketSegment's first run — and moved every drive."
echo ""
echo "What this is not: a segment. A veth pair has no propagation delay, so distributed clocks are"
echo "neither implemented nor claimed (every slave says Has DC: 0), and the round-trip times above"
echo "are Python's rather than silicon's. FSoE is not here either. The row's not-dry-verifiable"
echo "column is unchanged by this run."
