# Spec — `std.time`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.5](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_5_phase2_sync_io_completeness.md](../../prompts/v1/06_5_phase2_sync_io_completeness.md).

**Status:** shipped Phase 3 (`unix_ns`) and Phase 2 #06.5 T4
(`Duration` / `Instant` typed surface).  The flat `now_ns` free-fn
previously offered as a shortcut was removed in #06.5 T5.5 once
`Instant.now` / `Instant.elapsed` covered every use case.

`std.time` provides:

- **Wall-clock timestamps** via `unix_ns() -> Int` (nanoseconds since
  the Unix epoch).
- **Monotonic measurement** via the typed `Instant` + `Duration`
  classes — see [`std.time.{Instant, Duration}`](#instant--duration).

---

## B2 — `unix_ns() -> Int` returns wall-clock nanoseconds since 1970

**Given** the program calls `unix_ns()` on a system with a correctly-
set clock at any post-2020 moment
**Then** the returned value is `> 1_577_836_800_000_000_000`
(2020-01-01T00:00:00Z in nanoseconds).

May jump backwards / forwards under NTP correction — not monotonic.
Suitable for timestamps, not for measuring elapsed time.  A future
`SystemTime` class will wrap this; until then `unix_ns` stays exposed
as a bare `Int`-returning free-fn.

---

## Instant / Duration

The monotonic-clock surface is now class-based and lives in the same
`std.time` module:

```riven
use std.time.{Instant, Duration}
use std.thread.sleep

let start = Instant.now
sleep(Duration.from_millis(50))
let elapsed = start.elapsed   # Duration
puts "took #{elapsed.as_millis} ms"
```

Full behavioural spec: see `Instant` / `Duration` sections in this
same file (added by #06.5 T4).  This block exists so callers
searching for "monotonic" / "now_ns" land at the right import.

---

## Pin tests

| Behaviour | Test fn                                | File             |
|-----------|----------------------------------------|------------------|
| B2        | `time_unix_ns_is_post_2020`            | `stdlib_time.rs` |
| Instant   | `instant_monotonic_after_sleep`        | `stdlib_time.rs` |
| Instant   | `instant_elapsed_non_negative`         | `stdlib_time.rs` |
| Instant   | `instant_duration_since_returns_delta` | `stdlib_time.rs` |
| Instant   | `instant_duration_since_future_panics` | `stdlib_time.rs` |
| Duration  | `duration_from_secs_as_millis`         | `stdlib_time.rs` |
| Duration  | `duration_from_millis_as_secs_floors`  | `stdlib_time.rs` |
| Duration  | `duration_from_as_round_trip_matrix`   | `stdlib_time.rs` |
| Duration  | `duration_add_via_binop_and_named`     | `stdlib_time.rs` |
| Duration  | `duration_sub_saturates_to_zero`       | `stdlib_time.rs` |
| sleep     | `sleep_duration_elapses_in_tolerance_band` | `stdlib_time.rs` |

---

## Removed in #06.5 T5.5

- `now_ns() -> Int` free-fn — superseded by `Instant.now` +
  `Instant.elapsed` / `Instant.duration_since`.  The C symbol
  `riven_time_now_ns` is still linked (it is the implementation used
  internally by `riven_instant_now`) but is no longer reachable from
  Riven user code.

## Out of scope (v2)

- `SystemTime` class wrapper around `unix_ns` (calendar arithmetic,
  serialisation).
- Calendar / timezone primitives (`time.Date`, `time.DateTime`).
- Higher-resolution counters (`rdtsc`) or platform-specific clocks.
