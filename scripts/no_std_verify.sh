#!/usr/bin/env bash
# no_std acceptance bar (tier 4.04).
#
#   (a) BUILD + RUN: a no_std unit compiles WITHOUT the Ruxen stdlib runtime
#       (no `ruxen_*` symbol in the binary), runs, and signals a computed
#       value (42) via a minimal libc `exit` FFI.
#   (b) E1400: a no_std unit that constructs a heap value (a string literal)
#       is REJECTED at compile time with error[E1400].
#
# Uses the default-features toolchain (no LLVM needed for the no_std host
# path). SKIPs (not FAILs) when no ruxen binary is available.
#
# Usage:
#   scripts/no_std_verify.sh
#   RUXEN=/path/to/ruxen scripts/no_std_verify.sh
set -u

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXAMPLE="$REPO_ROOT/examples/06-no-std"

if [ -n "${RUXEN:-}" ]; then
  :
elif [ -x "$REPO_ROOT/target/debug/ruxen" ]; then
  RUXEN="$REPO_ROOT/target/debug/ruxen"
else
  RUXEN="$REPO_ROOT/target/release/ruxen"
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

say()  { printf '%s\n' "$*"; }
miss() { say "SKIP: $*"; exit 0; }

[ -x "$RUXEN" ] || miss "ruxen binary not found at $RUXEN"

fail=0

# ── Bar (a): build + run a no_std unit, assert exit 42 + no stdlib symbols ──
BIN="$WORK/exit42"
if ! "$RUXEN" compile "$EXAMPLE/exit42.rx" --no-std -o "$BIN" 2>"$WORK/build.err"; then
  say "FAIL: no_std build errored:"; cat "$WORK/build.err"; fail=1
else
  "$BIN"; rc=$?
  if [ "$rc" -eq 42 ]; then
    say "PASS: no_std exit42 built + ran, exit=$rc"
  else
    say "FAIL: no_std exit42 ran but exit=$rc (want 42)"; fail=1
  fi
  # No Ruxen stdlib runtime symbol should be present.
  if command -v nm >/dev/null 2>&1; then
    n="$(nm "$BIN" 2>/dev/null | grep -c 'ruxen_' || true)"
    if [ "$n" -eq 0 ]; then
      say "PASS: no_std binary has zero ruxen_* stdlib symbols"
    else
      say "FAIL: no_std binary unexpectedly links $n ruxen_* symbols"; fail=1
    fi
  fi
fi

# ── Bar (b): E1400 — heap allocation in a no_std unit is rejected ──
if "$RUXEN" compile "$EXAMPLE/heap_rejected.rx" --no-std -o "$WORK/x" 2>"$WORK/e1400.err"; then
  say "FAIL: heap_rejected.rx compiled under --no-std (expected E1400)"; fail=1
else
  if grep -q 'E1400' "$WORK/e1400.err"; then
    say "PASS: heap allocation in a no_std unit rejected with E1400"
  else
    say "FAIL: --no-std rejected heap_rejected.rx but without E1400:"
    cat "$WORK/e1400.err"; fail=1
  fi
fi

exit $fail
