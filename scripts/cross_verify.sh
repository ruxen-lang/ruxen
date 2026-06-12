#!/usr/bin/env bash
# Cross-compilation acceptance bars (tier 4.02).
#
#   (a) aarch64-unknown-linux-gnu  — cross-OS. Built from macOS via the
#       two-stage Docker flow; the ELF must RUN in a native linux/arm64
#       container and print the expected stdout. SKIPPED (not failed) when
#       Docker is unavailable.
#   (b) x86_64-apple-darwin        — cross-arch. Built + linked locally on a
#       macOS arm64 host; the Mach-O x86_64 binary must RUN under Rosetta and
#       print the expected stdout. SKIPPED when not on macOS arm64 / no Rosetta.
#
# Usage:
#   scripts/cross_verify.sh            # uses ./target/release/ruxen
#   RUXEN=/path/to/ruxen scripts/cross_verify.sh
#
# Exit non-zero only on a REAL failure (a bar ran and produced the wrong
# result). A skipped bar (missing Docker/Rosetta) is not a failure.
set -u

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUXEN="${RUXEN:-$REPO_ROOT/target/release/ruxen}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

EXPECT="hello cross-compile"
SRC="$WORK/hello.rx"
cat > "$SRC" <<EOF
def main
  println("$EXPECT")
end
EOF

fail=0
pass=0
skip=0

say()  { printf '%s\n' "$*"; }
ok()   { pass=$((pass+1)); say "PASS: $*"; }
bad()  { fail=$((fail+1)); say "FAIL: $*"; }
miss() { skip=$((skip+1)); say "SKIP: $*"; }

if [ ! -x "$RUXEN" ]; then
  say "error: ruxen binary not found/executable at: $RUXEN"
  say "       build it first: cargo build --release -p ruxen_cli"
  exit 2
fi

say "== ruxen: $RUXEN =="
say "== expected stdout: '$EXPECT' =="
say ""

# ─── Bar (a): aarch64-unknown-linux-gnu via Docker ───────────────────────
say "── bar (a) aarch64-unknown-linux-gnu (cross-OS, Docker) ──"
if ! command -v docker >/dev/null 2>&1; then
  miss "bar (a): docker not installed — cross-OS Linux bar skipped"
elif ! docker info >/dev/null 2>&1; then
  miss "bar (a): docker daemon not running — cross-OS Linux bar skipped"
else
  OUT_A="$WORK/hello_arm_linux"
  if "$RUXEN" compile "$SRC" -o "$OUT_A" --target aarch64-unknown-linux-gnu; then
    arch_line="$(file "$OUT_A" 2>/dev/null)"
    case "$arch_line" in
      *aarch64*|*ARM\ aarch64*) ;;
      *) bad "bar (a): produced binary is not aarch64 ELF: $arch_line" ;;
    esac
    got="$(docker run --rm --platform linux/arm64 \
            -v "$OUT_A":/app/hello:ro -w /app gcc:13 /app/hello 2>&1)"
    if [ "$got" = "$EXPECT" ]; then
      ok "bar (a): aarch64 ELF runs in linux/arm64 container → '$got'"
    else
      bad "bar (a): container run mismatch — got '$got', want '$EXPECT'"
    fi
  else
    bad "bar (a): cross-compile to aarch64-unknown-linux-gnu failed"
  fi
fi
say ""

# ─── Bar (b): x86_64-apple-darwin under Rosetta ──────────────────────────
say "── bar (b) x86_64-apple-darwin (cross-arch, Rosetta) ──"
host_os="$(uname -s)"
host_arch="$(uname -m)"
if [ "$host_os" != "Darwin" ]; then
  miss "bar (b): not a macOS host ($host_os) — cross-arch Darwin bar skipped"
else
  OUT_B="$WORK/hello_x64"
  if "$RUXEN" compile "$SRC" -o "$OUT_B" --target x86_64-apple-darwin; then
    arch_line="$(file "$OUT_B" 2>/dev/null)"
    case "$arch_line" in
      *x86_64*) ;;
      *) bad "bar (b): produced binary is not x86_64 Mach-O: $arch_line" ;;
    esac
    # On arm64, x86_64 runs under Rosetta (auto). On x86_64, it runs natively.
    got="$("$OUT_B" 2>&1)"
    if [ "$got" = "$EXPECT" ]; then
      ok "bar (b): x86_64 Mach-O runs (arch=$host_arch) → '$got'"
    else
      bad "bar (b): run mismatch — got '$got', want '$EXPECT'"
    fi
  else
    bad "bar (b): cross-compile to x86_64-apple-darwin failed"
  fi
fi
say ""

say "== summary: $pass passed, $skip skipped, $fail failed =="
[ "$fail" -eq 0 ]
