#!/usr/bin/env bash
# The installer narrates: numbered, timed steps and a log file that mirrors the terminal.
#
#   bash packaging/test-install-output.sh
#
# Three legs. A1 runs everywhere: bad paths, the `${2:?...}` backstop and the log mirror on a
# failing run. A2 needs systemd as PID 1 — the installer's own `require_host` — and runs a real
# `--check` against stub binaries, asserting the `✓ [N] (+Nms)` contract the PC wizard parses.
# B extracts the output functions from the real script and asserts their behaviour, including
# the color stripping a non-tty run cannot show. A leg whose precondition is missing says so
# loudly and skips, the way e2e.sh does; CI (ubuntu-latest) runs all three.
#
# Leg B reads the functions out of install.sh at test time rather than copying them: if the
# script drifts, the extraction fails loudly instead of testing a copy that no longer exists.

set -u

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
INSTALL="$HERE/install.sh"
FAILED=0
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED=$((FAILED + 1)); }
skip() { printf '  - SKIP %s\n' "$1"; }

# ── leg A1: everywhere ───────────────────────────────────────────────────────

# A log path that cannot be written fails as a bad path, before anything else runs — and the
# failure is one sentence, not bash's redirection diagnostic plus a sentence.
out="$("$INSTALL" --log-file /proc/there-is-no-such-dir/x.log --check 2>&1)"
code=$?
if [ "$code" -ne 0 ] && [[ "$out" == *"cannot write the log to"* ]] && [[ "$out" != *"No such file"* ]]; then
  pass "an unwritable --log-file fails as one sentence"
else
  fail "unwritable --log-file (exit $code): $out"
fi

# The backstop: `${2:?...}` exits before `die` is reached, and the EXIT trap still speaks.
out="$("$INSTALL" --driver 2>&1)"
code=$?
if [ "$code" -ne 0 ] && [[ "$out" == *"stopped early"* ]] && [[ "$out" == *"before its first step"* ]]; then
  pass "a missing argument value still gets a sentence (exit-trap backstop)"
else
  fail "exit-trap backstop (exit $code): $out"
fi

# Whatever the run says, the log file says exactly: same lines, no color codes. A plain
# --check, because an unknown flag dies during argument parsing — before logging starts.
"$INSTALL" --log-file "$T/mirror.log" --check >/dev/null 2>"$T/mirror.err" || true
if diff "$T/mirror.err" "$T/mirror.log" >/dev/null 2>&1; then
  pass "the log file mirrors the terminal exactly"
else
  fail "log mirror differs from the terminal:"
  diff "$T/mirror.err" "$T/mirror.log" | sed 's/^/      /'
fi
if grep -q $'\x1b' "$T/mirror.log"; then
  fail "color codes leaked into the log file"
else
  pass "no color codes in the log file"
fi

# ── leg A2: a real --check, needs systemd ────────────────────────────────────

if [ "$(ps -p 1 -o comm= 2>/dev/null || true)" != "systemd" ] || ! command -v systemctl >/dev/null 2>&1; then
  skip "leg A2: no systemd as PID 1 (this leg runs in CI)"
else
  mkdir -p "$T/bin"
  for b in skynetd skynetk skynet skynetp; do printf '#!/bin/sh\nexit 0\n' > "$T/bin/$b"; chmod +x "$T/bin/$b"; done
  "$INSTALL" --log-file "$T/check.log" --prefix "$T" --check >"$T/check.out" 2>"$T/check.err" || true

  # The contract, line by line: `✓ [N] (+Nms) <sentence>`, N counting from 1 without gaps.
  steps="$(grep -o '✓ \[[0-9]*\] (+[0-9]*ms)' "$T/check.err" || true)"
  if [ -z "$steps" ]; then
    fail "no numbered, timed steps in --check output"
  else
    expect=1
    bad=""
    while IFS= read -r line; do
      n="$(printf '%s' "$line" | sed 's/.*\[\([0-9]*\)\].*/\1/')"
      if [ "$n" != "$expect" ]; then bad="$line (wanted step $expect)"; break; fi
      expect=$((expect + 1))
    done <<< "$steps"
    if [ -z "$bad" ]; then
      pass "$((expect - 1)) steps, numbered 1..$((expect - 1)) and timed"
    else
      fail "step numbering broke: $bad"
    fi
  fi

  # The four stub binaries are the four passing checks, whatever else the machine holds.
  if grep -q 'skynetd is installed and executable' "$T/check.err" \
     && grep -q 'skynetp is installed and executable' "$T/check.err"; then
    pass "passing checks report as steps"
  else
    fail "the stub binaries did not report as steps"
  fi

  # FAIL lines and the verdict reach the log too — a mirror that skipped failures would be a
  # record of the good news.
  if grep -q 'FAIL' "$T/check.log" && grep -q . "$T/check.log"; then
    pass "failures and the verdict are in the log"
  else
    fail "the log is missing the failures"
  fi
  if diff "$T/check.err" "$T/check.log" >/dev/null 2>&1; then
    pass "--check output and its log are identical"
  else
    fail "--check log differs from the terminal"
  fi
fi

# ── leg B: the functions, extracted from the real script ─────────────────────

if ! date +%s%3N 2>/dev/null | grep -q '^[0-9][0-9]*$'; then
  skip "leg B: no GNU date (this leg runs in CI)"
else
  get() { awk "/^$1\\(\\) \\{/,/^\\}\$/" "$INSTALL"; }
  one() { grep -m1 "^$1() " "$INSTALL" || true; }
  EMIT="$(get emit)"; OKF="$(get ok)"; RUNF="$(get run)"; ONEXIT="$(get on_exit)"
  DIE="$(one die)"; NOTE="$(one note)"; WARN="$(one warn)"
  if [ -z "$EMIT" ] || [ -z "$OKF" ] || [ -z "$RUNF" ] || [ -z "$ONEXIT" ] \
     || [ -z "$DIE" ] || [ -z "$NOTE" ] || [ -z "$WARN" ]; then
    fail "could not extract the output functions from install.sh (it drifted)"
  else
    eval "$EMIT"; eval "$OKF"; eval "$RUNF"; eval "$ONEXIT"; eval "$DIE"; eval "$NOTE"; eval "$WARN"
    STEP=0; LAST_MS="$(date +%s%3N)"; DIED="no"; LOGGING="no"; LOG_FILE=""
    R=""; G=""; Y=""; D=""; B=""; Z=""

    # Numbering counts within the run; timing is milliseconds.
    err="$({ ok "first"; ok "second"; } 2>&1)"
    if printf '%s' "$err" | grep -q '^  ✓ \[1\] (+[0-9][0-9]*ms) first$' \
       && printf '%s' "$err" | grep -q '^  ✓ \[2\] (+[0-9][0-9]*ms) second$'; then
      pass "ok() numbers from 1 and times in ms"
    else
      fail "ok() shape: $err"
    fi

    # Color on the terminal, none in the file — with sentinel escapes, so no pty is needed.
    # STEP is reset: the two oks above ran in a command substitution, whose counter died with it.
    STEP=0
    R="<R>"; G="<G>"; Y="<Y>"; D="<D>"; B="<B>"; Z="<Z>"
    LOGGING="yes"; LOG_FILE="$T/strip.log"; : > "$LOG_FILE"
    err="$(ok "painted" 2>&1)"
    LOGGING="no"; R=""; G=""; Y=""; D=""; B=""; Z=""
    if [[ "$err" == *"<G>"* ]] && ! grep -q '<[RGYDBZ]>' "$T/strip.log" \
       && grep -q '✓ \[1\] (+[0-9][0-9]*ms) painted' "$T/strip.log"; then
      pass "the log strips color codes the terminal keeps"
    else
      fail "color stripping: terminal=[$err] file=[$(cat "$T/strip.log")]"
    fi

    # run(): success passes chatter through, failure carries the command's own words.
    err="$(run "declaring victory" echo hello 2>&1)"; code=$?
    if [ "$code" -eq 0 ] && [[ "$err" == *"hello"* ]]; then
      pass "run() passes success chatter through"
    else
      fail "run() success (exit $code): $err"
    fi
    err="$(run "frobbing the units" sh -c 'echo no-such-unit >&2; exit 3' 2>&1)"; code=$?
    if [ "$code" -ne 0 ] && [[ "$err" == *"frobbing the units"* ]] && [[ "$err" == *"no-such-unit"* ]]; then
      pass "run() failures name the step and the command's words"
    else
      fail "run() failure (exit $code): $err"
    fi

    # The backstop's two readings: mid-run it names the step, early it says so. The exit
    # status is not asserted: in production the process is already exiting when the trap runs,
    # so what on_exit returns is meaningless and only the sentence matters.
    err="$(STEP=2; DIED=no; false; on_exit 2>&1)"
    if [[ "$err" == *"after step 2"* ]]; then
      pass "the exit trap names the step it died after"
    else
      fail "exit trap mid-run: $err"
    fi
    err="$(STEP=0; DIED=no; false; on_exit 2>&1)"
    if [[ "$err" == *"before its first step"* ]]; then
      pass "the exit trap says so when nothing had run"
    else
      fail "exit trap early: $err"
    fi
  fi
fi

if [ "$FAILED" -ne 0 ]; then
  printf '\n%s check(s) failed\n' "$FAILED"
  exit 1
fi
printf '\nthe installer narrates\n'
