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

WASM="$WORK/add.wasm"

# Compile. A toolchain built WITHOUT --features llvm errors with a clear
# "requires the LLVM backend" message — treat that as SKIP, not FAIL.
COMPILE_OUT="$("$RUXEN" compile "$EXAMPLE/add.rx" \
  --target wasm32-unknown-unknown -o "$WASM" 2>&1)"
COMPILE_RC=$?
if [ $COMPILE_RC -ne 0 ]; then
  case "$COMPILE_OUT" in
    *"requires the LLVM backend"*|*"LLVM backend not available"*|*"wasm-ld not found"*)
      miss "wasm toolchain unavailable (rebuild with --features llvm): $COMPILE_OUT" ;;
    *)
      say "FAIL: ruxen compile --target wasm32-unknown-unknown errored:"
      say "$COMPILE_OUT"
      exit 1 ;;
  esac
fi

[ -f "$WASM" ] || { say "FAIL: no .wasm produced"; exit 1; }

# Validate + run the asserted exports in node.
if node "$EXAMPLE/run.mjs" "$WASM"; then
  say "PASS: wasm32-unknown-unknown — .wasm built by ruxen runs in node"
  exit 0
else
  say "FAIL: node run.mjs reported a wrong/invalid export"
  exit 1
fi
