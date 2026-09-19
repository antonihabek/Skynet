#!/usr/bin/env bash
#
# §23.5's machine preconditions, measured on a real-time kernel and refused on a stock one.
#
#   packaging/rt-vm.sh fetch <dir>    # Ubuntu: unpack QEMU, busybox and two kernels into <dir>, no root
#   packaging/rt-vm.sh check <dir>    # boot both, run `skynetk run --tier host` in each, assert
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
  sed -n '3,5p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
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

do_check() {
  local qemu="${QEMU:-$dir/root/usr/bin/qemu-system-x86_64}"
  local qemu_libs="${QEMU_LIBS:-$dir/root/usr/lib/x86_64-linux-gnu}"
  local busybox="${BUSYBOX:-$dir/root/usr/bin/busybox}"
  local rt_kernel stock_kernel
  rt_kernel="${RT_KERNEL:-$(only "$dir/root/boot/vmlinuz-*-realtime" "real-time kernel" RT_KERNEL)}"
  stock_kernel="${STOCK_KERNEL:-$(only "$dir/root/boot/vmlinuz-*-generic" "generic kernel" STOCK_KERNEL)}"
  for path in "$qemu" "$busybox" "$rt_kernel" "$stock_kernel"; do
    [ -r "$path" ] || { echo "::error::$path is not readable. Run \`$0 fetch $dir\` first."; exit 1; }
  done

  echo "== building skynetk and the mock body"
  cargo build --manifest-path "$REPO/Cargo.toml" --release -q -p skynet-kernel-host
  make -s -C "$REPO/drivers/mock" >/dev/null

  echo "== assembling one initramfs for both boots"
  local work="$dir/work" initramfs="$dir/work/initramfs"
  rm -rf "$work" && mkdir -p "$initramfs"/{bin,lib,proc,sys,dev,tmp}
  cp "$busybox" "$initramfs/bin/busybox"
  cp "$REPO/target/release/skynetk" "$initramfs/bin/skynetk"
  cp "$REPO/drivers/mock/libskynet_mock.so" "$initramfs/lib/libskynet_mock.so"
  with_libraries "$REPO/target/release/skynetk" "$initramfs"
  with_libraries "$REPO/drivers/mock/libskynet_mock.so" "$initramfs"
  # The same script under both kernels. Every line it prints that the assertions read is prefixed,
  # so a kernel message interleaved on the console cannot be mistaken for one.
  cat > "$initramfs/init" <<'INIT'
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
  chmod +x "$initramfs/init"
  (cd "$initramfs" && find . | "$busybox" cpio -o -H newc 2>/dev/null | gzip -9) > "$work/initramfs.cpio.gz"

  # QEMU's firmware from wherever it was unpacked, dereferenced: a distribution's QEMU symlinks its
  # BIOS into /usr/share/seabios, which is an absolute path that does not exist beside <dir>.
  local firmware="$work/firmware"
  mkdir -p "$firmware"
  local share
  for share in "$(dirname "$qemu")/../share/qemu" "$(dirname "$qemu")/../share/seabios"; do
    [ -d "$share" ] && find "$share" -maxdepth 1 \( -type f -o -type l \) -name '*.bin' \
      -exec cp -L {} "$firmware/" \; 2>/dev/null || true
  done
  local accel=tcg
  if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then accel=kvm; fi
  echo "   accelerator: $accel$([ "$accel" = tcg ] && echo ' (no /dev/kvm access — software emulation, slower and no less honest)')"

  boot() {
    local kernel="$1" log="$2"
    echo "== booting $(basename "$kernel")"
    LD_LIBRARY_PATH="$qemu_libs${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" timeout 900 "$qemu" \
      -L "$firmware" -accel "$accel" -m 512 -smp 2 -nographic -no-reboot -nic none -vga none \
      -kernel "$kernel" -initrd "$work/initramfs.cpio.gz" \
      -append "console=ttyS0 panic=-1 rdinit=/init loglevel=3" \
      </dev/null 2>&1 | tr -d '\r' > "$log" || true
    # Echoed for the reader; the assertions below read the log itself. `|| true` because a boot
    # that printed nothing is a failure the assertions name, not a reason to stop before them.
    grep '^rtvm: ' "$log" | sed 's/^/   /' || true
  }

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

case "$command" in
  fetch) do_fetch ;;
  check) do_check ;;
  *) usage ;;
esac
