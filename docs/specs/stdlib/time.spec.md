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

```ruxen
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

All in `library/std/time/tests/time_test.rx` (run `scripts/test-stdlib.sh time`).

| Behaviour | Test case                                             |
|-----------|-------------------------------------------------------|
| B2        | `reads unix_ns after 2020`                            |
| Instant   | `never goes backwards across a sleep`                 |
| Instant   | `reports a non-negative elapsed`                      |
| Instant   | `panics when duration_since gets a later instant`     |
| Duration  | `converts from_secs to every unit`                    |
| Duration  | `floors as_secs`                                      |
| Duration  | `round-trips every from_* and as_* pair`              |
| Duration  | `reads a sub-second value through every accessor`     |
| Duration  | `adds through the operator and add`                   |
| Duration  | `saturates subtraction at zero`                       |
| sleep     | `sleeps for about the requested time`                 |

---

## Removed in #06.5 T5.5

- `now_ns() -> Int` free-fn — superseded by `Instant.now` +
  `Instant.elapsed` / `Instant.duration_since`.  The C symbol
  `ruxen_time_now_ns` is still linked (it is the implementation used
  internally by `ruxen_instant_now`) but is no longer reachable from
  Ruxen user code.

## Out of scope (v2)

- `SystemTime` class wrapper around `unix_ns` (calendar arithmetic,
  serialisation).
- Calendar / timezone primitives (`time.Date`, `time.DateTime`).
- Higher-resolution counters (`rdtsc`) or platform-specific clocks.
