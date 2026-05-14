# Spec — `std.time`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.5](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 3 (timestamps).

`std.time` provides two clock primitives.  All values are
nanoseconds expressed as `Int`.

---

## B1 — `now_ns() -> Int` is monotonic

`now_ns()` returns nanoseconds from a monotonic clock that never
moves backwards.

**Given** the program calls `now_ns()` twice as `a` then `b`
**Then** `a > 0` and `b >= a`.

The clock's absolute origin is unspecified — only differences are
meaningful.  Suitable for measuring elapsed time, not for timestamps.

## B2 — `unix_ns() -> Int` returns wall-clock nanoseconds since 1970

**Given** the program calls `unix_ns()` on a system with a correctly-
set clock at any post-2020 moment
**Then** the returned value is `> 1_577_836_800_000_000_000`
(2020-01-01T00:00:00Z in nanoseconds).

May jump backwards / forwards under NTP correction — not monotonic.
Suitable for timestamps, not for measuring elapsed time.

---

## Pin tests

| Behaviour | Test fn                     | File             |
|-----------|-----------------------------|------------------|
| B1        | `time_now_ns_is_monotonic`  | `stdlib_time.rs` |
| B2        | `time_unix_ns_is_post_2020` | `stdlib_time.rs` |

---

## Out of scope (v2)

- `Duration` / `Instant` typed values; v1 uses raw `Int`
  nanoseconds.
- Calendar arithmetic (`time.Date`, `time.DateTime`, timezones).
- Sleeping primitives — `Thread.sleep` is in `std.sync`, not here.
- Higher-resolution counters (`rdtsc`) or platform-specific clocks.
