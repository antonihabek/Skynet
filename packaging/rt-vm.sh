#!/usr/bin/env bash
#
# §23.5's machine preconditions, measured on a real-time kernel and refused on a stock one.
#
#   packaging/rt-vm.sh fetch <dir>    # Ubuntu: unpack QEMU, busybox and two kernels into <dir>, no root
#   packaging/rt-vm.sh check <dir>    # boot both, run `skynetk run --tier host` in each, assert
#   packaging/rt-vm.sh timing <dir>   # boot both, turn the loop quiet, under stress and through a stall, report the distribution
#
# This is `DRY-COVERAGE-PLAN.md` Phase 6's harness of record and its gate's point 3: *"`skynetk
# --tier host` refuses to start on a kernel that is not PREEMPT_RT, and the refusal names what it
# read. Held to a control: the same binary on the RT kernel starts and reports `VmLck` non-zero."*
# The refusal half had been fixture-tested in both directions and run on every stock kernel in
# reach; the control half had never run, because no machine here had a real-time kernel on it.
#
# # What it shows, and what it cannot
#
# Two boots of one initramfs — the same `skynetk`, the same mock driver, the same `/init` — under
# two kernels from the same distribution: its PREEMPT_RT build and its generic one. The init runs as
# root with `RLIMIT_MEMLOCK` raised to unlimited, which is what `skynetk.service`'s
# `LimitMEMLOCK=infinity` gives the loop on a robot, and there is no swap in an initramfs. So the
# only reading that differs between the two boots is the one §23.5 is about: the kernel.
#
# * Under the real-time kernel the loop must **start**, report `rt PREEMPT_RT` read off
#   `/sys/kernel/realtime`, take the lock and report `VmLck` above zero, and turn its cycles.
# * Under the generic kernel the same binary must **refuse** before the driver opens, name
#   `/sys/kernel/realtime` as what it read, and exit non-zero.
#
# The VM has no network device at all (`-nic none`), which also makes every boot §11's offline case.
#
# It cannot show a latency. A VM is a machine rather than a model — F2's distinction, and why it
# is the right harness here — but under QEMU's software emulation a period is whatever the host
# scheduler makes of it, and even under KVM the jitter belongs to the host. No number from this
# script is bench evidence, and nothing here records one: `skynet bench record` binds a record to
# the artefacts a body was measured against, and there is no body.
#
# # Where the pieces come from
#
# `fetch` uses the distribution's own archive and nothing else: `apt-get download` into <dir> and
# `dpkg-deb -x` beside it, so nothing is installed system-wide and no root is needed. The kernels
# are whatever `linux-image-realtime` and `linux-image-generic` resolve to — Ubuntu has carried a
# PREEMPT_RT kernel in its archive since 26.04 — and QEMU is the closure of `qemu-system-x86` the
# machine does not already have. On another distribution, point the variables below at a QEMU, a
# static busybox and two kernels, and `check` does not care where they came from.
#
#   QEMU          qemu-system-x86_64                  default <dir>/root/usr/bin/qemu-system-x86_64
#   QEMU_LIBS     its library directory               default <dir>/root/usr/lib/x86_64-linux-gnu
#   BUSYBOX       a statically linked busybox          default <dir>/root/usr/bin/busybox
#   RT_KERNEL     a PREEMPT_RT vmlinuz                 default <dir>/root/boot/vmlinuz-*-realtime
#   STOCK_KERNEL  a non-RT vmlinuz                     default <dir>/root/boot/vmlinuz-*-generic
#
# GitHub's `ubuntu-latest` is 24.04, whose real-time kernel is not in the free archive, so CI does
# not run this. The plan's log records each run and the machine it ran on.

set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$HERE/.." && pwd)"

usage() {
  sed -n '3,6p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 2
}

[ $# -eq 2 ] || usage
command="$1"
dir="$(mkdir -p -- "$2" && cd -- "$2" && pwd)"

# The versioned image a metapackage depends on, so the kernel is a named file rather than whatever
# `apt-get download` of a metapackage would do (download the metapackage, which holds no kernel).
resolve_kernel() {
  apt-cache depends --no-recommends --no-suggests "$1" \
    | awk '/Depends: linux-image-[0-9]/ { print $2; exit }'
}

do_fetch() {
  for tool in apt-get apt-cache dpkg-deb; do
    command -v "$tool" >/dev/null || {
      echo "::error::\`fetch\` needs $tool — it unpacks the distribution's own packages. On another"
      echo "distribution, set QEMU, BUSYBOX, RT_KERNEL and STOCK_KERNEL and run \`check\` directly."
      exit 1
    }
  done
  local rt stock
  rt="$(resolve_kernel linux-image-realtime)"
  stock="$(resolve_kernel linux-image-generic)"
  if [ -z "$rt" ]; then
    echo "::error::this archive has no linux-image-realtime. Ubuntu carries one from 26.04; on an"
    echo "earlier release it is an Ubuntu Pro package, and \`check\` needs RT_KERNEL pointed at one."
    exit 1
  fi
  [ -n "$stock" ] || { echo "::error::linux-image-generic resolves to no kernel image"; exit 1; }
  # Only what this machine lacks: a library the system already has is on the loader's path anyway.
  local closure
  closure="$(apt-get -s install --no-install-recommends qemu-system-x86 busybox-static 2>/dev/null \
    | awk '/^Inst / { print $2 }')"
  mkdir -p "$dir/debs" "$dir/root"
  (cd "$dir/debs" && apt-get download $closure "$rt" "$stock")
  for deb in "$dir"/debs/*.deb; do
    dpkg-deb -x "$deb" "$dir/root"
  done
  echo "fetched  $rt, $stock and $(echo "$closure" | wc -w) package(s) of QEMU and busybox into $dir/root"
}

# The one file a glob names, or a refusal naming the glob. Two kernels of one flavour is a directory
# somebody fetched into twice, and picking one would be deciding which kernel the evidence is about.
only() {
  local matches=()
  for path in $1; do [ -e "$path" ] && matches+=("$path"); done
  if [ "${#matches[@]}" -ne 1 ]; then
    echo "::error::expected exactly one $2 matching $1, found ${#matches[@]}. Set $3 to the one meant." >&2
    exit 1
  fi
  echo "${matches[0]}"
}

# Copy a binary and every library it loads into the initramfs at the same absolute paths, so the
# loader inside the VM finds them exactly where it would outside.
with_libraries() {
  local binary="$1" into="$2" lib
  for lib in $(ldd "$binary" | awk '/=> \// { print $3 } /^\t\/.*ld-linux/ { print $1 }'); do
    mkdir -p "$into$(dirname "$lib")"
    cp -L "$lib" "$into$lib"
  done
}

# Everything the boots share: where QEMU, busybox and the two kernels are, skynetk and the mock body built, one initramfs
# assembled around the init script in $1, and the firmware QEMU needs. The variables it sets are the boots' too.
assemble() {
  local init_file="$1"
  qemu="${QEMU:-$dir/root/usr/bin/qemu-system-x86_64}"
  qemu_libs="${QEMU_LIBS:-$dir/root/usr/lib/x86_64-linux-gnu}"
  busybox="${BUSYBOX:-$dir/root/usr/bin/busybox}"
  rt_kernel="${RT_KERNEL:-$(only "$dir/root/boot/vmlinuz-*-realtime" "real-time kernel" RT_KERNEL)}"
  stock_kernel="${STOCK_KERNEL:-$(only "$dir/root/boot/vmlinuz-*-generic" "generic kernel" STOCK_KERNEL)}"
  for path in "$qemu" "$busybox" "$rt_kernel" "$stock_kernel"; do
    [ -r "$path" ] || { echo "::error::$path is not readable. Run \`$0 fetch $dir\` first."; exit 1; }
  done

  echo "== building skynetk and the mock body"
  cargo build --manifest-path "$REPO/Cargo.toml" --release -q -p skynet-kernel-host
  make -s -C "$REPO/drivers/mock" >/dev/null

  echo "== assembling one initramfs for both boots"
  work="$dir/work" initramfs="$dir/work/initramfs"
  rm -rf "$work" && mkdir -p "$initramfs"/{bin,lib,proc,sys,dev,tmp}
  cp "$busybox" "$initramfs/bin/busybox"
  local built="${CARGO_TARGET_DIR:-$REPO/target}/release"
  cp "$built/skynetk" "$initramfs/bin/skynetk"
  cp "$REPO/drivers/mock/libskynet_mock.so" "$initramfs/lib/libskynet_mock.so"
  with_libraries "$built/skynetk" "$initramfs"
  with_libraries "$REPO/drivers/mock/libskynet_mock.so" "$initramfs"
  # `chrt`, which busybox-static does not carry: the scheduling policy and priority `skynetk.service` gives the loop (SCHED_FIFO, 80).
  if command -v chrt >/dev/null 2>&1; then
    cp "$(command -v chrt)" "$initramfs/bin/chrt"
    with_libraries "$(command -v chrt)" "$initramfs"
  fi
  # The same script under both kernels. Every line it prints that the assertions read is prefixed,
  # so a kernel message interleaved on the console cannot be mistaken for one.
  cp "$init_file" "$initramfs/init"
  chmod +x "$initramfs/init"
  (cd "$initramfs" && find . | "$busybox" cpio -o -H newc 2>/dev/null | gzip -9) > "$work/initramfs.cpio.gz"

  # QEMU's firmware from wherever it was unpacked, dereferenced: a distribution's QEMU symlinks its
  # BIOS into /usr/share/seabios, which is an absolute path that does not exist beside <dir>.
  firmware="$work/firmware"
  mkdir -p "$firmware"
  local share
  for share in "$(dirname "$qemu")/../share/qemu" "$(dirname "$qemu")/../share/seabios"; do
    [ -d "$share" ] && find "$share" -maxdepth 1 \( -type f -o -type l \) -name '*.bin' \
      -exec cp -L {} "$firmware/" \; 2>/dev/null || true
  done
  accel=tcg
  if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then accel=kvm; fi
  echo "   accelerator: $accel$([ "$accel" = tcg ] && echo ' (no /dev/kvm access — software emulation: slower, and its timing is the emulator'"'"'s; the kvm group gives KVM)')"

}

boot() {
  local kernel="$1" log="$2"
  echo "== booting $(basename "$kernel")"
  # QEMU loads its accelerators as modules from a directory compiled in as /usr/lib/.../qemu, which is not where `fetch` unpacked them.
  QEMU_MODULE_DIR="$qemu_libs/qemu" LD_LIBRARY_PATH="$qemu_libs${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" timeout 900 "$qemu" \
    -L "$firmware" -accel "$accel" -m "${RTVM_MEMORY:-512}" -smp "${RTVM_SMP:-2}" -nographic -no-reboot -nic none -vga none \
    -kernel "$kernel" -initrd "$work/initramfs.cpio.gz" \
    -append "console=ttyS0 panic=-1 rdinit=/init loglevel=3" \
    </dev/null 2>&1 | tr -d '\r' > "$log" || true
  # Echoed for the reader; the assertions below read the log itself. `|| true` because a boot
  # that printed nothing is a failure the assertions name, not a reason to stop before them.
  grep '^rtvm: ' "$log" | sed 's/^/   /' || true
}


do_check() {
  local script="$dir/init-check"
  cat > "$script" <<'INIT'
#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev
# A line of its own first: the firmware leaves a clear-screen sequence on the console with no
# newline after it, and the first line printed here would otherwise start in the middle of it.
echo
echo "rtvm: kernel $(uname -r)"
echo "rtvm: realtime $(cat /sys/kernel/realtime 2>/dev/null || echo absent)"
echo "rtvm: swap $(awk '/^SwapTotal:/ { print $2 }' /proc/meminfo) kB"
# What `LimitMEMLOCK=infinity` gives the loop on a robot. As root this raises the hard limit too.
ulimit -l unlimited
echo "rtvm: memlock $(ulimit -l)"
/bin/skynetk run --driver /lib/libskynet_mock.so --tier host --cycles 50 --hz 100 \
  > /tmp/skynetk.out 2>&1
status=$?
sed 's/^/rtvm: skynetk | /' /tmp/skynetk.out
echo "rtvm: exit $status"
poweroff -f
INIT
  assemble "$script"

  local failures=0
  expect() {
    if grep -qE "$2" "$1"; then
      echo "ok    $3"
    else
      echo "FAIL  $3 — no line matching /$2/ in $1"
      failures=$((failures + 1))
    fi
  }

  local rt_log="$work/rt.log" stock_log="$work/stock.log"
  boot "$rt_kernel" "$rt_log"
  expect "$rt_log" '^rtvm: realtime 1$' "the real-time kernel says so in /sys/kernel/realtime"
  expect "$rt_log" '^rtvm: skynetk \| rt +PREEMPT_RT \(from /sys/kernel/realtime\)$' \
    "skynetk read it and reports PREEMPT_RT"
  expect "$rt_log" '^rtvm: skynetk \| mlock +locked, VmLck [1-9][0-9]* kB$' \
    "the lock was taken and VmLck is above zero"
  expect "$rt_log" '^rtvm: skynetk \| swap +none active' "no swap, read off /proc/meminfo"
  expect "$rt_log" '^rtvm: exit 0$' "--tier host started and ran to its cycle count"

  boot "$stock_kernel" "$stock_log"
  expect "$stock_log" '^rtvm: realtime (absent|0)$' "the generic kernel is not PREEMPT_RT"
  expect "$stock_log" 'cannot back `--tier host`: .*the kernel (is not|cannot be shown to be) PREEMPT_RT \(/sys/kernel/realtime' \
    "the same binary refuses, naming /sys/kernel/realtime as what it read"
  expect "$stock_log" '^rtvm: exit [1-9][0-9]*$' "and exits non-zero before the driver opens"

  if [ "$failures" -ne 0 ]; then
    echo "$failures assertion(s) failed. The consoles are $rt_log and $stock_log."
    exit 1
  fi
  echo "both kernels behaved as §23.5 requires. Not bench evidence: no body, and no latency."
}

# What the loop's timing looked like on each kernel: quiet, under stress, and through a stall. DRY-COVERAGE-PLAN.md 12.C.5, layer 7's OS half.
#
# One init script under both kernels, as `check` has: the loop under `chrt -f 80`, which is what `skynetk.service`'s
# `CPUSchedulingPolicy=fifo` and `CPUSchedulingPriority=80` give it on a robot, with the mock body, at RTVM_HZ for RTVM_SECONDS seconds.
#
#   quiet    nothing else runs.
#   stress   two CPU hogs a CPU at the default priority, a memory-bandwidth loop, a `/proc` walk and a tmpfs write-and-sync loop: what
#            a busy machine is, without a stress tool the initramfs does not carry.
#   stall    the loop is stopped with SIGSTOP for RTVM_STALL_PERIODS periods and continued: the scheduler taking the CPU away, which
#            nothing inside the process is told. The report must say a cycle started late, must not have run the missed cycles back to
#            back, and must not call the work slow (it was not).
#
# What it reports is `skynetk`'s own: the distribution of a cycle's work and of the gap between cycles' starts (p50, p99, p99.9, p99.99,
# worst), the rate the loop really turned at, and the late starts. The budgets below are in periods, and are the only thing asserted:
# the loop's rate within 2% of the asked, p99.99 of the interval within RTVM_BUDGET_PERIODS periods, and — on the real-time kernel only —
# no late start outside the injected stall. The generic kernel is measured the same way and **reported, not asserted**: the point of
# comparing is to see what PREEMPT_RT bought on this machine, and a stock kernel that happened to be quiet proves nothing either way.
#
# **This is a host-side budget and never a latency of any robot.** The VM's clock is the host's, its vCPUs are threads of the host's
# scheduler, and a number from here is the host's jitter as much as the guest's. No body is measured: nothing in the VM is a robot, and
# the only driver in it is the mock, so what is budgeted is the loop's own cost and its scheduling, not a driver's. A driver that needs a
# substrate process is not run here. `skynet bench record` is what binds a measurement to a body, and there is none.
do_timing() {
  local hz="${RTVM_HZ:-1000}" seconds="${RTVM_SECONDS:-10}" stall_periods="${RTVM_STALL_PERIODS:-30}" budget="${RTVM_BUDGET_PERIODS:-2}"
  local cycles=$((hz * seconds))
  local script="$dir/init-timing"
  cat > "$script" <<'INIT'
#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev
echo
echo "rtvm: kernel $(uname -r)"
echo "rtvm: realtime $(cat /sys/kernel/realtime 2>/dev/null || echo absent)"
echo "rtvm: cpus $(grep -c '^processor' /proc/cpuinfo)"
ulimit -l unlimited
# The generic kernel refuses `--tier host`; the harness's own override runs it anyway, so the two kernels can be compared.
# Said aloud in the report (`tier-check UNCHECKED`), and never a robot's setting.
export SKYNETK_UNCHECKED_TIER=1
HZ=@HZ@
CYCLES=@CYCLES@
STALL_US=@STALL_US@
loop() {
  # `chrt -f 80`: the unit's policy and priority. The loop's output, prefixed, line by line.
  chrt -f 80 /bin/skynetk run --driver /lib/libskynet_mock.so --tier host --hz "$HZ" --cycles "$1" --quiet > "/tmp/$2.out" 2>&1
  status=$?
  sed "s/^/rtvm: $2 | /" "/tmp/$2.out"
  echo "rtvm: $2 exit $status"
}
stress_start() {
  : > /tmp/stress.pids
  n=$(grep -c '^processor' /proc/cpuinfo)
  i=0
  while [ "$i" -lt $((n * 2)) ]; do
    ( while :; do :; done ) &
    echo $! >> /tmp/stress.pids
    i=$((i + 1))
  done
  ( while :; do dd if=/dev/zero of=/dev/null bs=8M count=64 2>/dev/null; done ) &
  echo $! >> /tmp/stress.pids
  ( while :; do for f in /proc/[0-9]*/stat; do cat "$f" > /dev/null 2>&1; done; done ) &
  echo $! >> /tmp/stress.pids
  ( while :; do dd if=/dev/zero of=/tmp/stress bs=1M count=32 2>/dev/null; sync; rm -f /tmp/stress; done ) &
  echo $! >> /tmp/stress.pids
}
stress_stop() {
  for pid in $(cat /tmp/stress.pids); do kill -9 "$pid" 2>/dev/null; done
  killall dd 2>/dev/null
  sleep 1
}
loop "$CYCLES" quiet
stress_start
loop "$CYCLES" stress
stress_stop
# The stall: stopped a second in, for STALL_US microseconds, then continued. `usleep` and `kill` are busybox's.
chrt -f 80 /bin/skynetk run --driver /lib/libskynet_mock.so --tier host --hz "$HZ" --cycles $((HZ * 4)) --quiet > /tmp/stall.out 2>&1 &
pid=$!
usleep 1000000
kill -STOP "$pid"
usleep "$STALL_US"
kill -CONT "$pid"
wait "$pid"
status=$?
sed "s/^/rtvm: stall | /" /tmp/stall.out
echo "rtvm: stall exit $status"
poweroff -f
INIT
  sed -i "s/@HZ@/$hz/; s/@CYCLES@/$cycles/; s/@STALL_US@/$((stall_periods * 1000000 / hz))/" "$script"
  assemble "$script"

  local rt_log="$work/rt-timing.log" stock_log="$work/stock-timing.log" failures=0
  boot "$rt_kernel" "$rt_log"
  boot "$stock_kernel" "$stock_log"

  local period_ms
  period_ms="$(awk -v hz="$hz" 'BEGIN { printf "%.3f", 1000 / hz }')"
  # A number out of a report line: `number LOG PHASE LABEL FIELD` — the word after FIELD on the line PHASE | LABEL.
  number() {
    awk -v phase="rtvm: $2 | $3" -v field="$4" 'index($0, phase) == 1 { n = split($0, w, " "); for (i = 1; i < n; i++) if (w[i] == field) { print w[i + 1]; exit } }' "$1"
  }
  # `p99.99 0.120 ms` on the `cycle interval` line, and the loop rate, for one phase of one log.
  show() {
    local log="$1" phase="$2" kernel="$3"
    printf '%-9s %-7s rate %8s Hz  interval p50 %7s  p99.99 %7s  worst %8s ms  work p99.99 %7s ms  late starts %s\n' \
      "$kernel" "$phase" "$(number "$log" "$phase" 'loop rate' 'rate')" \
      "$(number "$log" "$phase" 'cycle interval' 'p50')" "$(number "$log" "$phase" 'cycle interval' 'p99.99')" \
      "$(number "$log" "$phase" 'cycle interval' 'worst')" "$(number "$log" "$phase" 'cycle work' 'p99.99')" \
      "$(number "$log" "$phase" 'late starts' 'starts')"
  }
  echo
  echo "== what the loop's timing was (host-side budget, not a robot's latency), asked for ${hz} Hz, a period of ${period_ms} ms"
  for phase in quiet stress stall; do show "$rt_log" "$phase" "realtime"; done
  for phase in quiet stress stall; do show "$stock_log" "$phase" "generic"; done
  echo

  local ok
  check_that() {
    if [ "$1" = 1 ]; then echo "ok    $2"; else echo "FAIL  $2"; failures=$((failures + 1)); fi
  }
  for phase in quiet stress; do
    rate="$(number "$rt_log" "$phase" 'loop rate' 'rate')"
    ok="$(awk -v r="${rate:-0}" -v hz="$hz" 'BEGIN { print (r >= hz * 0.98 && r <= hz * 1.02) ? 1 : 0 }')"
    check_that "$ok" "real-time kernel, $phase: the loop turned at ${rate:-?} Hz, within 2% of the $hz Hz asked"
    p9999="$(number "$rt_log" "$phase" 'cycle interval' 'p99.99')"
    ok="$(awk -v p="${p9999:-999}" -v period="$period_ms" -v budget="$budget" 'BEGIN { print (p <= period * budget) ? 1 : 0 }')"
    check_that "$ok" "real-time kernel, $phase: p99.99 of the cycle interval is ${p9999:-?} ms, within $budget periods ($(awk -v p="$period_ms" -v b="$budget" 'BEGIN { print p * b }') ms)"
    late="$(number "$rt_log" "$phase" 'late starts' 'starts')"
    check_that "$([ "${late:-1}" = 0 ] && echo 1 || echo 0)" "real-time kernel, $phase: no cycle began more than two periods late (${late:-?})"
  done
  late="$(number "$rt_log" stall 'late starts' 'starts')"
  check_that "$([ "${late:-0}" -ge 1 ] 2>/dev/null && echo 1 || echo 0)" "real-time kernel, stall: the $stall_periods-period stop is reported as a late start (${late:-?})"
  worst="$(number "$rt_log" stall 'cycle interval' 'worst')"
  ok="$(awk -v w="${worst:-0}" -v period="$period_ms" -v n="$stall_periods" 'BEGIN { print (w >= period * n * 0.8) ? 1 : 0 }')"
  check_that "$ok" "real-time kernel, stall: the worst interval, ${worst:-?} ms, is the stall"
  over="$(number "$rt_log" stall 'overruns' 'overruns')"
  check_that "$([ "${over:-1}" = 0 ] && echo 1 || echo 0)" "real-time kernel, stall: and the work was never called slow (overruns ${over:-?}) — it was the sleep that was long"
  p99="$(number "$rt_log" stall 'cycle interval' 'p99')"
  ok="$(awk -v p="${p99:-999}" -v period="$period_ms" 'BEGIN { print (p <= period * 2) ? 1 : 0 }')"
  check_that "$ok" "real-time kernel, stall: and the missed cycles were not run back to back — an ordinary interval is still a period (p99 ${p99:-?} ms)"
  for phase in quiet stress stall; do
    grep -q "^rtvm: $phase exit 0$" "$rt_log" && status=1 || status=0
    check_that "$status" "real-time kernel, $phase: the loop ran to its cycle count and exited 0"
  done

  if [ "$failures" -ne 0 ]; then
    echo "$failures assertion(s) failed. The consoles are $rt_log and $stock_log."
    exit 1
  fi
  echo "the loop kept its rate and its period on the real-time kernel, and said so when it was descheduled. Host-side budget; not a latency of any robot."
}

case "$command" in
  fetch) do_fetch ;;
  check) do_check ;;
  timing) do_timing ;;
  *) usage ;;
esac
