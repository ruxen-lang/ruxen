#!/usr/bin/env bash
# WASM acceptance bar (tier 4.03).
#
# Compiles a Ruxen source to wasm32-unknown-unknown, links it with wasm-ld,
# and RUNS it in Node.js asserting the exported functions return the right
# values. This is the headline "a .wasm built by ruxen runs in a JS host"
# bar from the tier 4.03 / 4.04 definition of done.
#
# Requirements (all SKIP — not FAIL — when missing):
#   - a `ruxen` built with --features llvm (set RUXEN, or it falls back to
#     target/debug/ruxen then target/release/ruxen)
#   - wasm-ld (LLVM's lld; the compiler finds it at the LLVM-18 prefix or on
#     PATH, or via RUXEN_WASM_LD)
#   - node on PATH
#
# Usage:
#   scripts/wasm_verify.sh
#   RUXEN=/path/to/ruxen scripts/wasm_verify.sh
#
# Exit non-zero ONLY on a real failure (a bar ran and produced the wrong
# result). A skipped bar (no llvm-ruxen / no node) is not a failure.
set -u

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXAMPLE="$REPO_ROOT/examples/05-wasm"

# Pick a ruxen: explicit RUXEN, else debug, else release.
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

command -v node >/dev/null 2>&1 || miss "node not found"
[ -x "$RUXEN" ] || miss "ruxen binary not found at $RUXEN (build with --features llvm)"

# Verify one example: compile <dir>/<stem>.rx → wasm, run <dir>/run.mjs on it.
# Echoes PASS/SKIP/FAIL and returns 0 (pass), 2 (skip), or 1 (fail). A
# toolchain-without-llvm compile error is a SKIP, not a FAIL.
verify_example() {
  local dir="$1" stem="$2" label="$3"
  local wasm="$WORK/${stem}.wasm"
  local out rc
  out="$("$RUXEN" compile "$dir/${stem}.rx" --target wasm32-unknown-unknown -o "$wasm" 2>&1)"
  rc=$?
  if [ $rc -ne 0 ]; then
    case "$out" in
      *"requires the LLVM backend"*|*"LLVM backend not available"*|*"wasm-ld not found"*)
        say "SKIP: wasm toolchain unavailable ($label): $out"; return 2 ;;
      *)
        say "FAIL: ruxen compile $label errored:"; say "$out"; return 1 ;;
    esac
  fi
  [ -f "$wasm" ] || { say "FAIL: no .wasm produced ($label)"; return 1; }
  if node "$dir/run.mjs" "$wasm"; then
    say "PASS: $label — .wasm built by ruxen runs in node"; return 0
  else
    say "FAIL: node run.mjs reported a wrong/invalid export ($label)"; return 1
  fi
}

# Bar 1: pure-math reactor (tier 4.03). Bar 2: heap Array (tier 4.09).
verify_example "$EXAMPLE" "add" "05-wasm (math)"; rc1=$?
[ $rc1 -eq 2 ] && miss "math bar skipped — wasm toolchain unavailable"
verify_example "$REPO_ROOT/examples/07-wasm-heap" "heap" "07-wasm-heap (heap Array)"; rc2=$?

# A skipped heap bar is not a failure; a real FAIL is.
if [ $rc1 -eq 1 ] || [ $rc2 -eq 1 ]; then
  exit 1
fi
exit 0
