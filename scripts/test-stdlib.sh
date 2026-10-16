#!/usr/bin/env bash
# Run `ruxen test` for stdlib packages, one process per package in parallel.
#
# Usage: scripts/test-stdlib.sh [package ...]
#   No arguments runs every library/std/<pkg> that has a tests/ directory.
#   Uses target/release/ruxen (build it with `cargo build --release --bins`),
#   or $RUXEN_BIN when set. Exits nonzero when any package fails.
set -uo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
ruxen="${RUXEN_BIN:-$repo/target/release/ruxen}"
std="$repo/library/std"
logs="$repo/tmp/test-stdlib"

if [[ ! -x "$ruxen" ]]; then
  echo "test-stdlib: $ruxen not found; run cargo build --release --bins" >&2
  exit 2
fi

if (($# > 0)); then
  pkgs=("$@")
else
  pkgs=()
  for d in "$std"/*/tests; do
    [[ -d "$d" ]] && pkgs+=("$(basename "$(dirname "$d")")")
  done
fi

if ((${#pkgs[@]} == 0)); then
  echo "test-stdlib: no stdlib package has a tests/ directory"
  exit 0
fi

mkdir -p "$logs"
# A run that dies before writing its status must not reuse an old one.
rm -f "$logs"/*.status
export RUXEN_STDLIB_PATH="$std"

# Every package shares the workspace's incremental cache, and a new toolchain
# clears it on first use. Build serially so the parallel runs only hit it.
for pkg in "${pkgs[@]}"; do
  if [[ -d "$std/$pkg/tests" ]]; then
    (cd "$std/$pkg" && "$ruxen" test --no-run) >"$logs/$pkg.build.log" 2>&1
  fi
done

for pkg in "${pkgs[@]}"; do
  (
    start=$(date +%s.%N)
    if [[ -d "$std/$pkg/tests" ]]; then
      (cd "$std/$pkg" && "$ruxen" test) >"$logs/$pkg.log" 2>&1
      status=$?
    else
      echo "no tests/ directory in library/std/$pkg" >"$logs/$pkg.log"
      status=1
    fi
    end=$(date +%s.%N)
    echo "$status $(awk "BEGIN { printf \"%.2f\", $end - $start }")" >"$logs/$pkg.status"
  ) &
done
wait

failed=0
for pkg in "${pkgs[@]}"; do
  status=missing secs=?
  [[ -f "$logs/$pkg.status" ]] && read -r status secs <"$logs/$pkg.status"
  if [[ "$status" == 0 ]]; then
    printf 'pass  %-16s %6ss\n' "$pkg" "$secs"
  else
    printf 'FAIL  %-16s %6ss\n' "$pkg" "$secs"
    [[ "$status" == missing ]] && echo "      (no status file: the run died early)"
    [[ -f "$logs/$pkg.log" ]] && sed 's/^/      /' "$logs/$pkg.log"
    failed=1
  fi
done
exit "$failed"
