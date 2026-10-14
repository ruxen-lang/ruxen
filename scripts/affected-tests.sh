#!/usr/bin/env bash
# Run only the tests a change needs.
#
# Maps the changed paths (`git diff --name-only <base>` plus untracked
# files) to the narrowest test commands that cover them, then runs them.
# The full workspace run and the full release_e2e_smoke sweep stay the
# done gate; this script is for the edit/test loop in between.
#
# Usage:
#   scripts/affected-tests.sh [--print] [<base>]
#
#   <base>    git revision to diff against. Default: HEAD, i.e. staged
#             plus unstaged changes plus untracked files.
#   --print   print the commands instead of running them.
#
# Running stops at the first failing command and exits with its status.
#
# Path → command rules (keep in step with the test layout):
#   compiler/ruxen_core/src/<phase>/**   cargo test -p ruxen_core --lib -- <phase>::
#     lexer/parser                       + syntax_parity, parser_feature_parity
#     formatter                          + syntax_parity, formatter_corpus_roundtrip,
#                                          ruxen_ide format_basics
#     mir/borrow_check/async_lowering/codegen  + drop_fixtures
#     diagnostics/codes.rs, docs/errors  error_code_registry + ruxen_cli explain_long_form
#   compiler/ruxen_core/tests/<t>.rs     cargo test -p ruxen_core --test <t>
#   <crate>/tests/fixtures/**            the integration tests that name the fixture
#   library/std/<pkg>/src/*.rx           stdlib_<pkg>*.rs tests + e2e cases named *_<pkg>*
#   library/std/<pkg>/runtime/**         the line above + runtime_abi_derivation, runtime_safety
#   src/<crate>/**                       cargo test -p <crate> (ruxen_lsp adds ruxen_ide)
#   tests/release-e2e/{cases,expected}   RUXEN_E2E_CASES=<stems> release_e2e_smoke
#   tests/release-e2e/run.sh, scripts/*  bash -n
#   Cargo.toml, Cargo.lock, build.rs,    cargo test --workspace (anything the
#   .cargo/**, unknown paths               rules cannot place falls back to this)
#   *.md, docs/** (not docs/errors)      nothing
#
# E2E fixture choice: a compiler change runs the fixtures added or edited in
# the same diff. The script cannot tell which fixtures exercise an edited
# construct, so when the diff has none it prints a reminder instead of
# running the full sweep; pass RUXEN_E2E_CASES yourself in that case.

set -uo pipefail

usage() {
  sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
}

PRINT=0
BASE=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --print) PRINT=1 ;;
    -h|--help) usage; exit 0 ;;
    -*) printf 'affected-tests: unknown flag %s\n' "$1" >&2; exit 2 ;;
    *)
      if [ -n "$BASE" ]; then
        printf 'affected-tests: only one <base> allowed\n' >&2; exit 2
      fi
      BASE="$1" ;;
  esac
  shift
done

ROOT="$(git rev-parse --show-toplevel)" || exit 2
cd "$ROOT" || exit 2

DIFF="$(git diff --name-only "${BASE:-HEAD}" --)" || {
  printf 'affected-tests: git diff against %s failed\n' "${BASE:-HEAD}" >&2
  exit 2
}
CHANGED="$(
  { printf '%s\n' "$DIFF"; git ls-files --others --exclude-standard; } | sort -u
)"

if [ -z "$CHANGED" ]; then
  echo "# no changes against ${BASE:-HEAD}; nothing to run"
  exit 0
fi

# Accumulators: newline-separated, de-duplicated at the end. Plain strings
# instead of associative arrays so the script runs on macOS's bash 3.2.
FULL=0
CORE_LIB=""      # ruxen_core --lib filters (module paths)
CRATE_TESTS=""   # "<crate> <test>" pairs
CRATES=""        # whole crates
E2E=""           # release_e2e_smoke case stems
FULL_SMOKE=0     # the smoke harness itself changed: run every case
SHELL_CHECK=""   # scripts to syntax-check
NOTES=""
CORE_SRC_CHANGED=0

add()  { eval "$1=\"\${$1}\${2}
\""; }
note() { add NOTES "# $1"; }

core_test() { add CRATE_TESTS "ruxen_core $1"; }

# Integration tests in <crate_dir>/tests that mention a fixture by name.
tests_naming() {
  local crate="$1" dir="$2" needle="$3" t
  for t in $(grep -l -F -- "$needle" "$dir"/tests/*.rs 2>/dev/null || true); do
    add CRATE_TESTS "$crate $(basename "$t" .rs)"
  done
}

# e2e cases whose stem names a stdlib package (`530_fs_create`, `412_future`).
e2e_named_after_pkg() {
  local pkg="$1" f stem
  for f in tests/release-e2e/cases/*.rx; do
    stem="$(basename "$f" .rx)"
    case "_${stem#*_}_" in
      *"_${pkg}_"*) add E2E "$stem" ;;
    esac
  done
}

stdlib_pkg_tests() {
  local pkg="$1" t
  for t in compiler/ruxen_core/tests/stdlib_"$pkg".rs compiler/ruxen_core/tests/stdlib_"$pkg"_*.rs; do
    [ -f "$t" ] && core_test "$(basename "$t" .rs)"
  done
  e2e_named_after_pkg "$pkg"
}

crate_of_dir() {
  case "$1" in
    compiler/ruxen_core) echo ruxen_core ;;
    compiler/ruxen_driver) echo ruxen_driver ;;
    src/*) basename "$1" ;;
  esac
}

while IFS= read -r path; do
  [ -z "$path" ] && continue
  case "$path" in
    # ── workspace-wide inputs ──────────────────────────────────────
    Cargo.toml|Cargo.lock|.cargo/*|rust-toolchain*|*/build.rs|*/Cargo.toml)
      FULL=1; note "$path changes the build of every crate" ;;

    # ── docs / prose ───────────────────────────────────────────────
    docs/errors/*)
      core_test error_code_registry; add CRATE_TESTS "ruxen_cli explain_long_form" ;;
    *.md|docs/*|LICENSE*|.gitignore) : ;;

    # ── shell scripts ──────────────────────────────────────────────
    *.sh) add SHELL_CHECK "$path" ;;
    .github/*) note "$path is CI config; nothing local to run" ;;

    # ── ruxen_core sources ─────────────────────────────────────────
    compiler/ruxen_core/src/diagnostics/codes.rs)
      CORE_SRC_CHANGED=1
      add CORE_LIB "diagnostics::"; core_test error_code_registry
      add CRATE_TESTS "ruxen_cli explain_long_form" ;;
    compiler/ruxen_core/src/lexer/*|compiler/ruxen_core/src/parser/*)
      CORE_SRC_CHANGED=1
      phase="${path#compiler/ruxen_core/src/}"; phase="${phase%%/*}"
      add CORE_LIB "$phase::"
      core_test syntax_parity; core_test parser_feature_parity ;;
    compiler/ruxen_core/src/formatter/*)
      CORE_SRC_CHANGED=1
      add CORE_LIB "formatter::"
      core_test syntax_parity; core_test formatter_corpus_roundtrip
      add CRATE_TESTS "ruxen_ide format_basics" ;;
    compiler/ruxen_core/src/mir/*|compiler/ruxen_core/src/borrow_check/*|\
    compiler/ruxen_core/src/async_lowering/*|compiler/ruxen_core/src/codegen/*)
      CORE_SRC_CHANGED=1
      phase="${path#compiler/ruxen_core/src/}"; phase="${phase%%/*}"
      add CORE_LIB "$phase::"; core_test drop_fixtures ;;
    compiler/ruxen_core/src/*/*)
      CORE_SRC_CHANGED=1
      phase="${path#compiler/ruxen_core/src/}"; phase="${phase%%/*}"
      add CORE_LIB "$phase::" ;;
    compiler/ruxen_core/src/*.rs)
      CORE_SRC_CHANGED=1
      if [ "$path" = compiler/ruxen_core/src/lib.rs ]; then
        add CRATES ruxen_core
      else
        mod="$(basename "$path" .rs)"; add CORE_LIB "$mod::"
      fi ;;

    # ── integration tests and their fixtures ───────────────────────
    compiler/*/tests/*/*|src/*/tests/*/*)
      dir="${path%%/tests/*}"; crate="$(crate_of_dir "$dir")"
      sub="${path#"$dir"/tests/}"
      case "$sub" in
        common/*) add CRATES "$crate" ;;   # shared helper: every test may use it
        *) before="$(printf '%s' "$CRATE_TESTS" | wc -l)"
           tests_naming "$crate" "$dir" "$(basename "$path")"
           after="$(printf '%s' "$CRATE_TESTS" | wc -l)"
           # No test names the file directly (a whole fixture dir is read):
           # fall back to the crate's tests.
           [ "$before" = "$after" ] && add CRATES "$crate" ;;
      esac ;;
    compiler/*/tests/*.rs|src/*/tests/*.rs)
      dir="${path%%/tests/*}"
      [ -f "$path" ] || { note "$path was deleted"; continue; }
      add CRATE_TESTS "$(crate_of_dir "$dir") $(basename "$path" .rs)" ;;

    # ── other crates ───────────────────────────────────────────────
    compiler/ruxen_driver/*) add CRATES ruxen_driver ;;
    src/ruxen_lsp/*) add CRATES ruxen_lsp; add CRATES ruxen_ide ;;
    src/ruxen_ide/*|src/ruxen_cli/*|src/ruxenc/*|src/ruxen_repl/*)
      crate="${path#src/}"; add CRATES "${crate%%/*}" ;;

    # ── stdlib ─────────────────────────────────────────────────────
    library/std/*/runtime/*)
      pkg="${path#library/std/}"; pkg="${pkg%%/*}"
      stdlib_pkg_tests "$pkg"
      core_test runtime_abi_derivation; core_test runtime_safety ;;
    library/std/*)
      pkg="${path#library/std/}"; pkg="${pkg%%/*}"
      stdlib_pkg_tests "$pkg" ;;
    library/test_std/*)
      note "library/test_std changed: running the fixtures run.sh lists in TEST_STDLIB_CASES"
      add E2E 434_stream_drop_on_cancel; add E2E 436_stream_chain_drop
      add E2E 750_foobar_zero_rust_pin ;;

    # ── e2e fixtures and harness ───────────────────────────────────
    tests/release-e2e/cases/*.rx|tests/release-e2e/expected/*.out)
      stem="$(basename "$path")"; stem="${stem%.*}"
      if [ -f "tests/release-e2e/cases/$stem.rx" ]; then
        add E2E "$stem"
      else
        note "$path has no case file (deleted fixture)"
      fi ;;
    tests/release-e2e/*) note "$path is harness data; run tests/release-e2e/run.sh by hand if it matters" ;;

    examples/*) core_test parser_feature_parity; core_test syntax_parity ;;

    *) FULL=1; note "$path matches no rule" ;;
  esac
done <<< "$CHANGED"

uniq_lines() { printf '%s' "$1" | sed '/^$/d' | sort -u; }

# ── build the command list ─────────────────────────────────────────
CMDS=()

while IFS= read -r s; do
  [ -n "$s" ] && CMDS+=("bash -n $s")
done <<< "$(uniq_lines "$SHELL_CHECK")"

if [ "$FULL" = 1 ]; then
  CMDS+=("cargo test --workspace --no-fail-fast")
else
  crates="$(uniq_lines "$CRATES")"
  in_crates() { printf '%s\n' "$crates" | grep -qx -- "$1"; }

  lib_filters="$(uniq_lines "$CORE_LIB" | tr '\n' ' ' | sed 's/ $//')"
  if [ -n "$lib_filters" ] && ! in_crates ruxen_core; then
    CMDS+=("cargo test -p ruxen_core --lib -- $lib_filters")
  fi

  # One cargo invocation per crate with every --test it needs.
  pairs="$(uniq_lines "$CRATE_TESTS")"
  for crate in $(printf '%s\n' "$pairs" | awk 'NF {print $1}' | sort -u); do
    in_crates "$crate" && continue
    args=""
    for t in $(printf '%s\n' "$pairs" | awk -v c="$crate" '$1 == c {print $2}'); do
      # The smoke test is #[ignore]-gated; a plain --test would skip it.
      [ "$t" = release_e2e_smoke ] && { FULL_SMOKE=1; continue; }
      args="$args --test $t"
    done
    [ -n "$args" ] && CMDS+=("cargo test -p $crate$args")
  done

  while IFS= read -r crate; do
    [ -n "$crate" ] && CMDS+=("cargo test -p $crate")
  done <<< "$crates"
fi

e2e="$(uniq_lines "$E2E" | tr '\n' ',' | sed 's/,$//')"
if [ "$FULL_SMOKE" = 1 ]; then
  CMDS+=("cargo test -p ruxen_core --test release_e2e_smoke -- --ignored")
elif [ -n "$e2e" ]; then
  CMDS+=("RUXEN_E2E_CASES=$e2e cargo test -p ruxen_core --test release_e2e_smoke -- --ignored")
elif [ "$CORE_SRC_CHANGED" = 1 ]; then
  note "compiler source changed but the diff has no e2e fixture: run"
  note "  RUXEN_E2E_CASES=<stems using the construct> cargo test -p ruxen_core --test release_e2e_smoke -- --ignored"
fi

# ── print or run ───────────────────────────────────────────────────
notes="$(uniq_lines "$NOTES")"
if [ "$PRINT" = 1 ]; then
  [ -n "$notes" ] && printf '%s\n' "$notes"
  [ "${#CMDS[@]}" -gt 0 ] && printf '%s\n' "${CMDS[@]}"
  exit 0
fi

[ -n "$notes" ] && printf '%s\n' "$notes" >&2
if [ "${#CMDS[@]}" -eq 0 ]; then
  echo "# nothing to run for this diff" >&2
  exit 0
fi
for c in "${CMDS[@]}"; do
  printf '\n$ %s\n' "$c" >&2
  bash -c "$c"
  rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'affected-tests: failed (exit %s): %s\n' "$rc" "$c" >&2
    exit "$rc"
  fi
done
