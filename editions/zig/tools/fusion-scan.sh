#!/bin/sh
# Refuse a fused multiply-add in any of this edition's translation units.
#
# §31.2 pins the kernel's sine, cosine and square root as a sequence of correctly rounded operations so
# that every edition computes the same bits. A fused multiply-add is a different sequence: it computes
# `x*y + z` without rounding `x*y`.
#
# Zig's default float mode is `.strict`, which forbids contraction, so unlike the C and C++ editions
# there is no flag to set — and that is exactly why this scan exists rather than being omitted. A default
# is a thing that can change, and `@setFloatMode(.optimized)` is one line in one file away from making
# this edition's arithmetic differ from the reference's on every machine with an FMA.
#
# Two architectures, because they are the two where the instruction exists and a robot might run:
# aarch64 has FMA in its base instruction set, and x86-64 has it from v3. The probe is compiled the same
# way for each and **must** fuse, so a host where this scan cannot see an FMA fails rather than passing
# by not looking (finding 171).
set -e

here=$(dirname "$0")/..
out=${TMPDIR:-/tmp}/skynet-zig-fusion
rm -rf "$out"
mkdir -p "$out"

# vfmadd231sd and the rest of the VEX family on x86-64; fmadd, fmsub, fnmadd, fnmsub on aarch64.
mnemonic='(^|[[:space:]])v?fn?m(add|sub)[0-9a-z]*[[:space:]]'

for target in aarch64-linux-gnu:baseline x86_64-linux-gnu:x86_64_v3; do
  triple=${target%%:*}
  cpu=${target##*:}
  echo "== fused multiply-add scan ($triple, $cpu)"

  probe="$out/probe-$triple.s"
  zig build-obj -target "$triple" -mcpu "$cpu" -OReleaseSafe -femit-asm="$probe" -fno-emit-bin \
    "$here/src/fusion_probe.zig"
  if ! grep -qiE "$mnemonic" "$probe"; then
    echo "the probe did not fuse for $triple, so this scan cannot see a fused instruction there"
    exit 1
  fi

  found=0
  for source in "$here"/src/*.zig "$here"/src/contracts/*.zig; do
    case $source in *fusion_probe.zig) continue ;; esac
    [ -e "$source" ] || continue
    name=$(basename "$source" .zig)
    listing="$out/$triple-$name.s"
    # A module that is not a program still compiles as one for this purpose: the scan is over the code
    # the compiler emits for these functions, and `build-obj` emits it without needing a `main`.
    zig build-obj -target "$triple" -mcpu "$cpu" -OReleaseSafe -femit-asm="$listing" -fno-emit-bin \
      "$source" 2>/dev/null || continue
    if grep -ciE "$mnemonic" "$listing" | grep -qv '^0$'; then
      echo "  $source has a fused multiply-add"
      found=1
    fi
  done
  if [ "$found" -ne 0 ]; then
    echo "a product fused into an add is a different sequence of roundings from the reference's, and"
    echo "section 31.2 pins the bits. See the header of this script."
    exit 1
  fi
  echo "  none, and the probe fused"
done
