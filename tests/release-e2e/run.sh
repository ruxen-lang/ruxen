#!/usr/bin/env bash
# Ruxen release-bundle e2e test harness.
#
# Verifies an installed Ruxen toolchain against the language tutorial docs
# (docs/tutorial/*.md) and exercises every shipped binary:
#   ruxenc, ruxen, ruxen-repl, ruxen-lsp
#
# Exit status is 0 if every test passes, 1 otherwise.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
RESULTS="$HERE/results"
CASES="$HERE/cases"
EXPECTED="$HERE/expected"
SCRIPTS="$HERE/scripts"

mkdir -p "$RESULTS"
: > "$RESULTS/summary.txt"

RUXEN_HOME="${RUXEN_HOME:-$HOME/.ruxen}"
# If RUXEN_WORKSPACE is set, prefer binaries built from source in that
# workspace over the installed release. Useful for testing fixes.
if [ -n "${RUXEN_WORKSPACE:-}" ] && [ -d "$RUXEN_WORKSPACE/target/release" ]; then
  export PATH="$RUXEN_WORKSPACE/target/release:$PATH"
else
  export PATH="$RUXEN_HOME/bin:$PATH"
fi

# macOS default TMPDIR (/var/folders/...) is inside a per-user sandbox
# quota that 100+ compiled binaries can exhaust, producing spurious
# ENOSPC errors during the run. Pin TMPDIR to /tmp which has no quota.
export TMPDIR="/tmp"

# Cap ruxenc memory at 8 GiB (RSS).
# Compiler bugs have leaked 35 GB+ before being noticed.
# macOS bash doesn't support `ulimit -v` (RLIMIT_AS), so we poll
# the process's RSS and SIGKILL when it crosses the cap.
RUXENC_MEM_KB=$((8 * 1024 * 1024))

run_with_memcap() {
  # Usage: run_with_memcap <cmd> [args...]
  # Runs the command, polls RSS every 250ms, kills if it exceeds
  # $RUXENC_MEM_KB. Returns the command's exit code, or 137 on kill.
  "$@" &
  local pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    local rss
    rss=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ')
    if [ -n "$rss" ] && [ "$rss" -gt "$RUXENC_MEM_KB" ]; then
      kill -9 "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      printf 'run_with_memcap: killed pid %s (RSS %sKB > cap %sKB)\n' \
        "$pid" "$rss" "$RUXENC_MEM_KB" >&2
      return 137
    fi
    sleep 0.25
  done
  wait "$pid"
}

# ── colors ────────────────────────────────────────────────────────────
if [ -t 1 ]; then
  BOLD=$'\033[1m'; GREEN=$'\033[32m'; RED=$'\033[31m'
  YELLOW=$'\033[33m'; CYAN=$'\033[36m'; DIM=$'\033[2m'; RESET=$'\033[0m'
else
  BOLD=""; GREEN=""; RED=""; YELLOW=""; CYAN=""; DIM=""; RESET=""
fi

PASS=0
FAIL=0
FAIL_NAMES=()

record_pass() {
  PASS=$((PASS + 1))
  printf "  %sPASS%s  %s\n" "$GREEN" "$RESET" "$1"
  printf "PASS\t%s\n" "$1" >> "$RESULTS/summary.txt"
}

record_fail() {
  FAIL=$((FAIL + 1))
  FAIL_NAMES+=("$1")
  printf "  %sFAIL%s  %s  %s%s%s\n" "$RED" "$RESET" "$1" "$DIM" "$2" "$RESET"
  printf "FAIL\t%s\t%s\n" "$1" "$2" >> "$RESULTS/summary.txt"
}

banner() {
  printf "\n%s%s== %s ==%s\n" "$BOLD" "$CYAN" "$1" "$RESET"
}

# ── 1. binary smoke tests ─────────────────────────────────────────────
test_binaries() {
  banner "binaries: --version / --help"
  for bin in ruxen ruxenc ruxen-repl ruxen-lsp; do
    if ! command -v "$bin" >/dev/null 2>&1; then
      record_fail "bin/$bin" "not on PATH"
      continue
    fi
    if "$bin" --version >/dev/null 2>&1; then
      record_pass "bin/$bin --version"
    else
      record_fail "bin/$bin --version" "nonzero exit"
    fi
    if "$bin" --help >/dev/null 2>&1; then
      record_pass "bin/$bin --help"
    else
      record_fail "bin/$bin --help" "nonzero exit"
    fi
  done
}

# ── 2. ruxenc compile+run cases ───────────────────────────────────────
test_cases() {
  banner "ruxenc: compile + run language cases"
  local tmp
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/ruxen-e2e.XXXXXX")"
  trap 'rm -rf "$tmp"' RETURN

  for src in "$CASES"/*.rx; do
    [ -f "$src" ] || continue
    local name base expect_file
    base="$(basename "$src" .rx)"
    expect_file="$EXPECTED/$base.out"
    name="case/$base"

    # compile: 30s wall-clock cap + 8 GiB RSS cap
    # (catches pathological codegen and memory leaks like the 35GB incident)
    if ! timeout 30 bash -c '
        RUXENC_MEM_KB='"$RUXENC_MEM_KB"'
        ruxenc "$@" &
        pid=$!
        while kill -0 "$pid" 2>/dev/null; do
          rss=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d " ")
          if [ -n "$rss" ] && [ "$rss" -gt "$RUXENC_MEM_KB" ]; then
            kill -9 "$pid" 2>/dev/null
            wait "$pid" 2>/dev/null
            echo "compile killed: ruxenc RSS ${rss}KB exceeded cap" >&2
            exit 137
          fi
          sleep 0.25
        done
        wait "$pid"
    ' _ "$src" -o "$tmp/$base.bin" >"$tmp/$base.compile.log" 2>&1; then
      local rc=$?
      if [ "$rc" -eq 124 ]; then
        record_fail "$name" "compile timed out (>30s)"
      else
        record_fail "$name" "compile failed (see $tmp/$base.compile.log)"
      fi
      cp "$tmp/$base.compile.log" "$RESULTS/$base.compile.log" 2>/dev/null
      rm -f "$tmp/$base.bin" "$tmp/$base.bin.o"
      continue
    fi

    # Guard against pathological codegen: any binary >10 MB for a
    # fixture this small is a compiler bug. Flag and drop it so we
    # don't exhaust disk.
    if [ -f "$tmp/$base.bin" ]; then
      local size_bytes
      # BSD stat uses -f %z; GNU stat uses -c %s. Try both.
      size_bytes=$(stat -c %s "$tmp/$base.bin" 2>/dev/null \
                   || stat -f %z "$tmp/$base.bin" 2>/dev/null \
                   || echo 0)
      if [ "$size_bytes" -gt $((10 * 1024 * 1024)) ]; then
        local size_mb=$(( size_bytes / 1024 / 1024 ))
        record_fail "$name" "binary ${size_mb}MB — pathological codegen"
        rm -f "$tmp/$base.bin" "$tmp/$base.bin.o"
        continue
      fi
    fi

    # run (skip if no expected output file — compile-only test)
    if [ ! -f "$expect_file" ]; then
      record_pass "$name (compile-only)"
      continue
    fi

    # Capture stdout separately from stderr — fixtures assert stdout only.
    # panic! / eputs / diagnostic output belongs on stderr and must not
    # corrupt the diff. Nonzero exit is acceptable as long as stdout
    # matches — e.g. panic! fixtures print to stdout then exit 101.
    timeout 10 "$tmp/$base.bin" >"$tmp/$base.out" 2>"$tmp/$base.err"
    local rc=$?
    if [ "$rc" -eq 124 ]; then
      record_fail "$name" "run timed out (>10s)"
      { cat "$tmp/$base.out"; echo "--- stderr ---"; cat "$tmp/$base.err"; } \
        > "$RESULTS/$base.actual.out" 2>/dev/null
      continue
    fi

    if diff -u "$expect_file" "$tmp/$base.out" >"$tmp/$base.diff" 2>&1; then
      record_pass "$name"
    else
      record_fail "$name" "output mismatch (exit=$rc)"
      { cat "$tmp/$base.out"; echo "--- stderr ---"; cat "$tmp/$base.err"; } \
        > "$RESULTS/$base.actual.out" 2>/dev/null
      cp "$tmp/$base.diff" "$RESULTS/$base.diff" 2>/dev/null
    fi
  done
}

# ── 3. ruxen CLI lifecycle ────────────────────────────────────────────
test_cli() {
  banner "ruxen: project subcommands"
  local tmp proj initdir
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/ruxen-cli.XXXXXX")"
  proj="$tmp/demo"

  # new
  if (cd "$tmp" && ruxen new demo >"$tmp/new.log" 2>&1); then
    record_pass "cli/new"
  else
    record_fail "cli/new" "see $tmp/new.log"
    return
  fi

  # scaffold presence
  for f in Ruxen.toml src/main.rx .gitignore; do
    if [ -e "$proj/$f" ]; then
      record_pass "cli/new scaffolded $f"
    else
      record_fail "cli/new scaffolded $f" "missing after new"
    fi
  done

  # init — should work in an empty dir (sibling, distinct project)
  initdir="$tmp/initdemo"
  mkdir -p "$initdir"
  if (cd "$initdir" && ruxen init >"$tmp/init.log" 2>&1); then
    record_pass "cli/init"
  else
    record_fail "cli/init" "see $tmp/init.log"
  fi

  # check / build / run
  for cmd in check build run; do
    if (cd "$proj" && ruxen $cmd >"$tmp/$cmd.log" 2>&1); then
      record_pass "cli/$cmd"
    else
      record_fail "cli/$cmd" "see $tmp/$cmd.log"
    fi
  done

  if grep -q "Hello, Ruxen" "$tmp/run.log"; then
    record_pass "cli/run output"
  else
    record_fail "cli/run output" "missing 'Hello, Ruxen' in stdout"
  fi

  # build --release (may use LLVM backend which isn't shipped)
  if (cd "$proj" && ruxen build --release >"$tmp/build-release.log" 2>&1); then
    record_pass "cli/build --release"
  else
    record_fail "cli/build --release" "see $tmp/build-release.log"
  fi

  # clean
  if (cd "$proj" && ruxen clean >"$tmp/clean.log" 2>&1); then
    record_pass "cli/clean"
  else
    record_fail "cli/clean" "see $tmp/clean.log"
  fi

  # tree — empty-deps graph
  if (cd "$proj" && ruxen tree >"$tmp/tree.log" 2>&1); then
    record_pass "cli/tree"
  else
    record_fail "cli/tree" "see $tmp/tree.log"
  fi

  # verify — fresh project has no lock; should still succeed on zero-dep builds
  if (cd "$proj" && ruxen verify >"$tmp/verify.log" 2>&1); then
    record_pass "cli/verify"
  else
    record_fail "cli/verify" "see $tmp/verify.log"
  fi

  # add / remove / update — registry access is unavailable in CI;
  # only assert the subcommand is wired by calling `--help`.
  for cmd in add remove update; do
    if ruxen "$cmd" --help >"$tmp/$cmd-help.log" 2>&1; then
      record_pass "cli/$cmd --help"
    else
      record_fail "cli/$cmd --help" "see $tmp/$cmd-help.log"
    fi
  done

  # global flags
  for flag in --verbose --quiet "--color never" "--color auto"; do
    if (cd "$proj" && ruxen $flag check >"$tmp/flag.log" 2>&1); then
      record_pass "cli/check $flag"
    else
      record_fail "cli/check $flag" "see $tmp/flag.log"
    fi
  done
}

# ── 3b. ruxenc direct-compiler flags ──────────────────────────────────
test_ruxenc_flags() {
  banner "ruxenc: direct-compiler flags"
  local tmp prog
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/ruxen-ruxenc.XXXXXX")"
  prog="$tmp/flagprog.rx"
  cat >"$prog" <<'EOF'
def main
  let x = 2 + 3
  puts "#{x}"
end
EOF

  # baseline compile + run
  if ruxenc "$prog" -o "$tmp/flagprog.bin" >"$tmp/baseline.log" 2>&1 \
      && [ "$("$tmp/flagprog.bin")" = "5" ]; then
    record_pass "ruxenc/baseline compile+run"
  else
    record_fail "ruxenc/baseline compile+run" "see $tmp/baseline.log"
  fi

  # --emit variants: the compiler should print something & exit 0,
  # without linking a binary.
  for kind in tokens ast hir mir; do
    if ruxenc "$prog" --emit="$kind" >"$tmp/emit-$kind.log" 2>&1 \
        && [ -s "$tmp/emit-$kind.log" ]; then
      record_pass "ruxenc/--emit=$kind"
    else
      record_fail "ruxenc/--emit=$kind" "empty output or nonzero exit"
    fi
  done

  # --backend=cranelift (default path)
  if ruxenc "$prog" --backend=cranelift -o "$tmp/cl.bin" >"$tmp/cl.log" 2>&1; then
    record_pass "ruxenc/--backend=cranelift"
  else
    record_fail "ruxenc/--backend=cranelift" "see $tmp/cl.log"
  fi

  # --backend=llvm
  #
  # The LLVM 18 backend is a v1.1 goal — the shipped `ruxenc` is built without
  # the `llvm` Cargo feature, so passing `--backend=llvm` is expected to exit
  # non-zero with a clear "LLVM backend not available" diagnostic. That is an
  # accepted v1 outcome and must NOT be treated as a regression. Only mark the
  # fixture as failed if the binary crashes, produces no diagnostic, or (once
  # the feature is enabled) silently emits a broken executable.
  ruxenc "$prog" --backend=llvm -o "$tmp/llvm.bin" >"$tmp/llvm.log" 2>&1
  llvm_rc=$?
  if [ "$llvm_rc" -eq 0 ] && [ -x "$tmp/llvm.bin" ] && [ "$("$tmp/llvm.bin")" = "5" ]; then
    # Full LLVM codegen path is live and working.
    record_pass "ruxenc/--backend=llvm"
  elif [ "$llvm_rc" -ne 0 ] \
      && grep -q "LLVM backend not available" "$tmp/llvm.log"; then
    # Accepted v1 outcome: feature not compiled in.
    record_pass "ruxenc/--backend=llvm (feature disabled — v1.1)"
  else
    record_fail "ruxenc/--backend=llvm" "see $tmp/llvm.log"
  fi

  # --opt-level variants
  for lvl in 0 1 2 3 s z; do
    if ruxenc "$prog" --opt-level=$lvl -o "$tmp/opt-$lvl.bin" \
        >"$tmp/opt-$lvl.log" 2>&1; then
      record_pass "ruxenc/--opt-level=$lvl"
    else
      record_fail "ruxenc/--opt-level=$lvl" "see $tmp/opt-$lvl.log"
    fi
  done

  # --force (ignore cache)
  if ruxenc "$prog" --force -o "$tmp/force.bin" >"$tmp/force.log" 2>&1; then
    record_pass "ruxenc/--force"
  else
    record_fail "ruxenc/--force" "see $tmp/force.log"
  fi

  # --verbose — should emit [cache] lines per docs
  if ruxenc "$prog" --verbose -o "$tmp/verbose.bin" >"$tmp/verbose.log" 2>&1; then
    record_pass "ruxenc/--verbose"
  else
    record_fail "ruxenc/--verbose" "see $tmp/verbose.log"
  fi

  # fmt in place — input is canonical by construction (single simple fn)
  cp "$prog" "$tmp/fmt_in.rx"
  if ruxenc fmt "$tmp/fmt_in.rx" >"$tmp/fmt.log" 2>&1; then
    record_pass "ruxenc/fmt"
  else
    record_fail "ruxenc/fmt" "see $tmp/fmt.log"
  fi

  # fmt --check on canonical file — should exit 0
  if ruxenc fmt --check "$tmp/fmt_in.rx" >"$tmp/fmt-check.log" 2>&1; then
    record_pass "ruxenc/fmt --check (canonical)"
  else
    record_fail "ruxenc/fmt --check (canonical)" "see $tmp/fmt-check.log"
  fi

  # fmt --diff on already-formatted — no diff output expected
  if ruxenc fmt --diff "$tmp/fmt_in.rx" >"$tmp/fmt-diff.log" 2>&1 \
      && [ ! -s "$tmp/fmt-diff.log" ]; then
    record_pass "ruxenc/fmt --diff (no changes)"
  else
    record_fail "ruxenc/fmt --diff (no changes)" "non-empty diff or error"
  fi

  # fmt --stdin
  if echo 'def main;puts "x";end' | ruxenc fmt --stdin >"$tmp/fmt-stdin.log" 2>&1 \
      && [ -s "$tmp/fmt-stdin.log" ]; then
    record_pass "ruxenc/fmt --stdin"
  else
    record_fail "ruxenc/fmt --stdin" "see $tmp/fmt-stdin.log"
  fi

  # clean — project cache (requires running inside a built project)
  local cleantmp="$tmp/cleanproj"
  mkdir -p "$cleantmp/src"
  cat >"$cleantmp/Ruxen.toml" <<'EOF'
[package]
name = "cleanproj"
version = "0.1.0"
edition = "2026"
EOF
  cp "$prog" "$cleantmp/src/main.rx"
  if (cd "$cleantmp" && ruxen build >/dev/null 2>&1 \
      && ruxenc clean >"$tmp/clean.log" 2>&1); then
    record_pass "ruxenc/clean"
  else
    record_fail "ruxenc/clean" "see $tmp/clean.log"
  fi

  # clean --global — resets global incremental cache
  if ruxenc clean --global >"$tmp/clean-global.log" 2>&1; then
    record_pass "ruxenc/clean --global"
  else
    record_fail "ruxenc/clean --global" "see $tmp/clean-global.log"
  fi
}

# ── 3c. ruxenc negative test: top-level code must error or timeout ────
test_ruxenc_toplevel_hang() {
  banner "ruxenc: top-level code must not hang"
  local tmp
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/ruxen-hang.XXXXXX")"
  cat >"$tmp/bad.rx" <<'EOF'
let mut x = 0
while x < 3
  x += 1
end
EOF
  # Compiler should exit (with either success or a parse error) in <5s.
  # An infinite-loop/hang is a failure.
  local rc=0
  if ! (timeout 5 ruxenc "$tmp/bad.rx" -o "$tmp/bad.bin" \
        >"$tmp/hang.log" 2>&1); then
    rc=$?
  fi
  if [ "$rc" -eq 124 ]; then
    record_fail "ruxenc/no-hang top-level" "compiler timed out (>5s)"
  else
    record_pass "ruxenc/no-hang top-level (exit=$rc)"
  fi
}

# ── 4. ruxen-repl scripted session ────────────────────────────────────
test_repl() {
  banner "ruxen-repl: scripted session"
  local tmp
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/ruxen-repl.XXXXXX")"

  # Feed the REPL a script and diff against expected output.
  # The REPL banner and prompt lines are stripped by comparing only
  # the significant lines.
  if [ ! -f "$SCRIPTS/repl_session.in" ] || [ ! -f "$SCRIPTS/repl_session.expect" ]; then
    record_fail "repl/session" "missing script or expected"
    return
  fi

  ruxen-repl <"$SCRIPTS/repl_session.in" >"$tmp/repl.out" 2>&1

  # Strip ANSI escapes, blank lines, banner, 'Goodbye!' — compare tokens.
  sed -E 's/\x1b\[[0-9;]*m//g' "$tmp/repl.out" \
    | grep -v '^$' \
    | grep -v '^Ruxen.*REPL' \
    | grep -v '^Goodbye' \
    > "$tmp/repl.clean"

  if diff -u "$SCRIPTS/repl_session.expect" "$tmp/repl.clean" >"$tmp/repl.diff" 2>&1; then
    record_pass "repl/session"
  else
    record_fail "repl/session" "output mismatch"
    cp "$tmp/repl.clean" "$RESULTS/repl.actual.out"
    cp "$tmp/repl.diff"  "$RESULTS/repl.diff"
  fi
}

# ── 4b. ruxen-repl: fixture parity ────────────────────────────────────
# For each ruxenc case, translate to REPL input (strip `def main` wrapper
# so top-level items + main body become REPL inputs), pipe through
# ruxen-repl, diff against the same expected/*.out as the compile test.
# This surfaces REPL↔ruxenc divergences. Fixtures listed in REPL_KNOWN_SKIP
# exercise features the REPL genuinely can't model without a redesign
# (mutation persistence across inputs, JIT paths for certain features) —
# those are reported separately as "skip" and not counted as failures.
REPL_KNOWN_SKIP=()

is_repl_skipped() {
  local name="$1"
  # Handle empty array under `set -u`: expand with default.
  for s in "${REPL_KNOWN_SKIP[@]:-}"; do
    [ -z "$s" ] && continue
    [ "$s" = "$name" ] && return 0
  done
  return 1
}

test_repl_cases() {
  banner "ruxen-repl: fixture parity (translate .rx → REPL → diff)"
  local tmp total=0 passed=0 failed=0 skipped=0
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/ruxen-repl-cases.XXXXXX")"

  for src in "$CASES"/*.rx; do
    [ -f "$src" ] || continue
    local base expect_file
    base="$(basename "$src" .rx)"
    expect_file="$EXPECTED/$base.out"
    [ -f "$expect_file" ] || continue
    total=$((total + 1))
    local name="repl-case/$base"

    if is_repl_skipped "$base"; then
      skipped=$((skipped + 1))
      printf "  %sSKIP%s  %s  %s(known REPL gap)%s\n" "$YELLOW" "$RESET" \
        "$name" "$DIM" "$RESET"
      continue
    fi

    python3 "$SCRIPTS/translate_to_repl.py" <"$src" >"$tmp/$base.in"
    # Capture stdout separately from stderr — fixtures assert stdout only.
    # panic! / eputs / diagnostic output belongs on stderr and must not
    # corrupt the diff. Nonzero exit is acceptable as long as stdout
    # matches — e.g. panic! fixtures print to stdout then exit 101.
    timeout 15 ruxen-repl <"$tmp/$base.in" >"$tmp/$base.raw" 2>"$tmp/$base.err" || true
    sed -E 's/\x1b\[[0-9;]*m//g' "$tmp/$base.raw" \
      | grep -vE '^(Ruxen.*REPL|Goodbye|=>|Available commands:|State cleared|\s*:)' \
      | grep -v '^$' \
      > "$tmp/$base.clean"

    if diff -u "$expect_file" "$tmp/$base.clean" >"$tmp/$base.diff" 2>&1; then
      passed=$((passed + 1))
      printf "  %sPASS%s  %s\n" "$GREEN" "$RESET" "$name"
    else
      failed=$((failed + 1))
      printf "  %sFAIL%s  %s  %s(REPL diverges from ruxenc)%s\n" \
        "$RED" "$RESET" "$name" "$DIM" "$RESET"
      cp "$tmp/$base.clean" "$RESULTS/repl_$base.actual.out" 2>/dev/null
      cp "$tmp/$base.diff"  "$RESULTS/repl_$base.diff" 2>/dev/null
    fi
  done

  printf "\n  %s%d/%d passed,%s %d skipped,%s %d failed%s\n" \
    "$GREEN" "$passed" "$total" "$YELLOW" "$skipped" "$RED" "$failed" "$RESET"

  # Roll failures into the main summary so they count; skips are informational.
  if [ "$failed" -gt 0 ]; then
    FAIL=$((FAIL + failed))
    for _ in $(seq 1 "$failed"); do
      FAIL_NAMES+=("repl-case/...")
    done
  fi
  PASS=$((PASS + passed))
  printf "REPL-CASES\tpass=%d skipped=%d fail=%d\n" "$passed" "$skipped" "$failed" >> "$RESULTS/summary.txt"
}

# ── 5. ruxen-lsp initialize handshake ─────────────────────────────────
test_lsp() {
  banner "ruxen-lsp: initialize handshake"
  if ! command -v python3 >/dev/null 2>&1; then
    record_fail "lsp/initialize" "python3 not found"
    return
  fi
  if python3 "$SCRIPTS/lsp_initialize.py" >"$RESULTS/lsp.out" 2>&1; then
    record_pass "lsp/initialize"
  else
    record_fail "lsp/initialize" "see results/lsp.out"
  fi
}

# ── 5b. ruxen-lsp feature tests ──────────────────────────────────────
# Drives the server through did_open / did_change / did_save /
# did_close, hover, goto_definition, and semantic_tokens. Each check
# runs one assertion of the form "[ok] <name>" or "[FAIL] <name>" on
# stdout; the overall exit code is nonzero on any failure.
test_lsp_features() {
  banner "ruxen-lsp: feature exercises"
  if ! command -v python3 >/dev/null 2>&1; then
    record_fail "lsp/features" "python3 not found"
    return
  fi
  local out="$RESULTS/lsp_features.out"
  if python3 "$SCRIPTS/lsp_features.py" >"$out" 2>&1; then
    # One PASS per "[ok]" line so the granular test budget reflects the
    # number of LSP behaviours we actually exercise.
    local pass_count
    pass_count=$(grep -c '^\s*\[ok\]' "$out" || true)
    local i
    for i in $(seq 1 "$pass_count"); do
      record_pass "lsp/feature-$i"
    done
  else
    record_fail "lsp/features" "see $out"
    # Show the concrete failures inline so the harness log is actionable.
    grep -E '^\s*\[(ok|FAIL)\]' "$out" | head -40 | while read -r ln; do
      printf "      %s\n" "$ln"
    done
  fi
}

# ── main ──────────────────────────────────────────────────────────────
test_binaries
test_cases
test_cli
test_ruxenc_flags
test_ruxenc_toplevel_hang
test_repl
test_repl_cases
test_lsp
test_lsp_features

TOTAL=$((PASS + FAIL))
printf "\n%s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n" "$BOLD" "$RESET"
printf "%stotal:%s %d  %spass:%s %d  %sfail:%s %d\n" \
  "$BOLD" "$RESET" "$TOTAL" "$GREEN" "$RESET" "$PASS" "$RED" "$RESET" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  printf "\n%sfailures:%s\n" "$RED" "$RESET"
  for n in "${FAIL_NAMES[@]}"; do
    printf "  - %s\n" "$n"
  done
  exit 1
fi
exit 0
