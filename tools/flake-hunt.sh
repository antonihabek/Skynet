#!/bin/sh
# Run a command N times on a quiet machine, then N times with every core busy, and report the runs that failed.
#
#   sh tools/flake-hunt.sh <runs> <command> [args...]
#   sh tools/flake-hunt.sh 25 make -C drivers/lite3 suite
#
# Why it exists (DRY-COVERAGE-PLAN.md F210). A dry suite that is green on a quiet machine and red one run in five on a loaded one is not
# a suite that found a defect, and under `tools/seed-defects/` it is worse than a nuisance: the campaign's baseline is three green runs and
# its seeds then run on parallel workers, so a flake in the *unpatched* suite is counted as a defect noticed and inflates the detection
# rate this repository reports. About a dozen assertions in two driver suites had that fault, and none of them had failed on a
# developer's machine.
#
# A load of one busy loop per core is not a model of a CI runner. It is a way to find the checks that assert an ordering nothing
# promises — a frame inside the driver's own bound, "the last datagram" being the one that matters, a count that an earlier late push moves
# — by making the machine late in the way a loaded one is. A command that passes both rounds has shown nothing about a quiet one's luck;
# a command that fails the second has an assertion to restate.
#
# For each failing run the lines that look like a failure are printed with the eight before them (stderr and stdout interleaved as the
# command wrote them, so a driver's own message lands beside the check it explains). The exit status is 0 only when every run of both
# rounds passed. POSIX sh: no bashisms, and nothing it needs is more than `nproc` (or `getconf`) and `grep`.
set -u

if [ "$#" -lt 2 ]; then
  echo "usage: sh tools/flake-hunt.sh <runs> <command> [args...]" >&2
  exit 2
fi
runs="$1"
shift
case "$runs" in
  '' | *[!0-9]* | 0) echo "tools/flake-hunt.sh: <runs> is '$runs'; give a positive number of runs" >&2; exit 2 ;;
esac

cores="$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
burners=""
stop_burners() {
  for pid in $burners; do kill "$pid" 2>/dev/null; done
  burners=""
}
trap 'stop_burners' EXIT INT TERM

# One round: print the failing runs, then "<label>: G of N green". Returns 0 when all were.
round() {
  label="$1"
  shift
  green=0
  n=1
  while [ "$n" -le "$runs" ]; do
    out="$("$@" 2>&1)"
    rc=$?
    if [ "$rc" -eq 0 ]; then
      green=$((green + 1))
    else
      echo "-- $label, run $n of $runs exited $rc"
      printf '%s\n' "$out" | grep -B8 -E 'FAIL|FAILED|failed|error' | tail -n 40
    fi
    n=$((n + 1))
  done
  echo "$label: $green of $runs green"
  [ "$green" -eq "$runs" ]
}

status=0
round "quiet" "$@" || status=1

i=1
while [ "$i" -le "$cores" ]; do
  (while :; do :; done) &
  burners="$burners $!"
  i=$((i + 1))
done
echo "-- $cores busy loops started"
round "loaded" "$@" || status=1
stop_burners

exit "$status"
