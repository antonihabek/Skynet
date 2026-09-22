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
#     lib/        libskynet_mock.so — the body with no hardware behind it
#     packaging/  install.sh, virtual-robot.sh, the systemd units
#     docs/       the specification
#     LICENSE README.md SECURITY.md
#
# `install.sh` and `virtual-robot.sh` recognise this shape — binaries at bin/ and no Cargo.toml —
# and run from it without building anything.
#
# # What is deliberately not
#
# No driver but the mock. A real robot's driver comes from whoever wrote it for that robot, and a
# bundle that shipped a guess would be a robot installed against somebody else's joint ratings.

set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$HERE/.." && pwd)"
OUT="$REPO/dist"

while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
    -h|--help) sed -n '2,27p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
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

echo "  building $NAME" >&2
( cd "$REPO" && cargo build --release --workspace --locked ) >&2
make -C "$REPO/drivers/mock" lib >&2

rm -rf "$STAGE"
install -d -m 0755 "$STAGE/bin" "$STAGE/lib" "$STAGE/packaging/systemd" "$STAGE/docs"
for binary in skynetd skynetk skynet skynetp; do
  install -m 0755 "$REPO/target/release/$binary" "$STAGE/bin/$binary"
done
install -m 0755 "$REPO/drivers/mock/libskynet_mock.so" "$STAGE/lib/libskynet_mock.so"
install -m 0755 "$REPO/packaging/install.sh" "$REPO/packaging/virtual-robot.sh" "$STAGE/packaging/"
install -m 0644 "$REPO/packaging/README.md" "$STAGE/packaging/README.md"
install -m 0644 "$REPO/packaging/systemd/"*.service "$STAGE/packaging/systemd/"
install -m 0644 "$REPO/docs/skynet-os-architecture.md" "$STAGE/docs/"
install -m 0644 "$REPO/LICENSE" "$REPO/README.md" "$REPO/SECURITY.md" "$STAGE/"
# The robot manifest, so `skynet setup --robot` refuses typed (missing SDK, sim-only)
# from a bundle too — not only from a checkout that has drivers/ beside it.
install -m 0644 "$REPO/drivers/models.json" "$STAGE/models.json"

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
