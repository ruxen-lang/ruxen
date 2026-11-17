# library/test_std — test-only stdlib fixtures, never shipped to user programs

## What lives here
- `bootstrap_smoke/` — #06.8 proof-of-life FFI aliases (`bootstrap_smoke_add_one`, `BootstrapSmokeClass`) over `runtime/test_extern.c` (`ruxen_test_extern_*`, also used by the `lib`-alias link smokes).
- `_pin_zero_rust_stdlib/` — trio-leak pin package (`FooBar[T]`, `foobar_double_plus_one`) over `runtime/foobar.c`.
- `stream_res/runtime/stream_res.c` — free-tracked stream resource (`ruxen_stream_res_*`, `ruxen_stream_live_count`) for e2e 434/436.

## Public surface
- None in production. A process opts in by setting `RUXEN_TEST_STDLIB` to this directory: the bootstrap appends `TEST_BOOTSTRAP_FILES` (`compiler/ruxen_core/src/resolve/bootstrap.rs`) and `ruxen compile` links every `<pkg>/runtime/*.c` here.
- Cargo tests pass `test_stdlib_runtime_sources(<this dir>)` as extra runtime sources; `tests/release-e2e/run.sh` opts in per case via `TEST_STDLIB_CASES`.

## Depends on
- `library/std/core` (runtime header, prelude). Loaded after every production package.

## Invariants & gotchas
- Nothing here is embedded in the binary or globbed into `libruxenrt.a`; `bootstrap_prelude_merge::production_stdlib_ships_no_test_fixtures` pins that.
- The REPL links only the production runtime, so e2e cases that opt in are skipped in the REPL sweep.
