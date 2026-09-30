#!/usr/bin/env bash
# `skynet setup` against a fake robot: stub ssh, real tar, real binary.
#
#   bash packaging/test-setup-wizard.sh
#   SKYNET_BIN=target/release/skynet bash packaging/test-setup-wizard.sh
#
# The wizard's riskiest seams are process and pipe wiring — the tar pipe, the streamed
# relay, the prompts — which unit tests cannot touch. So this runs the real binary with a
# fake `ssh` on PATH: the probe, mkdir and cleanup legs answer canned, the receive leg
# unpacks the piped stage with real tar into a scratch directory and counts the bytes back
# exactly as the remote command would, and the install/check legs print canned installer
# transcripts in the shape install.sh actually emits (the token markers are asserted
# against install.sh itself, so a rewording fails here rather than silently breaking the
# token extraction). What is real: the binary, both ends of the tar pipe, the byte counts,
# the relay, the prompts, the log.
#
# Needs `cargo build -p skynet-cli` first, and builds it when the binary is missing.
# Unix-only: it prepends a stub directory to PATH, and the remote it fakes is Linux.

set -u

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$HERE/.." && pwd)"
BIN="${SKYNET_BIN:-$REPO/target/debug/skynet}"
FAILED=0
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED=$((FAILED + 1)); }

# Always through cargo (a no-op when current): a test that runs a stale binary tests
# yesterday's wizard. An explicit SKYNET_BIN is the operator's responsibility instead.
if [ -z "${SKYNET_BIN:-}" ]; then
  ( cd "$REPO" && cargo build -p skynet-cli ) || { fail "cargo build -p skynet-cli failed"; exit 1; }
fi
[ -x "$BIN" ] || { fail "no skynet binary at $BIN"; exit 1; }

# ── the fake world ───────────────────────────────────────────────────────────

# Bytes of ELF with the x86-64 machine field, from printf alone: the wizard reads the
# header, so the fakes need a header and nothing else. Four magic, twelve more identity,
# two type, then the machine (62, little-endian) at offset 18, then padding.
fake_elf() {
  printf '\177ELF\0\0\0\0\0\0\0\0\0\0\0\0\0\0>\0PADDING-PADDING' > "$1"
}
BUNDLE="$T/bundle"
mkdir -p "$BUNDLE/bin" "$BUNDLE/lib" "$BUNDLE/packaging/systemd"
printf '#!/usr/bin/env bash\necho stub\n' > "$BUNDLE/packaging/install.sh"
printf '[Service]\n' > "$BUNDLE/packaging/systemd/skynetd.service"
printf '[Service]\n' > "$BUNDLE/packaging/systemd/skynetk.service"
for b in skynetd skynetk skynet skynetp; do fake_elf "$BUNDLE/bin/$b"; done
fake_elf "$BUNDLE/lib/libskynet_mock.so"
printf '{"joints": []}' > "$BUNDLE/body.json"

# The canned transcripts, in the shape install.sh emits. The numbering is the wizard's
# progress contract (see test-install-output.sh); the token markers are quoted from the
# installer's own lines, and asserted against it below.
cat > "$T/install.txt" <<'EOF'
  logging this run to /tmp/skynet-setup/install.log

  Skynet OS → /usr, /etc/skynet, /lib/systemd/system, root /

  ✓ [1] (+120ms) binaries: skynetd skynetk skynet skynetp
  ✓ [2] (+8ms) driver: /usr/lib/skynet/libskynet_mock.so
  ✓ [3] (+30ms) layout: /data (packages, blackbox, state, identity) and /run
  ✓ [4] (+5ms) body: /data/state/embodiment.json
  ✓ [5] (+200ms) admin credential issued
  ✓ [6] (+12ms) environment: /etc/skynet/skynetd.env, /etc/skynetk.env (0600)

  The admin token, printed once and stored nowhere but the env file:

      stub-token-for-tests

  Nothing keeps it but a salted hash (§39.2). Write it down now.

  ✓ [7] (+400ms) every postcondition holds
EOF
cat > "$T/check.txt" <<'EOF'
  logging this run to /tmp/skynet-setup/check.log

  Skynet OS — checking the installation at /usr, /etc/skynet, root /

  ✓ [1] (+2ms) /usr/bin/skynetd is installed and executable
  ✓ [2] (+1ms) /usr/bin/skynetk is installed and executable
  ✓ [3] (+1ms) /etc/skynet/skynetd.env exists

  ✓ [4] (+0ms) every postcondition holds
EOF

mkdir -p "$T/stubbin"
cat > "$T/stubbin/ssh" <<'EOF'
#!/bin/sh
# Fake ssh for the wizard test. The remote command is always the last argument; answer by
# content. Anything unrecognised exits 99, so a wizard that starts saying new things fails
# loudly instead of chatting with a stub that understood the old ones.
FAKE_ROOT="${FAKE_ROOT:?FAKE_ROOT is not set}"
remote=""
for arg in "$@"; do remote="$arg"; done
case "$remote" in
  *SKYNET-ARCH*)
    printf 'motd noise above the markers\nSKYNET-ARCH\n%s\n' "${FAKE_ARCH:-x86_64}"
    ;;
  *SKYNET-BEGIN*)
    printf 'motd noise above the markers\nSKYNET-BEGIN\nLinux\n%s\nOK /usr/bin\nOK /etc\nOK /data\nOK /run\nOK /etc/systemd/system\nABSENT /usr/lib/systemd/system\nOK /lib/systemd/system\nSKYNET-END\n' "${FAKE_ARCH:-x86_64}"
    ;;
  *SKYNET-READY*) echo SKYNET-READY ;;
  *SKYNET-OK*)
    rm -rf "$FAKE_ROOT/remote"; mkdir -p "$FAKE_ROOT/remote"
    tar -xz -C "$FAKE_ROOT/remote" || { echo "unpack failed" >&2; exit 1; }
    echo SKYNET-OK
    find "$FAKE_ROOT/remote" -type f -exec cat {} + | wc -c
    ;;
  *install.sh\ install*) cat "$FAKE_ROOT/install.txt" ;;
  *install.sh\ --check*) cat "$FAKE_ROOT/check.txt" ;;
  *systemctl\ start*) exit 0 ;;
  *skynet\ doctor*) printf '  What the robot says about itself\n  ✓ the local socket answers\n' ;;
  'rm -rf /tmp/skynet-setup') exit 0 ;;
  *) echo "stub ssh: unexpected remote command: $remote" >&2; exit 99 ;;
esac
EOF
chmod +x "$T/stubbin/ssh"

# The token markers are install.sh's wording, quoted by the wizard. If the installer
# rewords them, the extraction silently stops — unless this fails first.
if grep -q 'printed once' "$REPO/packaging/install.sh" \
   && grep -q 'Write it down' "$REPO/packaging/install.sh"; then
  pass "the token markers still read as the wizard quotes them"
else
  fail "install.sh reworded the token markers; setup_cmd::extract_token follows them"
fi

# ── leg 1: dry run ───────────────────────────────────────────────────────────

out="$("$BIN" setup robot.test --bundle "$BUNDLE" --driver "$BUNDLE/lib/libskynet_mock.so" \
  --descriptor "$BUNDLE/body.json" --tier host --dry-run 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"9 files"* ]] && [[ "$out" == *"SKYNET-BEGIN"* ]] \
   && [[ "$out" == *"install.sh install"* ]] && [[ "$out" == *"--check"* ]] \
   && [[ "$out" == *"nothing was copied"* ]]; then
  pass "dry run prints the plan and the exact ssh calls"
else
  fail "dry run (exit $code): $out"
fi
# From a clean directory: asserting on the developer's cwd would fail on a stale log
# from any earlier run, including a manual one.
mkdir -p "$T/empty3"
( cd "$T/empty3" && "$BIN" setup robot.test --bundle "$BUNDLE" \
  --driver "$BUNDLE/lib/libskynet_mock.so" --descriptor "$BUNDLE/body.json" \
  --dry-run >/dev/null 2>&1 )
dry=$?
if [ "$dry" -ne 0 ]; then
  fail "dry run in clean dir failed (exit $dry)"
elif ls "$T/empty3"/skynet-setup-*.log >/dev/null 2>&1; then
  fail "dry run wrote a log file"
else
  pass "dry run writes no log"
fi

# A tarball works as --bundle: the release path, packed here with real tar.
( cd "$T" && tar -czf bundle.tar.gz bundle )
out="$("$BIN" setup robot.test --bundle "$T/bundle.tar.gz" --driver "$BUNDLE/lib/libskynet_mock.so" \
  --descriptor "$BUNDLE/body.json" --dry-run 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"9 files"* ]]; then
  pass "a release tarball unpacks as --bundle"
else
  fail "tarball bundle (exit $code): $out"
fi

# ── leg 2: refused connection, real ssh ──────────────────────────────────────

out="$("$BIN" setup localhost --port 1 --bundle "$BUNDLE" --driver "$BUNDLE/lib/libskynet_mock.so" \
  --descriptor "$BUNDLE/body.json" --yes --log-file "$T/refused.log" 2>&1)" && code=0 || code=$?
if [ "$code" -ne 0 ] && [[ "$out" == *"Connection refused"* ]] && [[ "$out" == *"sshd"* ]]; then
  pass "a refused connection names the cause and the remedy"
else
  fail "refused connection (exit $code): $out"
fi
if grep -q 'error: cannot reach the robot' "$T/refused.log" 2>/dev/null; then
  pass "the failure is recorded in the log"
else
  fail "the refused run left no verdict in the log"
fi

# ── leg 3: the whole run, stub ssh ───────────────────────────────────────────

export FAKE_ROOT="$T" PATH="$T/stubbin:$PATH"
out="$(printf 'y\n' | "$BIN" setup robot.test --user root --bundle "$BUNDLE" \
  --driver "$BUNDLE/lib/libskynet_mock.so" --descriptor "$BUNDLE/body.json" \
  --log-file "$T/happy.log" 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"installed:"* ]] && [[ "$out" == *"stub-token-for-tests"* ]]; then
  pass "the guided run installs end to end"
else
  fail "happy path (exit $code): $out"
fi
if [[ "$out" == *"(7 steps,"* ]] && [[ "$out" == *"(4 checks held"* ]]; then
  pass "the summary counts the streamed steps and checks"
else
  fail "step counts missing: $out"
fi
if grep -q '✓ \[7\]' "$T/happy.log" && grep -q 'stub-token-for-tests' "$T/happy.log" \
   && grep -q 'installed:' "$T/happy.log" && ! grep -q $'\x1b' "$T/happy.log"; then
  pass "the log holds the relayed steps, the token and no color codes"
else
  fail "happy-path log is incomplete"
fi
# The receive leg unpacked with real tar: install.sh, two units, four binaries, driver, body.
if [ "$(find "$T/remote" -type f | wc -l)" -eq 9 ] \
   && [ -f "$T/remote/driver/libskynet_mock.so" ] \
   && [ -f "$T/remote/descriptor.json" ]; then
  pass "the stage arrived whole through the tar pipe"
else
  fail "the remote stage is short: $(find "$T/remote" -type f 2>/dev/null)"
fi

# ── leg 4: prompts ─────────────────────────────────────────────────────────────

# Hermetic: the binary is copied where no bundle detection can reach, and the working
# directory is empty — so host, bundle and driver are all prompted for (the user
# defaults to root silently; the body is derived, leg 7). The trailing answers decline
# the tail offers.
mkdir -p "$T/empty" "$T/fakebin"
cp "$BIN" "$T/fakebin/skynet"
out="$(cd "$T/empty" && printf 'prompted.test\n%s\n\ny\nn\nn\n' "$BUNDLE" \
  | PATH="$T/stubbin:$PATH" FAKE_ROOT="$T" "$T/fakebin/skynet" setup \
    --descriptor "$BUNDLE/body.json" --log-file "$T/prompted.log" 2>&1)" \
  && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"robot host"* ]] \
   && [[ "$out" == *"bundle (tarball"* ]] && [[ "$out" == *"driver (.so)"* ]]; then
  pass "an empty command line prompts for what it needs"
else
  fail "prompt flow (exit $code): $out"
fi
if [[ "$out" == *"defaulted to the bundle's mock"* ]]; then
  pass "accepting the mock default says so loudly"
else
  fail "mock default not announced: $out"
fi

# ── leg 5: declined at the confirmation ──────────────────────────────────────

out="$(printf 'n\n' | "$BIN" setup robot.test --bundle "$BUNDLE" \
  --driver "$BUNDLE/lib/libskynet_mock.so" --descriptor "$BUNDLE/body.json" \
  --log-file "$T/aborted.log" 2>&1)" && code=0 || code=$?
if [ "$code" -ne 0 ] && [[ "$out" == *"aborted"* ]]; then
  pass "declining aborts before anything is copied"
else
  fail "abort flow (exit $code): $out"
fi

# ── leg 6: the release is fetched when none is here ───────────────────────────

# A local releases directory standing in for the release page: SKYNET_BASE_URL points at
# it, so the download and the checksum verification run for real over file:// URLs with
# no network involved.
mkdir -p "$T/releases"
( cd "$T" && tar -czf releases/skynet-os-linux-x86_64.tar.gz bundle )
( cd "$T/releases" && sha256sum skynet-os-linux-x86_64.tar.gz > SHA256SUMS )
mkdir -p "$T/empty2"
out="$(cd "$T/empty2" && printf 'y\n' \
  | PATH="$T/stubbin:$PATH" FAKE_ROOT="$T" SKYNET_BASE_URL="file://$T/releases" \
    "$T/fakebin/skynet" setup robot.test --user root --driver "$BUNDLE/lib/libskynet_mock.so" \
    --descriptor "$BUNDLE/body.json" --log-file "$T/fetched.log" 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"fetched and verified skynet-os-linux-x86_64.tar.gz"* ]] \
   && [[ "$out" == *"installed:"* ]]; then
  pass "a missing bundle is fetched for the robot's architecture"
else
  fail "fetch flow (exit $code): $out"
fi

# A damaged download is refused, not installed: the checksum names both hashes.
mkdir -p "$T/releases-bad"
cp "$T/releases/skynet-os-linux-x86_64.tar.gz" "$T/releases/SHA256SUMS" "$T/releases-bad/"
printf 'tamper' >> "$T/releases-bad/skynet-os-linux-x86_64.tar.gz"
out="$(cd "$T/empty2" && SKYNET_BASE_URL="file://$T/releases-bad" "$T/fakebin/skynet" setup \
  robot.test --driver "$BUNDLE/lib/libskynet_mock.so" --descriptor "$BUNDLE/body.json" \
  --yes --log-file "$T/tampered.log" 2>&1)" && code=0 || code=$?
if [ "$code" -ne 0 ] && [[ "$out" == *"checksum mismatch"* ]] && [[ "$out" == *"Pass --bundle"* ]]; then
  pass "a tampered download is refused with both hashes"
else
  fail "tamper flow (exit $code): $out"
fi

# ── leg 7: the body is derived from the driver ─────────────────────────────────

# Derivation loads the real thing, so this leg builds the mock driver and omits
# --descriptor entirely: host and driver are the whole of the input.
[ -f "$REPO/drivers/mock/libskynet_mock.so" ] || ( cd "$REPO" && make -C drivers/mock lib )
MOCK="$REPO/drivers/mock/libskynet_mock.so"
out="$(printf 'y\n' | "$BIN" setup robot.test --bundle "$BUNDLE" --user root --driver "$MOCK" \
  --log-file "$T/derived.log" 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"derived from the driver"* ]] \
   && [[ "$out" == *"installed:"* ]]; then
  pass "host and driver are enough; the body is derived"
else
  fail "derive flow (exit $code): $out"
fi

# A driver that will not describe itself is an error naming the escape hatch.
out="$("$BIN" setup robot.test --bundle "$BUNDLE" --driver "$BUNDLE/bin/skynetd" \
  --yes --log-file "$T/nodescribe.log" 2>&1)" && code=0 || code=$?
if [ "$code" -ne 0 ] && [[ "$out" == *"would not describe itself"* ]] \
   && [[ "$out" == *"--descriptor"* ]]; then
  pass "an undescribable driver fails as a sentence naming --descriptor"
else
  fail "derive failure (exit $code): $out"
fi

# ── leg 8: the tail is offered, not taken ─────────────────────────────────────

# Three yeses: confirm, start, check. The stub answers the start and prints a canned
# doctor, and the token printed earlier authenticates nothing here (no --lan), so the
# check runs on the robot over the same stub ssh.
out="$(printf 'y\ny\ny\n' | "$BIN" setup robot.test --bundle "$BUNDLE" \
  --driver "$BUNDLE/lib/libskynet_mock.so" --descriptor "$BUNDLE/body.json" \
  --log-file "$T/tail.log" 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"started: skynetk and skynetd"* ]] \
   && [[ "$out" == *"What the robot says about itself"* ]] \
   && [[ "$out" == *"the robot reports healthy"* ]]; then
  pass "accepting the tail starts the units and runs the first check"
else
  fail "tail flow (exit $code): $out"
fi

# --yes declines the offers and prints the by-hand commands instead: starting a robot
# is not something a flag implies.
out="$("$BIN" setup robot.test --bundle "$BUNDLE" \
  --driver "$BUNDLE/lib/libskynet_mock.so" --descriptor "$BUNDLE/body.json" \
  --yes --log-file "$T/tailyes.log" 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"Nothing is running"* ]] \
   && [[ "$out" == *"systemctl start skynetk skynetd"* ]]; then
  pass "--yes leaves the tail as printed commands"
else
  fail "tail --yes flow (exit $code): $out"
fi

# ── leg 9: --robot ────────────────────────────────────────────────────────────

# Against the real manifest beside the test binary. Shipped first: this bundle carries
# the mock, so --robot mock resolves inside it and the run is host-and-model alone.
out="$(printf 'y\nn\nn\n' | "$BIN" setup robot.test --bundle "$BUNDLE" --robot mock \
  --descriptor "$BUNDLE/body.json" --log-file "$T/robot.log" 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"$BUNDLE/lib/libskynet_mock.so"* ]] \
   && [[ "$out" == *"installed:"* ]]; then
  pass "--robot mock resolves to the shipped library"
else
  fail "robot flow (exit $code): $out"
fi

# Then built beside the manifest: a bundle without lib/ falls through to the checkout
# (leg 7's guard builds the mock).
[ -f "$REPO/drivers/mock/libskynet_mock.so" ] || ( cd "$REPO" && make -C drivers/mock lib )
rm -rf "$T/nolib" && cp -r "$BUNDLE" "$T/nolib" && rm -rf "$T/nolib/lib"
out="$(printf 'y\nn\nn\n' | "$BIN" setup robot.test --bundle "$T/nolib" --robot mock \
  --descriptor "$BUNDLE/body.json" --log-file "$T/robotbuilt.log" 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ] && [[ "$out" == *"drivers/mock/libskynet_mock.so"* ]] \
   && [[ "$out" == *"installed:"* ]]; then
  pass "--robot mock falls through to the built library"
else
  fail "robot built flow (exit $code): $out"
fi

out="$("$BIN" setup robot.test --bundle "$BUNDLE" --robot nosuch --yes \
  --log-file "$T/robotunknown.log" 2>&1)" && code=0 || code=$?
if [ "$code" -ne 0 ] && [[ "$out" == *"no such robot model"* ]] && [[ "$out" == *"mock"* ]]; then
  pass "an unknown model lists the known ones"
else
  fail "robot unknown flow (exit $code): $out"
fi

out="$("$BIN" setup robot.test --bundle "$BUNDLE" --robot mock --driver "$MOCK" --yes \
  --log-file "$T/robotconflict.log" 2>&1)" && code=0 || code=$?
if [ "$code" -ne 0 ] && [[ "$out" == *"one of --robot or --driver"* ]]; then
  pass "--robot and --driver together are refused"
else
  fail "robot conflict flow (exit $code): $out"
fi

# ── leg 10: every model in the manifest (DRY-COVERAGE-PLAN.md item 12.B.3) ───

# Until this leg only `mock` was ever asked for, so a model's row could be wrong for as long as
# nobody typed its name. Every model in drivers/models.json is asked for here, and each answer is
# one of exactly two things: a plan, or a refusal that names what to do next (a build command, the
# SDK variable, the pointer to --driver). Anything else — an unnamed error, a panic, a hang — is a
# bug in the row. Dry-run, so nothing is copied and no robot is needed.
#
# The bundle gets a stand-in python-host so the Python rows reach their payload check rather than
# stopping at "the host is not built"; the payloads themselves are the checkout's, read from disk.
fake_elf "$BUNDLE/lib/libskynet_python_host.so"
if ! command -v python3 >/dev/null 2>&1; then
  fail "python3 is needed to list the manifest's models"
else
  MODELS="$(python3 -c 'import json,sys; print(" ".join(json.load(open(sys.argv[1]))["models"]))' "$REPO/drivers/models.json")"
  # What a refusal has to contain: one of the actions this command's refusals name.
  NAMED='make -C|cargo build|--driver|--bundle|--robot|packaging/bundle.sh|README'
  asked=0; planned=0; refused=0
  for model in $MODELS; do
    asked=$((asked + 1))
    out="$(cd "$REPO" && "$BIN" setup robot.test --bundle "$BUNDLE" --robot "$model" \
      --descriptor "$BUNDLE/body.json" --dry-run 2>&1)" && code=0 || code=$?
    # A driver that reaches its robot over IP is refused until it is told where; that refusal is
    # the wizard working, and the row is then asked again with an address.
    if [ "$code" -ne 0 ] && [[ "$out" == *"--driver-address"* ]]; then
      out="$(cd "$REPO" && "$BIN" setup robot.test --bundle "$BUNDLE" --robot "$model" \
        --descriptor "$BUNDLE/body.json" --driver-address 192.0.2.10 --dry-run 2>&1)" && code=0 || code=$?
    fi
    if [ "$code" -eq 0 ] && [[ "$out" == *"plan: install Skynet OS"* ]]; then
      planned=$((planned + 1))
    elif [ "$code" -ne 0 ] && printf '%s' "$out" | grep -Eq -- "$NAMED"; then
      refused=$((refused + 1))
    else
      fail "model $model: neither a plan nor a named refusal (exit $code): $(printf '%s' "$out" | head -n 4)"
    fi
  done
  total="$(printf '%s\n' $MODELS | wc -l)"
  if [ "$asked" -eq "$total" ] && [ "$asked" -ge 20 ] && [ $((planned + refused)) -eq "$asked" ]; then
    pass "all $asked manifest models answer: $planned a plan, $refused a refusal that names the next step"
  else
    fail "the manifest walk covered $asked of $total models ($planned plans, $refused refusals)"
  fi
  # The rows that must reach a plan from this bundle: the mock (shipped) and the Python drivers (the
  # stand-in host is in the bundle and their payloads are on disk). A Python row that refuses here
  # has lost its package or its binding.
  for model in mock lerobot ur_rtde xarm canopen; do
    out="$(cd "$REPO" && "$BIN" setup robot.test --bundle "$BUNDLE" --robot "$model" \
      --descriptor "$BUNDLE/body.json" --driver-address 192.0.2.10 --dry-run 2>&1)" && code=0 || code=$?
    if [ "$code" -ne 0 ] && [[ "$out" == *"only means something"* ]]; then
      out="$(cd "$REPO" && "$BIN" setup robot.test --bundle "$BUNDLE" --robot "$model" \
        --descriptor "$BUNDLE/body.json" --dry-run 2>&1)" && code=0 || code=$?
    fi
    if [ "$code" -eq 0 ] && [[ "$out" == *"plan: install Skynet OS"* ]]; then
      pass "--robot $model reaches a plan from a bundle"
      # DRY-COVERAGE-PLAN.md item 12.C.1: the plan for a driver that commands a robot says which layers a dry test
      # reaches for it and which it does not, and never lists the hardware or the site among the first.
      if [ "$model" != mock ]; then
        reached="$(printf '%s\n' "$out" | sed -n '/evidence:/,/seeded defects noticed/p')"
        if [[ "$reached" == *"what a dry test reaches"* ]] && [[ "$reached" == *"not reached:"* ]] \
           && [[ "$reached" == *"9 electrical, thermal, mechanical"* ]] && [[ "$reached" == *"10 the site"* ]] \
           && [[ "$reached" == *"seeded defects noticed"* ]] && [[ "$reached" == *"1 wire and framing r"* ]]; then
          pass "--robot $model's plan says which layers a dry test reaches, and that the hardware and the site are not among them"
        else
          fail "--robot $model's plan carries no dry-test evidence: $(printf '%s' "$out" | head -n 12)"
        fi
      fi
    else
      fail "--robot $model does not reach a plan (exit $code): $(printf '%s' "$out" | head -n 4)"
    fi
  done
fi

if [ "$FAILED" -ne 0 ]; then
  printf '\n%s check(s) failed\n' "$FAILED"
  exit 1
fi
printf '\nthe wizard installs\n'
