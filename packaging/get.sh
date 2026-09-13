#!/bin/sh
#
# Download the latest Skynet OS release for this machine, verify it, and unpack it.
#
#   curl -fsSL https://raw.githubusercontent.com/antonihabek/Skynet/main/packaging/get.sh | sh
#
# It does not use sudo, install anything system-wide, or start a robot. It puts a verified bundle in
# a directory you own and prints the one command that does those things, so the decision to run it
# stays yours.
#
#   SKYNET_DIR       where to unpack. Default ~/.local/share/skynet-os
#   SKYNET_VERSION   a release tag such as v0.1.0. Default: the latest release.
#   SKYNET_BASE_URL  where releases are downloaded from. For mirrors and for testing this script.

set -eu

REPO_URL="https://github.com/antonihabek/Skynet"
DIR="${SKYNET_DIR:-$HOME/.local/share/skynet-os}"

say()  { printf '  %s\n' "$*" >&2; }
fail() { printf '\n  error %s\n\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Linux) ;;
  *) fail "Skynet OS runs on Linux, and this is $(uname -s).
  On Windows, run this inside WSL2. On macOS, inside a Linux VM.
  Building from source works anywhere Rust does: $REPO_URL#readme" ;;
esac

case "$(uname -m)" in
  x86_64)        ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) fail "there is no prebuilt release for $(uname -m). Build from source: $REPO_URL#readme" ;;
esac

if [ -n "${SKYNET_BASE_URL:-}" ]; then
  BASE="$SKYNET_BASE_URL"
elif [ -n "${SKYNET_VERSION:-}" ]; then
  BASE="$REPO_URL/releases/download/$SKYNET_VERSION"
else
  BASE="$REPO_URL/releases/latest/download"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
else
  fail "neither curl nor wget is installed."
fi

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  fail "no sha256sum or shasum to verify the download with, and an unverified download is not
  something this script will unpack."
fi

ARCHIVE="skynet-os-linux-$ARCH.tar.gz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

say "downloading $ARCHIVE"
fetch "$BASE/$ARCHIVE" "$TMP/$ARCHIVE" || fail "could not download $BASE/$ARCHIVE"
fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS" || fail "could not download $BASE/SHA256SUMS"

expected="$(grep " $ARCHIVE\$" "$TMP/SHA256SUMS" | cut -d' ' -f1 || true)"
[ -n "$expected" ] || fail "SHA256SUMS has no entry for $ARCHIVE, so it cannot be verified."
actual="$(sha256 "$TMP/$ARCHIVE")"
[ "$expected" = "$actual" ] || fail "checksum mismatch for $ARCHIVE — the download is damaged or is
  not the file that was released. Nothing was unpacked.
    expected $expected
    got      $actual"
say "verified sha256 $actual"

mkdir -p "$DIR"
tar -xzf "$TMP/$ARCHIVE" -C "$DIR"
BUNDLE="$(tar -tzf "$TMP/$ARCHIVE" | head -1 | cut -d/ -f1)"
[ -x "$DIR/$BUNDLE/bin/skynet" ] || fail "the bundle unpacked without bin/skynet at $DIR/$BUNDLE"

# `current` names the newest install, so a PATH entry and the command in the README keep working
# across upgrades. The versioned directory it points at stays, and so does the one before it.
ln -sfn "$BUNDLE" "$DIR/current"
ROOT="$DIR/current"

say "unpacked to $DIR/$BUNDLE"
say ""
say "Bring up a virtual robot — the real OS, the real Safety Kernel, a body with no hardware:"
say ""
say "    sudo $ROOT/packaging/virtual-robot.sh up"
say ""
say "Put the CLI on your PATH:"
say ""
say "    export PATH=\"$ROOT/bin:\$PATH\""
say ""
