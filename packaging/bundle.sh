#!/usr/bin/env bash
#
# Build a release bundle: everything needed to install Skynet OS or bring up a virtual robot, with
# no Rust toolchain and no sources on the machine that receives it.
#
#   ./packaging/bundle.sh                 # into dist/, for this machine's architecture
#   ./packaging/bundle.sh --out /tmp/rel
#
# This is the only thing that produces a release. CI runs it on each architecture and uploads what
# it writes, so a bundle built on a workstation and a bundle on the Releases page are the same
# artefact made the same way — and anything wrong with one is reproducible with the other.
#
# # What is in one
#
#   skynet-os-<version>-linux-<arch>/
#     bin/        skynetd skynetk skynet skynetp
#     lib/        every driver library that needs no vendor SDK: the mock, the pendant, the Rust
#                 drivers (CANopen, EtherCAT, Franka, UR) and python-host
#     python/     the Python drivers' packages and skynet_rhal.py, flat — python-host's payload
#     packaging/  install.sh, virtual-robot.sh, the systemd units
#     docs/       the specification
#     models.json the manifest `skynet setup --robot` reads
#     confidence.json  what dry evidence each driver has, layer by layer, which the same command prints
#     LICENSE README.md SECURITY.md
#
# `install.sh` and `virtual-robot.sh` recognise this shape — binaries at bin/ and no Cargo.toml —
# and run from it without building anything, and `skynet setup --robot <model>` resolves a driver
# from it without a checkout (DRY-COVERAGE-PLAN.md item 12.B.2).
#
# # What is deliberately not
#
# No driver that links a vendor's SDK (`"kind": "sdk"` in the manifest): those are built where the
# SDK is, and the wizard's refusal names the variable. And nothing is chosen for you — a driver in
# lib/ is a driver you can name, not a default: a robot installed against somebody else's joint
# ratings is what naming one is for. What ships is the manifest's own list, read here and not
# repeated, so a new driver is in the next bundle without anyone remembering to add it.

set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$HERE/.." && pwd)"
OUT="$REPO/dist"

while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
    -h|--help) sed -n '2,35p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "bundle.sh: \`$1\` is not an option (--out <dir>)" >&2; exit 1 ;;
  esac
done

die() { printf '\n  error %s\n\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || die "a release bundle is a Linux bundle, and this is $(uname -s). The
  binaries link the host's C library, so they are built on the architecture they are for."
command -v cargo >/dev/null 2>&1 || die "cargo is not installed."
command -v make  >/dev/null 2>&1 || die "make is not installed."

# The version is the workspace's, read from the one place it is written. A release tag that
# disagrees with it is refused by the release workflow rather than silently renamed here.
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/Cargo.toml" | head -1)"
[ -n "$VERSION" ] || die "could not read the workspace version from Cargo.toml"

case "$(uname -m)" in
  x86_64)        ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) die "no release is built for $(uname -m). x86_64 and aarch64 are; build from source instead." ;;
esac

NAME="skynet-os-$VERSION-linux-$ARCH"
STAGE="$OUT/$NAME"

command -v python3 >/dev/null 2>&1 || die "python3 is not installed. It reads drivers/models.json, which says which drivers a bundle ships."

echo "  building $NAME" >&2
( cd "$REPO" && cargo build --release --workspace --locked ) >&2

# What a bundle ships is the manifest's, not a list kept here: one line per thing, `make <dir> <lib>`
# for a driver a Makefile builds (prebuilt, and python-host), `cargo <lib>` for one the workspace
# built above, `python <dir/package>` for a Python driver's package. An SDK-linked driver is not
# listed: it needs the SDK on the machine that builds it.
DRIVER_PLAN="$(python3 - "$REPO/drivers/models.json" <<'PY'
import json, sys
models = json.load(open(sys.argv[1], encoding="utf-8"))["models"]
seen = set()
for name, m in models.items():
    kind = m.get("kind")
    if kind in ("prebuilt", "host"):
        print("make", m["dir"], m["lib"])
    elif kind == "cargo":
        print("cargo", m["lib"])
    elif kind == "python":
        for package in m["python"]["packages"]:
            if package not in seen:
                seen.add(package)
                print("python", package)
PY
)"
while read -r what a b; do
  if [ "$what" = "make" ]; then make -C "$REPO/drivers/$a" lib >&2 < /dev/null; fi
done <<< "$DRIVER_PLAN"

rm -rf "$STAGE"
install -d -m 0755 "$STAGE/bin" "$STAGE/lib" "$STAGE/packaging/systemd" "$STAGE/docs"
for binary in skynetd skynetk skynet skynetp; do
  install -m 0755 "$REPO/target/release/$binary" "$STAGE/bin/$binary"
done
while read -r what a b; do
  case "$what" in
    make)   install -m 0755 "$REPO/drivers/$a/$b" "$STAGE/lib/$b" ;;
    cargo)  install -m 0755 "$REPO/target/release/$a" "$STAGE/lib/$a" ;;
    python)
      # Flat, by the package's own name: python-host's PYTHONPATH is one directory, and a package is
      # imported by that name. Byte caches are the build machine's interpreter's, so they stay here.
      install -d -m 0755 "$STAGE/python"
      cp -R "$REPO/drivers/$a" "$STAGE/python/$(basename "$a")"
      ;;
  esac
done <<< "$DRIVER_PLAN"
if [ -d "$STAGE/python" ]; then
  find "$STAGE/python" -name __pycache__ -prune -exec rm -rf {} +
  install -m 0644 "$REPO/bindings/python/skynet_rhal.py" "$STAGE/python/skynet_rhal.py"
  find "$STAGE/python" -type d -exec chmod 0755 {} +
  find "$STAGE/python" -type f -exec chmod 0644 {} +
fi
[ -f "$STAGE/lib/libskynet_mock.so" ] || die "the manifest lists no mock body, and virtual-robot.sh needs one."
install -m 0755 "$REPO/packaging/install.sh" "$REPO/packaging/virtual-robot.sh" "$STAGE/packaging/"
install -m 0644 "$REPO/packaging/README.md" "$STAGE/packaging/README.md"
install -m 0644 "$REPO/packaging/systemd/"*.service "$STAGE/packaging/systemd/"
install -m 0644 "$REPO/docs/skynet-os-architecture.md" "$STAGE/docs/"
install -m 0644 "$REPO/LICENSE" "$REPO/README.md" "$REPO/SECURITY.md" "$STAGE/"
# The robot manifest, so `skynet setup --robot` refuses typed (missing SDK, sim-only)
# from a bundle too — not only from a checkout that has drivers/ beside it.
install -m 0644 "$REPO/drivers/models.json" "$STAGE/models.json"
# And the evidence that goes with each entry (DRY-COVERAGE-PLAN.md item 12.C.1): the plan prints a driver's
# layer-by-layer dry-test evidence beside its release stage, and says aloud when the file is not here.
install -m 0644 "$REPO/drivers/confidence.json" "$STAGE/confidence.json"

# A stable name for the archive, without the version in it, so that
# `releases/latest/download/skynet-os-linux-x86_64.tar.gz` always resolves to the newest one and an
# install script never has to ask an API what "latest" means. The version is inside, in the
# directory name and in `skynet version`.
ARCHIVE="$OUT/skynet-os-linux-$ARCH.tar.gz"
tar -C "$OUT" -czf "$ARCHIVE" "$NAME"

# Regenerated over every archive in the directory, in the format `sha256sum -c` reads. Rewritten
# rather than appended: an appended file keeps the hash of a bundle that has since been rebuilt, and
# a checksum that no longer matches its archive is worse than none.
( cd "$OUT" && sha256sum skynet-os-linux-*.tar.gz > SHA256SUMS )

echo "$ARCHIVE"
