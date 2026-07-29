# B35 design spec: civil `Time` / `Date` / `DateTime`

Design document for batch **B35** in [`../QUEUE.md`](../QUEUE.md). This is a
design, not an implementation: it contains no `.rx` and no Rust. Its job is to
settle the decisions that are expensive to reverse (representation, timezone
scope, rounding and ambiguity policy, the C boundary) before anyone writes code,
because date libraries fail by producing *plausible wrong answers* rather than by
crashing.

**Status:** proposed, not yet implemented. **Ranked P0** in `stdlib.md`: it is
the single biggest stdlib gap. Ruxen today has a monotonic `Instant`/`Duration`
plus a raw `Time.unix_ns` count and no calendar type at all, so a record cannot
carry a `created_at`, an API cannot emit an ISO 8601 field, a log line cannot
carry a timestamp, and a cookie cannot carry an expiry.

Everything below marked **(probed)** was verified on 2026-07-29 by compiling
against the workspace stdlib with
`RUXEN_STDLIB_PATH="$PWD/library/std" ./target/release/ruxen compile`.

---

## 1. What exists today

| Thing | Where | Shape |
|---|---|---|
| `Instant` | `library/std/time/src/lib.rx` | Opaque monotonic sample, all FFI. Not a wall clock, anchor unspecified, only differences are meaningful. |
| `Duration` | same | `nanos: Int` with `layout c`, pure `.rx` accessors (`as_secs`, `as_millis`, …) and FFI constructors and arithmetic. **Non-negative by contract**: `ruxen_duration_sub` saturates at zero. |
| `Time.unix_ns` | same, backed by `runtime/time.c` | `CLOCK_REALTIME` nanoseconds since the Unix epoch, returned as a bare `Int`. Clamps to 0 on `clock_gettime` failure. |
| TestClock seam | `library/std/future/runtime/test_clock.c` | `ruxen_test_clock_now_ns(real)` returns a virtual now when a `TestClock` is installed on the thread. Both the async sleep deadline and sync `Instant` already route through it. |

No `localtime`, `gmtime`, `tm_gmtoff`, `strftime`, or `timezone` appears anywhere
in `library/std/*/runtime/*.c` (grepped). The OS calendar surface is entirely
untouched, so nothing constrains the design.

---

## 2. The `.rx` versus C split

### Decision

**All calendar math lives in `.rx`. The C shim is two functions, one of which
already exists.**

| Concern | Home | Why |
|---|---|---|
| days-from-civil, civil-from-days (Howard Hinnant's algorithms) | **`.rx`** | Pure integer arithmetic. No syscall, no locale, no platform variance. |
| Leap-year, day-of-week, day-of-year, month lengths | **`.rx`** | Same. |
| `strftime` / `strptime` / ISO 8601 / RFC 3339 / RFC 2822 / httpdate | **`.rx`** | String building over `self.chars`, which `String` bodies may do (the pattern the String batches use). Never call C `strftime`: it is locale-sensitive and its `%` coverage varies per platform, which is exactly the silent-wrong-answer surface we are trying to avoid. |
| Arithmetic, comparison, conversions | **`.rx`** | Field reads on a normal user class. |
| **Current wall-clock instant** | **C, already exists** | `ruxen_time_unix_ns` (`clock_gettime(CLOCK_REALTIME)`). No new code. |
| **Local UTC offset for a given instant** | **C, one new function** | Requires the platform tz database. Genuinely inexpressible in `.rx`: it is a query against OS state, not arithmetic. |

### Why the `.rx` half is genuinely possible (probed)

The repo rule is `.rx`-first and a C shim needs justification, so this was
verified rather than assumed. A normal user class with several `Int` fields
supports everything the design needs, and none of the traps that block the
generic-collection and scalar batches apply to it:

- A class with `secs: Int` and `nsec: Int` fields, a `def init(@secs, @nsec)`
  auto-assign constructor, class methods (`def self.x`) doing arithmetic on `Int`
  parameters, field-read arithmetic (`self.secs + n`), and methods returning
  freshly constructed instances all compile and run correctly **(probed)**.
- The "generic collections must not dispatch on `T`" constraint does not apply:
  there is no `T`. The "scalar `self` cannot be the left operand of arithmetic"
  constraint does not apply either: `self.secs` is a plain `Int` field read, not
  a scalar method-home `self`. The existing `Duration.as_micros` body
  (`self.nanos / 1000`) is the same shape and has shipped for some time.

So a C calendar shim would be unjustifiable. It would also be worse: C's
`localtime_r`/`timegm` behaviour around pre-1970 dates and non-POSIX platforms is
a portability liability we would inherit for no gain.

### The one trap that will silently corrupt pre-1970 dates

**Ruxen integer division truncates toward zero, and `%` takes the sign of the
dividend (probed):**

```
-7 / 2       ==  -3      (not -4)
-7 % 2       ==  -1      (not 1)
-719468 / 146097 == -4   (not -5)
-719468 % 146097 == -135080
```

Hinnant's `civil_from_days` and `days_from_civil` are correct only under **floor**
division. Written naively in Ruxen they produce an off-by-one-day error for every
date before 1970-01-01, and no error at all for dates after it, which is the
worst possible failure mode: it passes every casual test.

**Mandatory rule for the implementation:** define `Calendar.fdiv(a, b)` and
`Calendar.fmod(a, b)` as floor-division helpers and route *every* calendar
expression through them. Never write a bare `/` or `%` on a value that can be
negative. A verified floor-division helper shape is:

> quotient `q = a / b`; if `a % b != 0` and the signs of `a` and `b` differ, the
> answer is `q - 1`, else `q`.

Probed: `fdiv(-7, 2) == -4`, `fdiv(-1, 86400) == -1`, `fdiv(86400, 86400) == 1`.

The alternative (the standard C++ formulation that pre-biases the era, `z >= 0 ?
z : z - 146096`) is equally valid. Pick one, state it in the module `CLAUDE.md`,
and use it uniformly. Do not mix the two.

---

## 3. The OS boundary and the portability risk

Exactly two facts come from outside the program.

### 3.1 Current instant

`ruxen_time_unix_ns()` already provides it. `Time.now` must route through the
**existing TestClock seam** (`ruxen_test_clock_now_ns`) the way `Instant.now`
does, so that a test can freeze the clock. Without that, every test that touches
`Time.now` is nondeterministic, and the pinned cases in §10 become unwritable.
This is a small amount of wiring, but it must land in the same sub-batch as
`Time.now`, not later.

Note the existing contract: the C function clamps to `0` on `clock_gettime`
failure rather than returning an error. Keep it. A `Result`-returning `Time.now`
would poison every call site for a syscall with no practical failure mode.

### 3.2 Local UTC offset

**New C function, the only new one:**

```
int64_t ruxen_time_local_offset_secs(int64_t utc_secs)
```

It returns the local zone's offset from UTC, in seconds east of UTC, **for the
instant given**. It must take the instant as an argument rather than returning
"the current offset", because the offset is a function of the instant: any zone
with DST has at least two, and historical offsets differ from present ones.
A parameterless version would be wrong for every timestamp that is not "now",
which is most of them.

Implementation: `localtime_r` into a `struct tm`, read `tm_gmtoff`. Fall back to
`0` (treat as UTC) if the platform cannot answer.

**Portability risk, stated plainly:**

| Platform | Status |
|---|---|
| Linux, macOS, BSD | `tm_gmtoff` is available and correct. This is the supported path. |
| Windows | `tm_gmtoff` does not exist in the MSVC CRT. Needs a separate implementation over `GetTimeZoneInformationForYear`. **Not in v1.** On Windows the shim returns `0`, so local time equals UTC and every UTC-based API stays correct. |
| wasm32 | There is no tz database in the sandbox. The shim returns `0`. Same consequence. |

The consequence of the fallback is bounded and honest: **UTC and fixed-offset
handling is correct everywhere; only the "system local zone" convenience
degrades to UTC on platforms we have not implemented.** That degradation must be
detectable, so also expose a `Time.local_zone_available? -> Bool` (or return the
offset as `Option[Int]` internally) rather than silently pretending the machine
is in UTC. An application that genuinely needs local time can then fail loudly
instead of writing wrong timestamps.

### 3.3 What we deliberately do not do

**Ruxen does not bundle an IANA tz database.** It is roughly a megabyte of data
that changes several times a year through political decisions, and shipping it
means owning a release cadence tied to `tzdata`. Vendoring a stale copy is worse
than not having one. If named-zone support is ever required, the right shape is a
separate optional package that reads the system zoneinfo files, not a core stdlib
dependency. Recorded here so the decision is not silently revisited.

---

## 4. Type design

Three types. The split follows Ruby's, because the goal is Ruby parity and Ruby
programmers expect `Date` to be date-only.

### 4.1 `Time`: an absolute instant plus an offset

```
class Time
  secs:       Int   # seconds since 1970-01-01T00:00:00Z, may be negative
  nsec:       Int   # 0 .. 999_999_999, always non-negative
  utc_offset: Int   # seconds east of UTC, e.g. +19800 for +05:30
end
```

`Time` identifies a point on the timeline (`secs`, `nsec`, always UTC-based) and
*separately* remembers how to render it (`utc_offset`). Two `Time` values with
the same `secs`/`nsec` and different offsets are the **same instant** and compare
equal under `<=>`, and they render differently under `strftime`. This is Ruby's
model and it is the only model that makes DST arithmetic come out right.

**Why not a single nanosecond count.** An `Int` is i64. A nanosecond count
overflows at **2262-04-11T23:47:16Z** and underflows symmetrically at 1677. That
is close enough to be a real bug for date-of-birth fields, mortgage schedules,
and any "far future" sentinel. Splitting into seconds plus nanos moves the limit
to roughly ±292 billion years, which is not a limit anyone will meet. The
existing `Time.unix_ns` keeps its current shape and stays a separate, lower-level
accessor.

**Explicit numbers:**

| Quantity | Limit |
|---|---|
| `secs` range | ±9,223,372,036,854,775,807 (i64). Not the binding constraint. |
| Practical civil range | Years **1 through 9999**, because `%Y` is a four-digit field and formatting outside that is ill-defined. Constructing outside it is allowed for arithmetic; **formatting outside it is an error, not a wrong string.** |
| Precision | Nanosecond. `nsec` is normalised to `0 .. 999_999_999` on every construction and every arithmetic result, so `secs` alone always floors correctly. |
| `utc_offset` range | ±86,399 accepted; real zones span roughly −12:00 to +14:00. Values outside ±25 hours are rejected at construction. |

### 4.2 `Date`: a day number

```
class Date
  days: Int   # days since 1970-01-01, proleptic Gregorian, may be negative
end
```

Not "a `Time` at midnight". A day number cannot represent an impossible date, has
no timezone question, and makes `Date` arithmetic exact by construction. Ruby's
`Date` is a distinct class with distinct semantics and this preserves that. The
calendar is **proleptic Gregorian** for all dates, including before 1582: it does
not switch to Julian. Ruby's `Date` defaults to a reform date of 1582-10-15 and
therefore disagrees with this for dates before then. **This is a deliberate
deviation.** It is stated in the row, and the alternative (implementing the
Julian/Gregorian reform, plus Ruby's `Date::ENGLAND`/`ITALY`/`JULIAN` reform
constants) buys historical-scholarship accuracy that no target application needs
and costs a permanent complexity tax on every conversion.

### 4.3 `DateTime`

Ruby's `DateTime` is a `Date` subclass carrying time-of-day and an offset, and
Ruby's own documentation now steers users to `Time` instead. **Proposal:
`DateTime` is a thin type over the same representation as `Time`**, provided so
that Ruby code that names `DateTime` compiles, with `DateTime.now`, `.parse`,
`.iso8601`, `#to_time`, `#to_date`, and the field accessors delegating to `Time`.

**Open question:** whether `DateTime` should exist at all in v1, or whether
`Time` plus `Date` is the honest surface and `DateTime` should be deferred until
something asks for it. Tradeoff: providing it costs a small amount of delegation
code and some conceptual duplication; omitting it means ported Ruby code that
mentions `DateTime` fails to compile with no substitute named. Recommendation:
provide it, thin, and document that `Time` is preferred.

### 4.4 The `Duration` interaction

`Duration` is non-negative by an existing contract (`ruxen_duration_sub`
saturates at zero). A difference between two `Time` values is frequently
negative, so **`Time#-(Time)` must not return `Duration`**. It returns `Float`
seconds, matching Ruby.

**Dependency to flag:** the returned `Float` is a value, so producing and
comparing it is fine, but calling a method on it (`.abs`, `.round`) hits the
Float receiver-ABI bug tracked as **B01**. Until B01 lands, document that the
result of `Time#-` should be compared or printed, not method-called. This is a
real, current limitation and it should appear in the module `CLAUDE.md`, not just
here.

---

## 5. Timezone scope for v1

### Recommendation

**v1 supports exactly three things: UTC, an arbitrary fixed offset, and the
system local zone resolved per instant through the OS.**

| Supported in v1 | Not in v1 |
|---|---|
| `Time.utc(...)`, `Time.now.utc`, `#utc?`, `#getutc` | Named zones (`"America/New_York"`) in any constructor or conversion |
| Fixed offsets: construct with `+05:30`, parse an ISO 8601 offset, `#getlocal(offset)`, `#new_offset` | Zone abbreviations (`"EST"`, `"PDT"`), which are ambiguous anyway (`"CST"` names at least three zones) |
| System local zone via the OS, correct across DST because the offset is queried per instant | Historical offsets for any zone other than the local one |
| `#utc_offset` / `#gmt_offset` as an integer, `#zone` as `"UTC"` or a rendered numeric offset (`"+05:30"`) | `#zone` as a name, `#dst?` as anything better than "the local offset here differs from the local January offset" |

### What this does and does not buy

It **does** give correct DST behaviour for the machine's own zone, at any
instant, including historical ones, because the platform tz database answers the
question rather than us. A log timestamp, a `created_at` rendered in local time,
and a cookie expiry are all correct.

It **does not** let a server in UTC render a timestamp in a user's zone. That is
the single most common thing full tz support is wanted for, and it is honestly
out of scope for v1. The substitute available on day one is to store the user's
offset (not their zone name) and use `#getlocal(offset)`, which is correct except
across a DST transition for that user.

### Why not full IANA support

It is very large: parsing the binary TZif format, the POSIX TZ string fallback
for dates beyond the last transition, per-zone transition tables, and a release
cadence tied to `tzdata`. It is a package in its own right, not a part of this
batch. Scoping it in would mean B35 never ships, and B35 blocks records, APIs,
and logs.

---

## 6. Leap seconds, DST, and impossible local times

These three are where date libraries produce silent wrong answers, so each gets
an explicit policy rather than an emergent one.

### 6.1 Leap seconds: ignored

A day is exactly 86,400 seconds. `secs` is a POSIX-style count that does not
include leap seconds, so a leap second is not representable and elapsed time
across one is understated by a second.

Justification: this is what POSIX `time_t` does, what Ruby does, what Go, Java's
`Instant`, and Rust's `SystemTime` do, and what every network protocol we
interoperate with assumes. Representing leap seconds correctly requires a leap
table with the same freshness problem as `tzdata`, and it would make `Time`
disagree with the operating system.

Consequence to pin: parsing `"2016-12-31T23:59:60Z"` is an **error**, not a
silent coercion to `23:59:59` or `00:00:00`. Silently coercing is the failure
mode to avoid.

### 6.2 DST: correct for the local zone, by construction

Because `Time` stores an absolute instant and the offset separately, arithmetic
happens on the instant and never on the wall-clock fields. So:

- `t + 86400` advances the instant by exactly 24 hours. Across a spring-forward,
  the local wall-clock reading advances by 25 hours. This matches Ruby, and it is
  the correct answer for "one day of elapsed time".
- "Same wall-clock time tomorrow" is a *different* operation, expressed by
  decomposing to civil fields, adding one day there, and re-resolving the offset.
  Both must be available and they must be named differently. Proposal:
  `#+(seconds)` is the elapsed-time operation and `#next_day` (via `Date`) is the
  civil operation. Document the distinction prominently, because conflating them
  is the classic DST bug.

### 6.3 Ambiguous local times (fall-back): choose the earlier instant

When the clocks go back, a local wall-clock time such as 01:30 occurs twice.
Policy: **resolve to the earlier instant** (the pre-transition offset, that is,
the DST one).

Rationale: it is deterministic, it is the more common convention, and it means a
sequence of civil timestamps constructed across the transition is non-decreasing.

**Open question:** CRuby's `Time.local` behaviour in this case should be verified
against a real interpreter before this is pinned, since matching Ruby exactly is
the batch's goal and the documented behaviour is thin. If CRuby resolves to the
later instant, match CRuby and update this section. Either way the choice must be
explicit in the code and covered by a pinned case, never left to whatever
`mktime` happens to do.

### 6.4 Nonexistent local times (spring-forward gap): shift forward

When the clocks go forward, a local wall-clock time such as 02:30 does not exist.
Policy: **shift forward by the length of the gap**, yielding 03:30 local, which
is the first instant at or after the requested one.

Rationale: it is total (never fails), it is monotonic, and it never silently
moves a timestamp backwards. Returning an error would be defensible too, but it
would make `Time.local` fallible for one hour a year, which poisons every call
site.

Both 6.3 and 6.4 must be pinned with a fixed `TZ` environment variable so the
cases are reproducible on any machine.

---

## 7. Formatting and parsing

### 7.1 `strftime`, v1 directive set

Chosen as the set required by ISO 8601, RFC 3339, RFC 2822, httpdate, and normal
log lines. Implemented in `.rx`, never delegated to C `strftime`.

| Directive | Meaning | Directive | Meaning |
|---|---|---|---|
| `%Y` | 4-digit year | `%H` | hour, 00 to 23 |
| `%C` | century | `%I` | hour, 01 to 12 |
| `%y` | 2-digit year | `%M` | minute |
| `%m` | month, 01 to 12 | `%S` | second |
| `%d` | day, 01 to 31 | `%L` | milliseconds |
| `%e` | day, space-padded | `%N` | fractional seconds, width-aware (`%3N`, `%6N`, `%9N`) |
| `%j` | day of year | `%p` / `%P` | AM/PM, upper and lower |
| `%A` / `%a` | weekday name, full and abbreviated | `%z` | `+hhmm` |
| `%B` / `%b` | month name, full and abbreviated | `%:z` | `+hh:mm` |
| `%u` / `%w` | weekday number, ISO and Sunday-based | `%Z` | zone name (`"UTC"` or the numeric rendering, per §5) |
| `%s` | epoch seconds | `%%` | literal `%` |
| `%F` | `%Y-%m-%d` | `%T` | `%H:%M:%S` |
| `%D` / `%x` | `%m/%d/%y` | `%R` | `%H:%M` |
| `%c` | date and time | `%r` | 12-hour time |

Flags in v1: `-` (no padding), `_` (space padding), `0` (zero padding), `^`
(upcase), and an explicit width (`%3N`, `%10d`).

**Deferred:** `%G`/`%g`/`%V` (ISO week-based year and week number, correct
implementation is fiddly and rarely used), `%U`/`%W` (other week numberings),
`%n`/`%t` (literal newline and tab, trivial to add later), `%+` (date(1)
format), and any locale-dependent behaviour. Month and weekday names are
**English only** in v1, stated as a limitation, because localisation needs a
locale mechanism that does not exist yet.

### 7.2 Canonical emitters, all v1

`#iso8601` / `#xmlschema` (they are the same thing), `#rfc3339`, `#rfc2822`, and
`#httpdate`. The last two are not optional niceties: `net/http` (B45) needs
`httpdate` for `Date`, `Expires`, and `If-Modified-Since` headers, and `logger`
(B41) wants a canonical timestamp. Each is a fixed `strftime` pattern plus the
correct fixed zone (httpdate is always GMT), so they are cheap once `strftime`
exists.

### 7.3 `strptime`, v1 directive set

`%Y %C %y %m %d %e %j %H %I %M %S %L %N %p %P %z %:z %s %A %a %B %b %F %T %D %R
%%`, plus literal text matching and whitespace flexibility. That is enough to
read back everything §7.1 and §7.2 write, which is the minimum bar: **a
round-trip through format and parse must be lossless for every v1 emitter, and
that must be a pinned case.**

Deferred: `%Z` parsing (zone *names* are ambiguous and unresolvable without a tz
database, so accepting them would mean guessing), and the week-based directives.

### 7.4 `Time.parse` / `Date.parse` (the heuristic parsers)

Ruby's `Date._parse` accepts a large, loosely specified family of formats. v1
ships a **documented, bounded** subset: full ISO 8601 (with and without an
offset, with `T` or a space separator), RFC 2822, httpdate, and `YYYY-MM-DD`.
Anything else is an error.

Justification: a heuristic parser that quietly guesses wrong on `03/04/2026` (is
it March 4 or April 3?) is a data-corruption bug. Being narrow and explicit is
better than being Ruby-compatible on ambiguous input. State the accepted grammar
in the module `CLAUDE.md`.

### 7.5 Errors are `Result`, never a panic

Every parser returns `Result[Time, TimeError]` (or `Result[Date, TimeError]`).
Ruby raises `ArgumentError`; the Ruxen idiom for the same distinction is a
`Result`, exactly as `String#parse_int` already does and as `gems-db.md` records
for the ORM's `find`. `TimeError` is an enum with `include Error` and a
`message`, carrying at minimum: `InvalidFormat`, `OutOfRange` (field present but
impossible, such as month 13 or February 30), `Incomplete`, and
`UnsupportedDirective`.

---

## 8. Arithmetic semantics

### 8.1 Exact operations

| Operation | Result | Note |
|---|---|---|
| `Time + Int` / `Time + Float` seconds | `Time` | Fractional seconds honoured to nanosecond precision. Offset preserved. |
| `Time - Int` / `Time - Float` | `Time` | |
| `Time - Time` | `Float` seconds | Ruby parity. May be negative, which is why it is not a `Duration` (§4.4). |
| `Date + Int` / `Date - Int` days | `Date` | Exact. |
| `Date - Date` | `Int` days | **Deviation:** Ruby returns a `Rational`. Ruxen has no `Rational` and `Date` has no time-of-day, so the difference is always a whole number of days and `Int` is the honest type. Record the deviation in the row. |
| `<=>`, `==`, `<`, `>`, `between?` | via `include Comparable` | Compares the instant, not the rendered fields. Two `Time`s with the same instant and different offsets are **equal**. |

### 8.2 Month and year arithmetic: clamping, not associative, not reversible

Adding a month is not a well-defined operation on all dates, and every date
library has to choose. Ruby clamps to the last valid day of the target month, and
Ruxen matches Ruby exactly:

```
2026-01-31 >> 1   ==  2026-02-28      (clamped, not 2026-03-03)
2024-01-31 >> 1   ==  2024-02-29      (leap year)
2026-03-31 >> 1   ==  2026-04-30
2026-01-31 >> 1 >> 1  ==  2026-03-28  (NOT 2026-03-31)
2026-01-31 >> 1 << 1  ==  2026-01-28  (NOT the original date)
```

The consequences are counterintuitive and must be stated in the doc comment, not
just here:

- **Not reversible.** Adding a month then subtracting a month does not return the
  original date.
- **Not associative with day arithmetic.** `(d >> 1) + 1` is not `(d + 1) >> 1`.
- **Not distributive.** `d >> 2` is not `d >> 1 >> 1`. Ruby computes `>> n`
  directly against the target month rather than iterating, so
  `2026-01-31 >> 2 == 2026-03-31` while `2026-01-31 >> 1 >> 1 == 2026-03-28`.
  The implementation must compute in one step, or it will disagree with Ruby.

The same clamping applies to `next_month`, `prev_month`, `next_year`, `prev_year`
(which is `>> 12`), and to `Time`'s equivalents if provided.

Year arithmetic has the same shape with one case: `2024-02-29 >> 12 ==
2025-02-28`.

---

## 9. Method inventory

`v1` means in scope for B35. `defer` means it is recorded as a row and left for a
follow-up. Mapped against CRuby's `Time`, `Date`, and `DateTime`.

### 9.1 `Time` construction

| Ruby | Status | Note |
|---|---|---|
| `Time.now` | v1 | Through the TestClock seam. Local offset attached. |
| `Time.at(secs)`, `Time.at(secs, subsec)` | v1 | |
| `Time.utc(y,m,d,h,mi,s)` / `Time.gm` | v1 | |
| `Time.local(...)` / `Time.mktime` | v1 | Ambiguity and gap policy per §6.3 and §6.4. |
| `Time.new(y,m,d,h,mi,s, offset)` | v1 | Explicit offset accepted as `Int` seconds or `"+05:30"`. |
| `Time.parse`, `Time.iso8601`, `Time.xmlschema` | v1 | Bounded grammar per §7.4. |
| `Time.strptime(str, fmt)` | v1 | |
| `Time.at(secs, in: zone_name)` | defer | Needs named zones. |

### 9.2 `Time` accessors and predicates

| Ruby | Status |
|---|---|
| `#year #month #mon #day #mday #hour #min #sec` | v1 |
| `#usec #nsec #subsec` | v1 (`#subsec` returns `Float`, deviating from Ruby's `Rational`) |
| `#wday #yday` | v1 |
| `#monday?` … `#sunday?` | v1 |
| `#to_i #to_f` | v1 |
| `#to_r` | defer (no `Rational`) |
| `#utc? #utc #getutc #localtime #getlocal(offset) #utc_offset #gmt_offset #zone` | v1 |
| `#dst? #isdst` | v1, weak: true only when the local offset at this instant differs from the local offset in January of the same year. Documented as a heuristic. |
| `#round(n) #ceil(n) #floor(n)` | v1 |
| `#hash #eql?` | v1 |
| `#to_a`, `#gmtime`, `#succ` | defer (deprecated or rare in Ruby) |

### 9.3 `Time` formatting and arithmetic

| Ruby | Status |
|---|---|
| `#strftime(fmt)` | v1, directive set per §7.1 |
| `#iso8601 #xmlschema #rfc3339 #rfc2822 #httpdate` | v1 |
| `#to_s #inspect` | v1 |
| `#+ #- #<=> #== #between? #clamp` | v1 |

### 9.4 `Date`

| Ruby | Status | Note |
|---|---|---|
| `Date.new(y,m,d)` / `Date.civil` | v1 | Proleptic Gregorian, no reform date (§4.2). |
| `Date.today` | v1 | Uses the local offset to decide which day it is. |
| `Date.parse` / `Date.iso8601` / `Date.strptime` | v1 | |
| `Date.jd(n)` | v1 | Julian Day Number conversion is pure arithmetic. |
| `Date.ordinal` / `Date.commercial` | defer | |
| `#year #month #mon #day #mday #wday #yday` | v1 | |
| `#cwday #cweek #cwyear` | defer | ISO week-based, pairs with the deferred `%V`. |
| `#leap?` | v1 | |
| `#julian? #gregorian?` | defer | No reform date, so these are constants. |
| `#+ #- #<< #>> #<=> #==` | v1 | Clamping per §8.2. |
| `#next_day #prev_day #next_month #prev_month #next_year #prev_year #succ #next` | v1 | |
| `#step(limit, step) #upto #downto` | v1 | |
| `#strftime #iso8601 #rfc3339 #httpdate #jd` | v1 | |
| `#mjd #ajd` | defer | |
| `#to_time #to_date #to_datetime` | v1 | `#to_time` is midnight local. |
| `#monday?` … `#sunday?` | v1 | |

### 9.5 `DateTime`

`DateTime.now`, `.new(...)`, `.parse`, `.strptime`, `.iso8601`, the `Time` field
accessors, `#new_offset`, `#to_time`, `#to_date`, and the emitters: all v1 as
thin delegation, subject to the open question in §4.3.

---

## 10. Test strategy

Every case below is a required pin under `tests/release-e2e/cases/`. The values
must be computed independently (from CRuby, or from a published table) and
hardcoded as expected output. **Do not generate the expected output from the
implementation under test**, which is the standard way a calendar bug gets
pinned in place.

### 10.1 The kernel (highest value, cheapest to run)

- **Epoch boundary:** `0` is `1970-01-01T00:00:00Z`, a Thursday. `-1` is
  `1969-12-31T23:59:59Z`. Round-trip both directions.
- **Pre-1970 negative times:** `1969-07-20T20:17:40Z` (Apollo 11), `1900-01-01`,
  `1801-03-01`, `0001-01-01`. This is the family the floor-division trap (§2)
  breaks, so it is the single most important group of pins in the batch. Include
  at least one date where `secs % 86400 != 0` and `secs < 0`, which is where a
  truncating division silently loses a day.
- **Round-trip fuzz-by-table:** a fixed table of roughly 100 dates spanning years
  1 to 9999, each converted civil → days → civil, asserted identical. A fixed
  table rather than a random fuzz, so failures are reproducible.

### 10.2 Leap years

- **2000 is a leap year** (divisible by 400). February has 29 days.
- **1900 is not** (divisible by 100, not by 400). February has 28 days. This is
  the case that a naive `year % 4 == 0` rule gets wrong, and it sits inside the
  range of real birth dates.
- **2024 is** (the ordinary rule). **2026 is not.**
- `2024-12-31` is day-of-year 366; `2026-12-31` is 365.

### 10.3 Month-end rollover

- `2026-01-31 >> 1 == 2026-02-28`
- `2024-01-31 >> 1 == 2024-02-29`
- `2026-01-31 >> 2 == 2026-03-31` **and** `2026-01-31 >> 1 >> 1 == 2026-03-28`,
  asserted in the same case, to pin the non-associativity of §8.2.
- `2026-03-31 << 1 == 2026-02-28`
- `2024-02-29 >> 12 == 2025-02-28`

### 10.4 DST

Run with an explicitly set `TZ` so the case is machine-independent. Use
`America/New_York` for the classic transitions:

- **Spring forward**, 2026-03-08: local 01:59:59 plus one second is local
  03:00:00. The nonexistent local time 02:30 resolves per §6.4.
- **Fall back**, 2026-11-01: local 01:30 is ambiguous and resolves per §6.3.
  Adding 3600 seconds to the first 01:30 gives the second 01:30, with a different
  `utc_offset` and the same rendered wall clock.
- **Elapsed versus civil:** across the spring-forward, `t + 86400` advances the
  local wall clock by 25 hours, while `date.next_day` advances it by 24.
- A **southern-hemisphere** zone (`Australia/Sydney`), so the transition
  direction is not hardcoded by accident.

### 10.5 Year 2038 and other representation limits

- `2038-01-19T03:14:07Z` (`INT32_MAX` seconds) and the second after it both work
  and format correctly. Ruxen uses i64 throughout, so this is a regression guard
  rather than a real limit, and it is worth pinning precisely because it is where
  a stray 32-bit cast in the C shim would show up.
- `2262-04-11T23:47:16Z`, the point where a nanosecond i64 count would overflow,
  works. This pins the §4.1 decision not to use one.
- Formatting a year outside 1 to 9999 is an **error**, not a wrong string.

### 10.6 Format and parse

- **Lossless round-trip** for every v1 emitter: `iso8601`, `rfc3339`, `rfc2822`,
  `httpdate`, and a `strftime`/`strptime` pair, each formatted and parsed back to
  an identical value. Include a value with non-zero nanoseconds and a non-UTC
  offset.
- Known-answer vectors: `httpdate` of a fixed instant equals exactly
  `"Sun, 06 Nov 1994 08:49:37 GMT"` (the RFC 7231 example).
- `"2016-12-31T23:59:60Z"` is an error (§6.1).
- `"2026-02-30"` is `OutOfRange`, not a silent roll into March.
- Every rejected input returns the specific `TimeError` variant, asserted.

---

## 11. Implementation order (sub-batches)

B35 as one batch is too large for a single session, and the sub-batches have a
strict dependency order because everything rests on the kernel being right.
Each is one agent, one session.

| Sub-batch | Goal | Depends on | Risk |
|---|---|---|---|
| **B35a: the calendar kernel** | `Calendar.fdiv`/`fmod`, `days_from_civil`, `civil_from_days`, leap-year, month lengths, day-of-week, day-of-year, and the `Date` type with day arithmetic and comparison. Pure `.rx`, **no OS, no C, no formatting**. Pinned with §10.1 to §10.3. | nothing | Low risk, highest value. Everything else is wrong if this is wrong, and it is fully testable with fixed inputs. |
| **B35b: the `Time` value type** | `Time` with `secs`/`nsec`/`utc_offset`, normalisation, construction from epoch and from civil fields (UTC and explicit-offset forms only), all field accessors, `include Comparable`, seconds arithmetic, `Time`↔`Date` conversion. Still **no OS call**. | B35a | Low. |
| **B35c: the OS boundary** | `Time.now` routed through the existing TestClock seam, the new `ruxen_time_local_offset_secs` C shim, `Time.local`/`#localtime`/`#getlocal`/`#utc`/`#zone`/`#utc_offset`/`#dst?`, and the §6.3/§6.4 ambiguity and gap policies. Pinned with §10.4 under a fixed `TZ`. | B35b | **Highest risk in the batch.** It is the only sub-batch with new C, the only one whose behaviour depends on the host, and the one where the DST policies become real. |
| **B35d: formatting** | `strftime` with the §7.1 directive set, plus `iso8601`, `xmlschema`, `rfc3339`, `rfc2822`, `httpdate`, `to_s`, `inspect`. | B35b (B35c only for `%z`/`%Z` on local times) | Medium. Large surface, but each directive is independently testable. |
| **B35e: parsing** | `strptime` with the §7.3 set, `Time.iso8601`, `Date.iso8601`, the bounded `Time.parse`/`Date.parse` grammar, and the `TimeError` enum. Pinned with the §10.6 round-trips. | B35d (round-trip pins need both) | Medium. |
| **B35f: calendar arithmetic and stepping** | `>>`, `<<`, `next_month`, `prev_month`, `next_year`, `prev_year`, `next_day`, `prev_day`, `succ`, `step`, `upto`, `downto`, the weekday predicates, `Date.jd`, and the `DateTime` delegation layer. Pinned with §10.3. | B35a, B35b | Low, but §8.2's non-associativity must be pinned, not just implemented. |

**Ship order:** B35a, then B35b, then B35d and B35f in parallel (they do not
touch each other), then B35c, then B35e. B35c is deliberately late: it is the
riskiest and the only host-dependent piece, and everything except local-zone
rendering is useful without it.

**Pin blocks.** `QUEUE.md` allocates B35 the block 1340 to 1349, which is not
enough for six sub-batches. Proposed allocation, to be recorded in `QUEUE.md`
when it is next free to edit: **B35a 1900-1909, B35b 1910-1919, B35c 1920-1929,
B35d 1930-1939, B35e 1940-1949, B35f 1950-1959**, with 1340 to 1349 left unused.

---

## 12. Consumers waiting on this

Listed so the sequencing consequences are visible: `logger` (B41) timestamps,
`net/http` (B45) `Date`/`Expires`/`If-Modified-Since` headers, `securerandom`'s
`uuid_v7` (B37), the ORM's `created_at`/`updated_at` and `timestamps` (B60),
rondo's cookie expiry and session lifetime (B66), `Pathname#mtime` and
`FileUtils.uptodate?` (B48), and `fs.metadata` time fields.

Several of those need only `Time.now` plus `iso8601`, which is B35a plus B35b
plus a slice of B35d. Worth knowing when scheduling: the majority of the
downstream value lands before the risky sub-batch.

---

## 13. Open questions

1. **`DateTime` in v1 at all?** (§4.3) Provide it as thin delegation, or omit it
   and direct users to `Time`. Recommendation: provide it. Cost is small,
   omission breaks ported code with no named substitute.
2. **Ambiguous-local-time resolution** (§6.3): verify CRuby's actual `Time.local`
   choice before pinning the "earlier instant" policy, and match CRuby if it
   differs.
3. **Proleptic Gregorian versus Ruby's 1582 reform date** (§4.2). The spec
   chooses proleptic and records the deviation. If historical accuracy before
   1582 is ever needed, this is the decision to revisit, and it is much cheaper
   to revisit before there are stored dates than after.
4. **`#subsec` and `Date#-` return types** (§8.1, §9.2). Ruby returns `Rational`
   for both. Ruxen has no `Rational`, so the spec chooses `Float` and `Int`. If a
   `Rational` type ever lands (it is currently listed as out of scope in
   `language.md`), these rows should be revisited.
5. **Whether `Time.now` should attach the local offset or UTC by default.** Ruby
   attaches local. Matching Ruby means every `Time.now` makes an OS call for the
   offset, and it means `Time.now` behaves differently on Windows and wasm where
   the shim returns 0 (§3.2). The alternative, defaulting to UTC with an explicit
   `.localtime`, is more predictable but is a visible deviation from Ruby.
   Recommendation: match Ruby (attach local), because the deviation would show up
   in every `puts Time.now` a Ruby developer writes.
6. **Where the package lives.** Extend `library/std/time`, or add a sibling
   `library/std/date` and keep `time` as the monotonic primitives? Extending
   `time` keeps one import and matches Ruby's `Time` being core. Recommendation:
   extend `library/std/time`, since `Instant`/`Duration` and civil `Time` are
   already neighbours there and the module `CLAUDE.md` can carry the distinction.

---

*Written 2026-07-29 for QUEUE.md batch B35. Probe findings in §2 (the truncating
division and sign-of-dividend modulo, and the pure-`.rx` viability of a
multi-field class) were established by compiling against the workspace stdlib on
that date. No implementation exists yet.*
