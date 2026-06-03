# Phase 5 — Data-driven method resolution: the resolver pipeline (`typeck/method_resolvers`)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`)
> syntax for tracking. This is Phase 5 of `2026-06-04-thermonuke-master.md`.

**Goal:** Convert the ~1,090-arm `match (ty, method)` in `typeck/method_resolvers/mod.rs` (1,212 LOC,
290 receiver-headed arms, 112 `if name == "…"` named-class guards across 28 stdlib types) into an
ORDERED resolver pipeline — `declared-method → named-stdlib-resolver → structural-builtin` — exactly
the design the file's own header comment (lines 7–12) already specs (`resolvers() -> Vec<MethodResolver>`,
"the internal arm groupings below already mark the cut lines"). Precedence becomes ONE decision (the
order of the resolver `Vec`) instead of being smeared across 1,090 arm positions. This also closes
latent **bug A2**: a *user* class literally named like a stdlib type (`Mutex`/`File`/`BufReader`/…) that
declares its own `new -> Result[…]` is currently shadowed by the guarded builtin arms (e.g. `(Ty::Class
{ name, .. }, "new") if name == "Mutex"` at line 335 fires *before* the declared-method lookup the
current branch diff added at line 1173), so the user's declared return type is silently overridden.

**Architecture:** One ordered dispatcher walking a `&[MethodResolver]`. The first resolver whose
`matches(ty, method)` returns true *and* whose `resolve(...)` returns `Some` wins; ties never arise
because order is total. Three tiers, in precedence order:

1. **declared-method** — `eng.lookup_class_method_return(name, method)` for `Class`/`Struct`/`Enum`
   receivers. A user-declared method ALWAYS beats a same-named builtin (fixes A2). This is the
   generalization of the one-off ladder the current branch diff already inserted at `mod.rs:1173` for
   `(Ty::Class, "new")`.
2. **named-stdlib-resolver** — the 28 guarded stdlib types, grouped by namespace into small per-file
   tables (`concurrency.rs`, `io.rs`, `fs.rs`, `process.rs`, `time.rs`, `strings.rs`, `collections.rs`,
   `numeric.rs`, `fmt.rs`). Each contributes a `pub fn resolvers() -> Vec<MethodResolver>`.
3. **structural-builtin** — the type-shape arms with no name guard (`Ty::String`/`Str`/`Int`/`Bool`/
   `Array`/`Option`/`Result`/`Map`/`Set`/`Tuple` methods, plus the generic `Class/Struct/Enum`
   `to_s`/`clone`/`default`/`new` fallbacks). These are precedence-LAST so a named resolver wins, but
   the *declared* tier (tier 1) still beats them — preserving the current behaviour where
   `(…, "new") if name == "Mutex"` runs before the generic `(Ty::Class, "new")` at line 1173.

The single hard invariant: **the pipeline must reproduce the existing 1,090-arm match's answer for
every `(receiver_ty, method, args)` triple, arm-for-arm.** Task 1 captures that as a GOLDEN corpus
from the CURRENT code; every migration task re-asserts golden parity. The ONLY intended behaviour change
is bug A2, and that change is itself pinned by a dedicated test (Task 11) that the golden corpus is
explicitly *not* allowed to contradict (the corpus uses stdlib-named-but-not-user-declared receivers).

**Tech Stack:** Rust 1.91, `cargo test -p ruxen_core`. No new dependencies. (`MethodResolver` is a
plain struct with two function-pointer/closure fields — no crate needed; the std-only cost is zero.)

> **Per-task direction-check (maintainer-mandated):** after the commit step of EVERY task below, run the
> `thermonuke` skill scoped to that task's diff: invoke it with arg `git diff HEAD~1..HEAD` and confirm
> (a) net lines moved in the intended direction (arms LEAVING `mod.rs`, landing in a per-namespace table —
> not duplicated into both), (b) **no new `_ =>` catch-all** introduced in `mod.rs` (the single existing
> `_ => None` at line 1210 becomes the dispatcher's "no resolver matched → None" and must not be
> reintroduced inside any per-namespace `resolvers()`), (c) no new god-function in the dispatcher
> (it stays ~150 lines), (d) the task's structural goal was met (a named-class arm family was MOVED to a
> table, not copied). If it flags drift, STOP and surface it. The full multi-agent sweep runs in Task 12.
> Each task's checkbox list ends with a `- [ ] Direction-check (/thermonuke on this task's diff)` step.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `compiler/ruxen_core/src/typeck/method_resolvers/mod.rs` | The `MethodResolver` struct, the ordered dispatcher (`builtin_method_type` entry, unchanged signature), and the `resolvers()` assembly that concatenates the tiers in precedence order | Modify (gut the giant match → dispatcher) |
| `compiler/ruxen_core/src/typeck/method_resolvers/resolver.rs` | `pub struct MethodResolver { matches, resolve }` + `Resolved` helper + the declared-method (tier 1) and structural-builtin (tier 3) resolvers | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/concurrency.rs` | `Mutex`/`MutexGuard`/`Arc`/`SharedSync`/`Thread`/`ThreadPanic`/`JoinHandle`/`Future` arms (mod.rs:229–432) — incl. the E1101/E1102 Send-bound checks | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/fmt.rs` | `Formatter` arms (mod.rs:443–470) | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/io.rs` | `Stdin`/`Stdout`/`Stderr`/`BufReader`/`BufWriter`/`IoError` arms (mod.rs:588–642, 957–1093) incl. the `is_bufio_inner_supported` E0714 guard | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/fs.rs` | `Metadata`/`File`/`OpenOptions` arms (mod.rs:648–800) | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/process.rs` | `Command`/`Output`/`ExitStatus` arms (mod.rs:659–687) | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/net.rs` | `TcpListener`/`TcpStream` arms (mod.rs:853–940) | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/time.rs` | `Duration`/`Instant` arms (mod.rs:809–845) | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/strings.rs` | `Ty::String`/`Ty::Str` structural arms (mod.rs:30–~110) + `SplitIter` (1098) + `ParseIntError`/`ParseFloatError` (112) | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/collections.rs` | `Ty::Array`/`Ty::Option`/`Ty::Result`/`Ty::Map`/`Ty::Set`/`Ty::Tuple` structural arms | **Create** |
| `compiler/ruxen_core/src/typeck/method_resolvers/numeric.rs` | `Ty::Int*`/`UInt*`/`USize`/`ISize`/`Float*`/`Bool`/`Char`/`Unit` structural arms | **Create** |
| `compiler/ruxen_core/tests/method_resolver_golden.rs` | Golden parity corpus + the bug-A2 fix assertion | **Create** |

**Module placement rationale:** the header comment (lines 7–12) already names the intended cut files
and asserts "the internal arm groupings below already mark the cut lines." We follow that, grouping by
the namespace boundaries the guarded class names already form (verified by line-range survey:
concurrency 229–432, fmt 443–470, io/fs/process 588–800 + 957–1093, net/time/strings 809–1098). The
dispatcher and the two tier-spanning resolvers (declared, structural-fallback) live in `resolver.rs`
next to `mod.rs` because they are not namespace-specific.

> **Naming caution (verified against the repo):** the directory `method_resolvers/` is a sibling of
> `typeck/infer/`. `builtin_method_type` is re-exported and called from `infer/collect.rs:326-334`
> (a thin `pub(super)` wrapper) and reached from `infer/expr.rs:482`. **Its signature
> `(eng, ty, method, args, span) -> Option<Ty>` MUST NOT change** — all callers stay byte-identical.
> The pipeline is entirely internal to this function.

---

## The `MethodResolver` design (load-bearing — read before Task 2)

The current arms come in two shapes the struct must accommodate:

- **Pure shape arms** (majority): `(Ty::String, "len") => Some(Ty::USize)` — depend only on `(ty, method)`.
- **Effectful arms** (~20): need `eng` (fresh type vars, `eng.ctx.resolve`, symbol lookups), `args`
  (e.g. `Mutex.new` infers payload from `args[0]`), `span` (diagnostics), and some PUSH a diagnostic
  + `return Some(Ty::Error)` (the 5 early-returns: `BufReader/BufWriter` E0714, `Mutex/SharedSync` E1101).

A single closure shape covers both. `matches` is a cheap pre-filter (so the corpus/debug can see *which*
resolver claimed a triple); `resolve` does the work and may still return `None` (a resolver can match the
receiver namespace but not the method, falling through to the next):

```rust
// resolver.rs
use crate::diagnostics::Diagnostic;
use crate::hir::nodes::HirExpr;
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::typeck::infer::InferenceEngine;

/// One stage of the ordered method-resolution pipeline. The dispatcher walks
/// `resolvers()` in order; the FIRST resolver whose `resolve` returns `Some`
/// wins. `matches` is a cheap structural pre-filter used both to skip work and
/// (in the golden test) to label which resolver claimed a triple. Precedence is
/// the Vec order — there is exactly one decision, not 1,090 arm positions.
pub(super) struct MethodResolver {
    /// Cheap pre-filter on receiver+method. MUST be side-effect free.
    pub matches: fn(&Ty, &str) -> bool,
    /// The actual return-type computation. May read/mutate `eng`, inspect
    /// `args`, push diagnostics keyed at `span`, and may return `None` to fall
    /// through to the next resolver even when `matches` was true.
    pub resolve: fn(&mut InferenceEngine<'_>, &Ty, &str, &[HirExpr], &Span) -> Option<Ty>,
}
```

> **Implementer note (function pointers vs boxed closures):** every existing arm is a free computation
> with no captured environment, so `fn(...)` pointers suffice — no `Box<dyn Fn>`, no allocation, the
> `resolvers()` Vec is built once per call (cheap; the alternative `OnceLock<Vec<…>>` is a YAGNI
> optimization — do NOT add it until a profile shows this path is hot). If the implementer finds a
> single arm that genuinely needs to capture state, STOP and surface it: it would mean a `Box<dyn Fn>`
> field and the cost trade-off must be a maintainer decision, not a silent one.

### The dispatcher (target ~150 LOC, lives in `mod.rs`)

```rust
// mod.rs — replaces the giant match. Signature UNCHANGED (callers in
// infer/collect.rs:333 and infer/expr.rs:482 stay byte-identical).
pub(super) fn builtin_method_type(
    eng: &mut InferenceEngine<'_>,
    ty: &Ty,
    method: &str,
    args: &[HirExpr],
    span: &Span,
) -> Option<Ty> {
    for r in resolvers() {
        if (r.matches)(ty, method) {
            if let Some(ret) = (r.resolve)(eng, ty, method, args, span) {
                return Some(ret);
            }
        }
    }
    None // the single, deliberate "nothing claimed it" — was line 1210's `_ => None`.
}

/// The ONE precedence decision. Tier 1 (declared) before Tier 2 (named stdlib)
/// before Tier 3 (structural). Within a tier, namespace tables are concatenated;
/// no two resolvers in different namespaces can match the same (ty, method) pair
/// because the name guard partitions them (asserted by the golden test).
fn resolvers() -> Vec<MethodResolver> {
    let mut v = Vec::new();
    v.extend(resolver::declared_method_resolvers());   // TIER 1 — fixes A2
    v.extend(concurrency::resolvers());                 // TIER 2 — named stdlib
    v.extend(fmt::resolvers());
    v.extend(io::resolvers());
    v.extend(fs::resolvers());
    v.extend(process::resolvers());
    v.extend(net::resolvers());
    v.extend(time::resolvers());
    v.extend(strings::resolvers());                     // TIER 3 — structural
    v.extend(collections::resolvers());
    v.extend(numeric::resolvers());
    v.extend(resolver::structural_fallback_resolvers()); // generic Class/Struct/Enum to_s/clone/default/new
    v
}
```

> **CRITICAL precedence subtlety (bug A2):** the CURRENT match has `(Ty::Class, "new") if name ==
> "Mutex"` (335) firing BEFORE the generic `(Ty::Class, "new")` (1173) which *itself* now calls
> `lookup_class_method_return`. So a *user* class named `Mutex` with a declared `new` is shadowed by the
> stdlib Mutex arm — that is A2. The fix: tier 1 (declared) runs FIRST. But this is ONLY correct if the
> stdlib types (`Mutex` et al.) do NOT also have a same-named *declared* method in the symbol table.
> **Implementer obligation:** Task 2 must verify (a test) that `lookup_class_method_return("Mutex",
> "new")` returns `None` for the real stdlib `Mutex` (its `new` is a builtin arm, not a symbol-table
> `DefKind::Method`). If it does NOT return `None`, tier-1-first would change stdlib behaviour and the
> precedence must instead be: declared-method **scoped to user-defined types only** (a resolver that
> matches only when the receiver name is NOT in the stdlib namespace set). Decide this empirically in
> Task 2, record the decision in the commit message, and pin it with the Task 11 A2 test + the golden
> corpus (which uses stdlib receivers and must stay green).

---

## Task 1: Golden parity corpus — capture CURRENT behaviour as the migration oracle

**Files:**
- Create: `compiler/ruxen_core/tests/method_resolver_golden.rs`

This task does NOT touch `mod.rs`. It builds the safety net the entire phase asserts against. It is the
risk-register mitigation for "Method-resolver pipeline reorders precedence vs the 1,090-arm match"
(master plan risk row, Phase 5).

- [ ] **Step 1: Enumerate the corpus from the real arms**

The corpus is a `&[(Ty, &str, &str /* expected debug of Option<Ty> */)]` covering, at minimum, ONE triple
per distinct `(Ty-head, method)` arm in `mod.rs`. Generate it mechanically, not by hand:

```bash
# List every arm's (receiver, method) to seed the corpus — implementer transcribes
# the receiver Ty constructor + method string for each into the corpus array.
grep -nE '^\s*\(Ty::' compiler/ruxen_core/src/typeck/method_resolvers/mod.rs \
  | tee tmp/test-cache/phase5-task1-arm-inventory.txt
```

> **Implementer obligation (explicit sub-step, not a placeholder):** the corpus array is ~290 entries —
> one per arm line in the inventory above. For each, write the receiver `Ty` (e.g. `Ty::String`,
> `class("Mutex", vec![Ty::Int])`, `Ty::Array(Box::new(Ty::Int))`), the method string, and the args
> needed for the EFFECTFUL arms (Mutex.new needs an `args[0]` whose `.ty` is `Ty::Int`; BufReader.new
> needs `args[0].ty == class("File", vec![])`). Use a tiny `class`/`arg` helper to keep it terse. The
> "expected" column is FILLED IN by running the current code (Step 3), not guessed.

- [ ] **Step 2: Write the harness that drives the CURRENT `builtin_method_type`**

The test constructs a minimal real `InferenceEngine` (mirror the construction used in the existing
`typeck` unit/integration tests — `regex_typeck.rs` shows the public `typeck::type_check` entry, but
this test needs the lower-level engine; locate the constructor used by `infer`'s own `#[cfg(test)]`
module and reuse it). For each corpus triple it calls `builtin_method_type` and formats `{:?}` of the
result.

> **Implementer obligation:** `builtin_method_type` is `pub(super)` (collect.rs:326) — NOT reachable from
> an integration test in `tests/`. Two options, pick the one matching repo convention:
> (a) add a `#[cfg(test)] pub fn __golden_builtin_method_type(...)` shim re-export in
> `method_resolvers/mod.rs` gated behind a test-only cfg, OR (b) put the golden test INLINE as
> `#[cfg(test)] mod golden` inside `method_resolvers/mod.rs` where `pub(super)` is in scope. Prefer (b)
> — it needs no visibility widening and keeps the oracle next to the code it pins. Adjust the file in
> the table above from `tests/method_resolver_golden.rs` to an inline module if (b) is chosen; record
> which in the commit.

- [ ] **Step 3: Capture the golden output (the oracle)**

Run the harness in "record" mode (print `(receiver, method) => {:?}` for every triple) against the
CURRENT, unmodified `mod.rs`, and freeze the output as the expected column:

```bash
cargo test -p ruxen_core --lib method_resolvers::golden -- --nocapture \
  2>&1 | tee tmp/test-cache/phase5-task1-golden-record.log
```

Paste the recorded `=> Some(...)`/`=> None` values into the corpus's expected column (or write them to a
committed `tests/fixtures/method_resolver_golden.snapshot` and assert against it — implementer picks the
repo's existing snapshot convention; if none, the inline expected column is simplest).

- [ ] **Step 4: Flip the harness to assert-mode and confirm it passes on CURRENT code**

```bash
cargo test -p ruxen_core --lib method_resolvers::golden \
  2>&1 | tee tmp/test-cache/phase5-task1-green.log
```

Expected: PASS (the oracle agrees with itself — proves the harness is sound BEFORE any migration). This
is the parity backstop every later task re-runs.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/typeck/method_resolvers/mod.rs   # if inline module chosen
git commit -m "test(typeck): golden parity corpus for method_resolvers

Captures the current 1,090-arm match's Option<Ty> answer for one triple per
arm (~290 entries) as the migration oracle. Every Phase-5 migration task
re-asserts this corpus stays green, so the pipeline cannot reorder precedence.
Effectful arms (Mutex.new, BufReader.new) seeded with the args they inspect.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — confirm: pure test addition, no `_ =>`
  catch-all, no production-code change beyond an optional test-cfg shim.

---

## Task 2: Introduce `MethodResolver` + the dispatcher (NO arms migrated yet)

**Files:**
- Create: `compiler/ruxen_core/src/typeck/method_resolvers/resolver.rs`
- Modify: `compiler/ruxen_core/src/typeck/method_resolvers/mod.rs` (add `mod resolver;`, add the
  dispatcher + `resolvers()` assembly; the giant match stays for now, wrapped as a single
  "legacy" resolver so the dispatcher is exercised end-to-end before any arm moves)

- [ ] **Step 1: Write the failing test (dispatcher exists + A2 probe)**

Add to the golden test module two new tests:

```rust
#[test]
fn dispatcher_matches_legacy_for_whole_corpus() {
    // Same assertion as Task 1, but routed through `resolvers()` — proves the
    // dispatcher wrapping the legacy match is behaviour-identical.
    assert_golden_parity(); // reuse Task 1 helper
}

#[test]
fn stdlib_mutex_new_has_no_declared_method_symbol() {
    // A2 precondition: the real stdlib Mutex's `new` is a BUILTIN arm, not a
    // symbol-table DefKind::Method. If this fails, tier-1-first would change
    // stdlib behaviour — see the precedence subtlety note; switch tier 1 to
    // "user-defined receivers only".
    let eng = build_test_engine_with_stdlib(); // implementer: load the prelude
    assert_eq!(eng.lookup_class_method_return("Mutex", "new"), None);
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p ruxen_core --lib method_resolvers 2>&1 | tee tmp/test-cache/phase5-task2-red.log
```

Expected: FAIL to compile — `resolvers`/`MethodResolver` not found.

- [ ] **Step 3: Create `resolver.rs` with the struct + a legacy-wrapping resolver**

`resolver.rs` gets the `MethodResolver` struct (shown in the design section above) plus, FOR NOW, a
single resolver that delegates to the existing match — so the dispatcher is real but no arm has moved:

```rust
/// TEMPORARY (removed by the final migration task): wraps the entire legacy
/// match as one resolver so the dispatcher runs end-to-end before any arm is
/// carved out. `matches` is always-true; `resolve` is the old match body.
pub(super) fn legacy_resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |_, _| true,
        resolve: super::legacy_builtin_method_type, // the renamed old match fn
    }]
}
```

- [ ] **Step 3b: In `mod.rs`, rename the old match fn and add the dispatcher**

Rename the current `pub(super) fn builtin_method_type` body to
`fn legacy_builtin_method_type(...)` (same signature), then add the NEW
`pub(super) fn builtin_method_type` dispatcher (design section) whose `resolvers()` — for THIS task only —
returns `resolver::legacy_resolvers()`. Add `mod resolver;`.

> Tier-1-first vs user-only decision: with the `stdlib_mutex_new_has_no_declared_method_symbol` test
> green, tier-1-first is safe. If that test is RED, change `declared_method_resolvers` (built in Task 3)
> to skip receivers whose name is in a `STDLIB_TYPE_NAMES` set. Record which in the Task 3 commit.

- [ ] **Step 4: Run to verify green**

```bash
cargo test -p ruxen_core --lib method_resolvers 2>&1 | tee tmp/test-cache/phase5-task2-green.log
```

Expected: PASS — corpus parity holds (the dispatcher is a transparent wrapper) and the A2 probe answers
the precedence question.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/typeck/method_resolvers/mod.rs
git commit -m "feat(typeck): MethodResolver struct + ordered dispatcher (legacy-wrapped)

The dispatcher is a transparent wrapper over the existing match (corpus parity
holds). Records the A2-precedence probe result. Sets up the per-namespace
resolver migration; precedence is now one decision, not arm order.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — confirm: net ~+60 LOC (struct + dispatcher),
  the giant match is RENAMED not duplicated, no new `_ =>` (the dispatcher's trailing `None` replaces the
  old `_ => None` 1:1), `resolvers()` is the single precedence decision.

---

## Tasks 3–10: Migrate one tier/namespace per task (assert golden parity after each)

Each task below carves a named group of arms OUT of `legacy_builtin_method_type` and INTO a
`resolvers()` function in its namespace file, inserts that namespace into the dispatcher's `resolvers()`
assembly at its correct precedence slot, DELETES the moved arms from the legacy match, and re-runs the
golden corpus. The corpus staying green IS the proof of behaviour preservation. Arms are **moved, not
copied** — the legacy match shrinks by exactly the arms moved (thermonuke verifies this each task).

**Per-task template (applies to Tasks 3–10 identically):**

- [ ] **Step 1 (test):** the golden corpus already covers these arms (Task 1). No new test needed for
  parity; add a namespace-focused assertion ONLY if the group has an effectful arm whose diagnostic must
  be pinned (e.g. concurrency's E1101, io's E0714) — assert the diagnostic still fires with the same code
  and the same `Some(Ty::Error)` return. Run to confirm it currently passes (still on legacy):
  `cargo test -p ruxen_core --lib method_resolvers 2>&1 | tee tmp/test-cache/phase5-taskN-pre.log`.
- [ ] **Step 2 (red):** create the namespace file with an EMPTY `pub(super) fn resolvers() -> Vec<MethodResolver> { vec![] }`,
  add `mod <ns>;`, insert `v.extend(<ns>::resolvers());` at the right precedence slot, and DELETE the
  group's arms from the legacy match. Run:
  `cargo test -p ruxen_core --lib method_resolvers 2>&1 | tee tmp/test-cache/phase5-taskN-red.log`.
  Expected: FAIL — the corpus triples for the deleted arms now return `None` (the empty namespace hasn't
  re-supplied them yet). This is the red that proves the arms were actually removed from legacy.
- [ ] **Step 3 (impl):** fill `<ns>::resolvers()` with one `MethodResolver` per logical arm (or one per
  named type, with `resolve` doing an inner `match method` — implementer picks the granularity that keeps
  each `resolve` readable; prefer one resolver per receiver TYPE with an inner method match, since the
  `matches` guard is "is this receiver a `Class` named X"). Transcribe the arm bodies VERBATIM from the
  legacy match — same `eng`/`args`/`span` calls, same diagnostics, same early-`return Some(Ty::Error)`.
- [ ] **Step 4 (green):** `cargo test -p ruxen_core --lib method_resolvers 2>&1 | tee tmp/test-cache/phase5-taskN-green.log`.
  Expected: PASS — corpus parity restored. If a triple differs, the transcription dropped a guard/branch;
  fix the namespace `resolve`, do NOT touch the corpus.
- [ ] **Step 5 (commit):**

```bash
git add compiler/ruxen_core/src/typeck/method_resolvers/
git commit -m "refactor(typeck): move <namespace> arms to method_resolvers/<ns>.rs

Behaviour-preserving — golden corpus green. <N> arms moved out of the legacy
match into the namespace resolver; arms are MOVED not duplicated.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```
- [ ] **Direction-check (/thermonuke on this task's diff):** legacy match shrank by exactly the moved
  arms (net negative in mod.rs, net positive in the new file, no duplication), no new `_ =>` inside the
  namespace `resolve` (an inner `match method { … _ => None }` is permitted ONLY as the namespace's
  "method not in this type" fallthrough — flag if it's a catch-all over RECEIVERS), the pin diagnostic
  test (if any) still green.

The eight migration tasks, in dispatcher precedence order (so each task's golden run also validates the
precedence slot is correct):

### Task 3 — TIER 1: `declared_method_resolvers` + `structural_fallback_resolvers` (in `resolver.rs`)
The generalization of the current-branch one-off at `mod.rs:1173`. `declared_method_resolvers` matches
`Class`/`Struct`/`Enum` receivers and returns `eng.lookup_class_method_return(name, method)` (tier 1).
`structural_fallback_resolvers` holds the generic `(Ty::Class, "to_s"/"clone"/"new"/"default")`,
`(Ty::Struct, …)`, `(Ty::Enum, …)` arms (mod.rs:1143–1194) as tier 3's tail. **Delete** the legacy
`(Ty::Class, "new") => match lookup_class_method_return …` arm (1173) — it is now tier 1. Honour the
Task 2 A2 decision (tier-1-first vs user-only). This is the task that FIXES A2; the golden corpus
(stdlib receivers, no user-declared methods) must stay green, proving stdlib behaviour is unchanged.

### Task 4 — TIER 2: `concurrency.rs` (`Mutex`/`MutexGuard`/`Arc`/`SharedSync`/`Thread`/`ThreadPanic`/`JoinHandle`/`Future`, mod.rs:229–432)
Includes the E1101/E1102 Send-bound diagnostics (Mutex.new line 348, SharedSync.new). Pin test: assert
`Mutex.new` on a non-Send payload still pushes E1101 and returns `Some(class("Mutex", …))`.

### Task 5 — TIER 2: `fmt.rs` (`Formatter`, mod.rs:443–470)

### Task 6 — TIER 2: `io.rs` (`Stdin`/`Stdout`/`Stderr`/`BufReader`/`BufWriter`/`IoError`, mod.rs:588–642, 957–1093)
Includes the `is_bufio_inner_supported` E0714 guard + `return Some(Ty::Error)` (mod.rs:962, 979, 1017).
Pin test: `BufReader.new` with an unsupported inner still pushes E0714 and returns `Some(Ty::Error)`.

### Task 7 — TIER 2: `fs.rs` (`Metadata`/`File`/`OpenOptions`, mod.rs:648–800)

### Task 8 — TIER 2: `process.rs` (`Command`/`Output`/`ExitStatus`, mod.rs:659–687) + `net.rs` (`TcpListener`/`TcpStream`, mod.rs:853–940) + `time.rs` (`Duration`/`Instant`, mod.rs:809–845)
Three small namespaces in one task (each < 10 arms; splitting would be three near-identical commits for
no review benefit — YAGNI). Add three `mod` decls + three `v.extend` slots.

### Task 9 — TIER 3: `strings.rs` (`Ty::String`/`Ty::Str` structural, mod.rs:30–~110) + `SplitIter` (1098) + `ParseIntError`/`ParseFloatError` (112)
Largest structural group (82 `Ty::String` + 33 `Ty::Str` arm lines).

### Task 10 — TIER 3: `collections.rs` (`Array`/`Option`/`Result`/`Map`/`Set`/`Tuple`) + `numeric.rs` (`Int*`/`UInt*`/`USize`/`ISize`/`Float*`/`Bool`/`Char`/`Unit`)
The remaining structural arms. After this task, `legacy_builtin_method_type` should be EMPTY except an
unreachable tail — remove the legacy resolver from `resolvers()` and DELETE `legacy_builtin_method_type`
entirely. Verify with: `grep -n 'legacy_builtin_method_type' compiler/ruxen_core/src/typeck/method_resolvers/mod.rs`
returns nothing. The dispatcher's trailing `None` is the only "unmatched" path left.

> **Implementer note for Tasks 9–10:** transcribe the structural arm bodies (some build nested `Ty`, e.g.
> `Ty::String, "chars" => Some(Ty::Array(Box::new(Ty::Char)))`) verbatim. These are pure (no `eng`/`args`),
> so the `resolve` fn ignores those params (`|_, ty, method, _, _| match (ty, method) { … }`). Keep the
> receiver-head match exhaustive-per-namespace with a trailing `_ => None` that means "this method isn't
> a String method" — that is a within-namespace fallthrough, NOT the banned cross-cutting catch-all.

---

## Task 11: Verify bug A2 is FIXED (the only intended behaviour change)

**Files:**
- Add to: `compiler/ruxen_core/tests/method_resolver_golden.rs` (or the inline golden module) — this is a
  new behaviour test, distinct from the parity corpus.

- [ ] **Step 1: Write the A2 fix test (must already pass after Task 3, re-verified here at phase end)**

```rust
/// Bug A2: a USER class named like a stdlib type (`File`/`Mutex`) that declares
/// its own `new -> Result[...]` must honour its DECLARED return, not be shadowed
/// by the builtin stdlib arm. Before Phase 5, the named-stdlib arm (e.g. the
/// File arms at mod.rs:692) fired first and returned the stdlib shape.
#[test]
fn user_class_named_like_stdlib_honours_declared_new_return_a2() {
    // A user program declaring `class File ... def self.new -> Result[File, String]`.
    let src = r#"
        class File
          def self.new -> Result[File, String]
            Ok(File.new)
          end
        end
        def main
          let f = File.new
        end
    "#; // implementer: adjust to real Ruxen surface syntax for self.new + Result
    let ret = builtin_method_type_for(src, /* receiver */ class("File", vec![]), "new");
    // DECLARED return wins: Result[File, String], NOT the bare File the stdlib
    // File arms (or the structural Class-new fallback) would have produced.
    assert!(matches!(ret, Some(Ty::Result(..))),
        "user-declared File.new -> Result must win over builtin; got {ret:?}");
}
```

> **Implementer obligation:** there is no stdlib `File` arm for `new` returning bare `File` in the
> current code that a user could collide with on `new` specifically — VERIFY against the fs.rs arms
> (mod.rs:692–787) which method names the stdlib `File` actually claims, and pick the COLLISION method
> the test should exercise (it may need to be a method the stdlib File arm DOES define, e.g. a
> read/metadata method, declared on the user class with a different return). The point of A2 is
> "declared beats builtin"; choose a real collision. If the only safe collision is `new` via the
> structural fallback, the test above is correct as written. Record the chosen collision in the commit.

- [ ] **Step 2: Run** (`cargo test -p ruxen_core --lib method_resolvers::golden::user_class_named_like_stdlib 2>&1 | tee tmp/test-cache/phase5-task11.log`).
  Expected: PASS (tier-1-first / declared-method-resolver already routes it). If RED, the precedence
  decision from Task 2/3 is wrong — fix the resolver order, NOT the test.

- [ ] **Step 3: Commit**

```bash
git add compiler/ruxen_core/src/typeck/method_resolvers/
git commit -m "test(typeck): pin bug A2 fix — declared method beats builtin stdlib arm

A user class named like a stdlib type with a declared method now honours its
declared return instead of being shadowed by the builtin arm. Records the
chosen real collision method (see implementer note). Golden corpus still green.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — pure test addition pinning the one intended
  behaviour change; confirm the golden parity corpus (stdlib receivers) is STILL green alongside it (the
  two are not allowed to contradict — A2 only changes user-declared receivers).

---

## Task 12: Phase-5 final integration (phase gate)

**Files:** none (verification only).

- [ ] **Step 1: Run the full compiler-crate suite once**

```bash
cargo test -p ruxen_core 2>&1 | tee tmp/test-cache/phase5-final.log
```

Expected: all green. Per global rule 41/42 this is the ONLY full-suite run in Phase 5; intermediate
tasks ran only `--lib method_resolvers` (narrow) plus their pin tests.

- [ ] **Step 1b: Run the named pin test from the master plan**

```bash
cargo test -p ruxen_core --test regex_typeck 2>&1 | tee tmp/test-cache/phase5-final-regex.log
```

Expected: PASS (5 tests) — `Regex.new -> Result` is the canonical declared-method-beats-builtin case the
current branch diff already exercises; it MUST still resolve through tier 1.

- [ ] **Step 2: Confirm the legacy match is gone and no catch-all leaked**

```bash
grep -n 'legacy_builtin_method_type' compiler/ruxen_core/src/typeck/method_resolvers/mod.rs   # expect: nothing
grep -rnE '_\s*=>\s*Some' compiler/ruxen_core/src/typeck/method_resolvers/   # expect: nothing (no catch-all CLAIMING a type)
grep -rnE 'matches:\s*\|_,\s*_\|\s*true' compiler/ruxen_core/src/typeck/method_resolvers/ | grep -v legacy   # expect: nothing (no always-match resolver survives)
```

The only permitted `_ => None` is (a) the dispatcher tail in `mod.rs` and (b) within-namespace
"method not in this type" fallthroughs. NO resolver may match all receivers, and NO `_ => Some(...)`
may exist (that would re-create the global-claim bug class the file's NOTE at mod.rs:1196 warns about).

- [ ] **Step 3: Full multi-agent `/thermonuke` sweep (authoritative phase gate)**

Invoke the `thermonuke` skill on the whole Phase 5 diff (`git diff <phase5-base>..HEAD`). It must
confirm: the ~1,090-arm / 1,212-LOC `mod.rs` match is REPLACED by a ~150-LOC dispatcher + small
per-namespace tables (~900 net reduction target), precedence is one ordered decision, bug A2 is fixed
and pinned, the golden corpus pins every arm, and no structural debt (no god-resolver, no catch-all)
was introduced. Surface its report to the maintainer.

- [ ] **Step 4: Report**

Report: net line delta (`git diff --stat <phase5-base>..HEAD`), the A2 fix test + regex pin green, the
golden corpus green (cite `tmp/test-cache/phase5-task1-green.log` → `phase5-final.log`), full suite green,
and the statement: "No behaviour changed except bug A2 (declared method now beats builtin stdlib arm);
every arm's return type is pinned identical by the ~290-entry golden corpus." Await go-ahead for Phase 3.

---

## Self-Review (run before handing off)

**Spec coverage:** Phase 5 goal — convert the match to the `declared → named-stdlib → structural`
pipeline the header (mod.rs:7–12) specs (✓ Tasks 2–10), one precedence decision (✓ the `resolvers()`
order), bug A2 fixed (✓ Task 3, pinned Task 11). The header's own `resolvers() -> Vec<MethodResolver>`
phrasing is honoured literally.

**Precedence-preservation proof:** the golden corpus (Task 1, ~290 entries — one per arm line, verified
via `grep '^\s*(Ty::'`) is the oracle; Tasks 3–10 each re-assert it. The ONE intended divergence (A2) is
isolated to user-declared receivers, which the corpus deliberately does not contain, so corpus-green and
A2-green are non-contradictory (Task 11 direction-check enforces this).

**Catch-all discipline:** the single legacy `_ => None` (mod.rs:1210) becomes the dispatcher tail; no new
cross-cutting `_ =>` is introduced (Task 12 Step 2 greps prove it). Within-namespace `match method { _ =>
None }` fallthroughs are scoped to a single receiver type and are NOT the banned global catch-all (the
file's mod.rs:1196 NOTE documents exactly the `_ => Some(String)` bug class we must not reintroduce).

**Placeholder scan:** No fake code. Genuinely API-dependent points are marked as explicit implementer
sub-steps: (1) the corpus's 290 entries + expected column are RECORDED from the current code, not guessed
(Task 1 Steps 1,3); (2) the `InferenceEngine` test-construction helper is located from the existing infer
test module, not invented (Task 1 Step 2, Task 2); (3) the tier-1-first vs user-only precedence is decided
EMPIRICALLY by the `stdlib_mutex_new_has_no_declared_method_symbol` probe (Task 2), not assumed; (4) the
A2 collision method is chosen against the real fs.rs arms (Task 11). Each is flagged inline.

**Signature stability:** `builtin_method_type(eng, ty, method, args, span) -> Option<Ty>` is unchanged —
callers at `infer/collect.rs:333` and `infer/expr.rs:482` are byte-identical. `MethodResolver` is
`pub(super)`, function-pointer fields (no `Box<dyn Fn>`, no new dep) — verified every arm is a free
computation with no captured env; if one isn't, the implementer is instructed to STOP and surface it.

**DRY/YAGNI:** namespace files map 1:1 to the guarded-name groups the code already formed (no invented
abstraction layer); three tiny namespaces share one task (Task 8); no `OnceLock` resolver-caching added
(no profile justifies it). Net target ~900 LOC removed (1,212 → ~300 across mod.rs + tables), matching
the master-plan estimate.
