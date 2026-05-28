# REPL State Refactor — Drop `all_statements` Replay, Migrate to Slot-Based State

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Ruxen REPL execute each user input's side-effecting statements **exactly once** (matching compile-and-run semantics) while preserving variable state across inputs, so the 5 release-e2e parity fixtures and any future non-idempotent fixture run correctly under `ruxen repl`.

**Architecture:** The current REPL clones `session.all_statements` into every wrapper and uses a line-count `prev_captured_output` diff to mask the side-effects of replay. The proper architecture (already partially scaffolded in `session.rs` via `slots` + `var_slots` + `register_var()`) is to make each new input's wrapper contain **only the new statement**, with session-variable reads/writes lowered to load/store against a stable, address-fixed slot region. Replay then disappears, and so does the capture-buffer workaround.

**Tech Stack:** Rust (compiler + REPL crates), Cranelift JIT (`cranelift-jit` 0.130), C runtime (`library/std/future/runtime/*.c` for async persistence).

---

## Acceptance bar

The refactor is done when **all** of the following are true on a clean clone:

```bash
# 1. All 5 currently failing REPL parity fixtures pass:
PHASES=repl CASE_FILTER='508_command_status.rx|534_fs_canonicalize.rx|536_fs_read_link.rx|727_async_tcp_echo.rx|727b_async_tcp_read_timeout.rx' \
  RUXEN_WORKSPACE=$PWD ./tests/release-e2e/run.sh

# 2. Full release-e2e run, fully green:
RUXEN_WORKSPACE=$PWD ./tests/release-e2e/run.sh   # total/pass match, fail=0

# 3. Workspace tests green:
cargo test --workspace --no-fail-fast
```

Interactive REPL session UX (verified by hand): `let mut x = 0`, then `x += 1`, then `x` prints `1`. Adding a `def foo(...)` in one input and calling it in the next still works. `Instant.now()` evaluated in input N does **not** re-evaluate when input N+1 is submitted (already true today via the existing slots scaffolding; the refactor must preserve this).

---

## Affected file inventory

**Modified:**
- `src/ruxen_repl/src/session.rs` — drop `all_statements`, `prev_captured_output`; tighten `var_slots` API.
- `src/ruxen_repl/src/eval.rs` — rewrite `eval_expression` and `eval_statement` to stop replaying; drop the line-count suffix logic.
- `src/ruxen_repl/src/jit.rs` — lower session-var identifier reads to `load.i64 [slot_addr]`, session-var writes to `store.i64 ... [slot_addr]`; remove any code path that handles the replayed-statements case.
- `src/ruxen_repl/src/capture.rs` — either delete (preferred) or shrink to a no-op tee. Per design Q1, `puts` and subprocess output both pass through to real stdout.

**Created:**
- `src/ruxen_repl/src/tests/state_persistence.rs` — golden tests covering var read/write across inputs for every supported type.
- `src/ruxen_repl/src/tests/single_execution.rs` — assertions that every side-effecting statement runs exactly once.
- `src/ruxen_repl/src/tests/async_persistence.rs` — task spawned in input N is still alive in input N+1.

**Touched but not refactored (verify behaviour unchanged):**
- `library/std/future/runtime/executor.c` / `scheduler.c` / `reactor.c` — the executor is already a process-global; we just need to confirm it's never reset on `eval_input` boundary.

---

## Phase 0 — Safety net

### Task 0.1: Snapshot current REPL state-persistence behaviour

**Why:** The refactor moves load-bearing semantics. Before changing anything, lock in tests for what we expect to keep working: `let` persisting across inputs, function defs being callable from a later input, mutation surviving.

**Files:**
- Create: `src/ruxen_repl/src/tests/mod.rs`
- Create: `src/ruxen_repl/src/tests/state_persistence.rs`
- Modify: `src/ruxen_repl/src/lib.rs` (add `#[cfg(test)] mod tests;`)

- [ ] **Step 1: Write the failing test scaffold**

```rust
// src/ruxen_repl/src/tests/state_persistence.rs
use crate::eval::{eval_input, EvalResult};
use crate::session::ReplSession;

/// Feed a sequence of inputs to a fresh session, capturing the
/// real-stdout side of each `EvalResult`. Used as the golden harness
/// for every test in this file.
fn run_session(inputs: &[&str]) -> Vec<String> {
    let mut session = ReplSession::new().expect("session");
    inputs
        .iter()
        .map(|inp| match eval_input(&mut session, inp) {
            EvalResult::Ok(Some(s)) => s,
            EvalResult::Ok(None) => String::new(),
            EvalResult::Command(s) => s,
            EvalResult::Quit => String::new(),
            EvalResult::Err(e) => panic!("input {:?} → {}", inp, e),
        })
        .collect()
}

#[test]
fn let_binding_survives_next_input() {
    let outs = run_session(&[
        "let x = 41",
        "x + 1",
    ]);
    // Last input's display value should contain "42"
    assert!(outs[1].contains("42"), "got: {:?}", outs);
}

#[test]
fn mutation_persists_across_inputs() {
    let outs = run_session(&[
        "let mut counter = 0",
        "counter = counter + 1",
        "counter = counter + 1",
        "counter",
    ]);
    assert!(outs[3].contains("2"), "got: {:?}", outs);
}

#[test]
fn def_callable_from_later_input() {
    let outs = run_session(&[
        "def double(n: Int) -> Int; n * 2; end",
        "double(21)",
    ]);
    assert!(outs[1].contains("42"), "got: {:?}", outs);
}

#[test]
fn type_item_visible_from_later_input() {
    let outs = run_session(&[
        "class Point\n  x: Int\n  y: Int\n  def init(@x: Int, @y: Int) end\nend",
        "let p = Point.new(3, 4)",
        "p.x + p.y",
    ]);
    assert!(outs[2].contains("7"), "got: {:?}", outs);
}
```

- [ ] **Step 2: Wire the test module into the crate**

```rust
// src/ruxen_repl/src/tests/mod.rs
mod state_persistence;
```

```rust
// src/ruxen_repl/src/lib.rs — append at the bottom of the file:
#[cfg(test)]
mod tests;
```

- [ ] **Step 3: Run the tests and confirm they pass against current (unrefactored) code**

```bash
cargo test -p ruxen_repl --lib state_persistence -- --nocapture
```
Expected: all 4 PASS. (If any fail today, that's a pre-existing bug — surface it in the commit message before continuing.)

- [ ] **Step 4: Commit**

```bash
git add src/ruxen_repl/src/tests/ src/ruxen_repl/src/lib.rs
git commit -m "test(repl): lock in state-persistence semantics before refactor

Four golden tests covering: let across inputs, mutation across inputs,
def callable from later input, class visible from later input. These
pin the user-visible contract the upcoming all_statements-replay
removal must preserve."
```

### Task 0.2: Snapshot the side-effect-firing contract

**Why:** Capture the bug we're fixing as an executable test. After the refactor it must flip from RED (current behaviour: prints twice) to GREEN (prints once).

**Files:**
- Create: `src/ruxen_repl/src/tests/single_execution.rs`
- Modify: `src/ruxen_repl/src/tests/mod.rs`

- [ ] **Step 1: Write the test that documents current (broken) behaviour as `#[ignore]`d, and a sibling expected-after-refactor test**

```rust
// src/ruxen_repl/src/tests/single_execution.rs
use super::state_persistence::run_session;

/// Once the refactor lands, `puts` MUST fire exactly once per input.
/// Currently the REPL line-count-diffs cumulative output to mask the
/// replay, which works for puts of static strings but NOT for
/// subprocess output (508) or for prior statements that fail on
/// replay (534/536/727/727b).
///
/// Marked `#[ignore]` until Phase 3 lands so CI stays green.
#[test]
#[ignore = "flips green when Phase 3 lands"]
fn puts_fires_exactly_once_per_input() {
    let outs = run_session(&[
        r#"puts "hello""#,
        r#"puts "world""#,
    ]);
    // input 0 emits "hello\n", input 1 emits "world\n". Neither
    // should contain a duplicate "hello".
    assert_eq!(outs[0].matches("hello").count(), 1, "got: {:?}", outs);
    assert_eq!(outs[1].matches("hello").count(), 0, "got: {:?}", outs[1]);
    assert_eq!(outs[1].matches("world").count(), 1, "got: {:?}", outs[1]);
}

#[test]
#[ignore = "flips green when Phase 3 lands"]
fn subprocess_stdout_appears_once() {
    let outs = run_session(&[
        r#"use std.process.Command"#,
        r#"let _ = Command.new("echo", "hello").status"#,
        r#"puts "after""#,
    ]);
    let joined: String = outs.concat();
    assert_eq!(joined.matches("hello").count(), 1, "got: {:?}", outs);
    assert_eq!(joined.matches("after").count(), 1, "got: {:?}", outs);
}
```

```rust
// src/ruxen_repl/src/tests/mod.rs — append:
mod single_execution;
```

- [ ] **Step 2: Verify the `#[ignore]`d tests are discoverable but not run by default**

```bash
cargo test -p ruxen_repl --lib single_execution 2>&1 | grep -E "test .* ignored"
```
Expected: both `puts_fires_exactly_once_per_input` and `subprocess_stdout_appears_once` listed as ignored.

- [ ] **Step 3: Commit**

```bash
git add src/ruxen_repl/src/tests/single_execution.rs src/ruxen_repl/src/tests/mod.rs
git commit -m "test(repl): pin the once-per-input side-effect contract (ignored)

Two ignored tests document the bug the refactor fixes. They flip
green when Phase 3 lands and the all_statements replay disappears."
```

---

## Phase 1 — Slot-based identifier RESOLVE in the JIT

This is the foundation: when the JIT lowers a reference to a session variable, it must emit `load.i64 [slot_addr]` against a baked-in absolute address. Right now `var_slots` is declared but `jit.rs` has zero `slots_base_addr` callers (confirmed via grep). We need to add the load path first, in isolation, before removing the replay.

### Task 1.1: Add session-var resolver state to the JIT compile context

**Files:**
- Modify: `src/ruxen_repl/src/jit.rs:603-624` (the `def_local` / `use_local` helpers that resolve `LocalId`)
- Modify: `src/ruxen_repl/src/eval.rs` (pass the session's `var_slots` view into the compile path)

- [ ] **Step 1: Add a `SessionVarMap` struct to jit.rs that maps name → absolute slot address**

```rust
// src/ruxen_repl/src/jit.rs — near the top of the file, after imports:

/// Maps session-variable names to their absolute slot addresses.
/// Built once per `eval_input` from `ReplSession::var_slots` and
/// `ReplSession::slot_addr(idx)`. Used by the JIT when lowering
/// identifier reads / writes to load/store against the persistent
/// slot region.
#[derive(Default, Clone)]
pub struct SessionVarMap {
    /// name → absolute byte address of the slot's i64 cell
    inner: std::collections::HashMap<String, i64>,
}

impl SessionVarMap {
    pub fn new() -> Self { Self::default() }
    pub fn insert(&mut self, name: String, addr: i64) {
        self.inner.insert(name, addr);
    }
    pub fn lookup(&self, name: &str) -> Option<i64> {
        self.inner.get(name).copied()
    }
}
```

- [ ] **Step 2: Thread `SessionVarMap` through the JIT compile entry point**

Locate the `pub fn define_function` (or equivalent — the public JIT entrypoint called from `eval::compile_and_execute`). Add a `session_vars: &SessionVarMap` parameter. Inside, store it in the per-function compile context alongside `stack_slots`.

- [ ] **Step 3: Build the map at the call site in `eval.rs::compile_and_execute`**

```rust
// in src/ruxen_repl/src/eval.rs — inside compile_and_execute, BEFORE
// the call to define_function:
let mut session_vars = jit::SessionVarMap::new();
for vs in &session.var_slots {
    session_vars.insert(vs.name.clone(), session.slot_addr(vs.idx));
}
```

- [ ] **Step 4: Build, ensure no regression**

```bash
cargo build -p ruxen_repl 2>&1 | tail -5
cargo test -p ruxen_repl --lib state_persistence
```
Expected: build green, 4 state-persistence tests still PASS (we haven't changed semantics, just plumbing).

- [ ] **Step 5: Commit**

```bash
git add src/ruxen_repl/src/jit.rs src/ruxen_repl/src/eval.rs
git commit -m "repl(jit): plumb SessionVarMap into the compile context

No semantic change yet — the map is built and threaded through but
not consulted. Foundation for replacing all_statements replay with
slot loads/stores in Phase 1.2-1.4."
```

### Task 1.2: Lower session-var IDENTIFIER reads to slot loads

**Files:**
- Modify: `src/ruxen_repl/src/jit.rs` (the `gen_value` arm that handles identifier/local-id reads — search for the `use_local` callsite)

The MIR lowering for the REPL wrapper currently emits `LocalId` references for identifiers. Those `LocalId`s come from the synthetic wrapper that includes replayed `let`s. We need: **when the identifier name corresponds to a `SessionVarMap` entry**, emit `load.i64` from the baked-in address instead.

This requires that the MIR lowering preserves the source-level name for identifiers that are session vars — currently identifiers become anonymous `LocalId`s after lowering. The simplest approach: keep a side-channel `local_id → session_var_name` map populated during the per-input MIR lowering. The JIT consults this map; if a `LocalId` has a session-var name attached, the JIT uses the slot path.

- [ ] **Step 1: Write the failing unit test**

```rust
// src/ruxen_repl/src/tests/state_persistence.rs — append:

#[test]
fn slot_load_is_used_for_session_var_read() {
    // Two inputs, the second references a var defined in the first.
    // Even when Phase 4 removes all_statements replay, this test must
    // remain green — it exercises the slot READ path that Phase 1
    // installs.
    let outs = run_session(&[
        "let answer = 42",
        "answer",
    ]);
    assert!(outs[1].contains("42"), "got: {:?}", outs);
}
```

- [ ] **Step 2: Run the test to confirm current behaviour still passes**

```bash
cargo test -p ruxen_repl --lib slot_load_is_used_for_session_var_read
```
Expected: PASS (currently via replay; Phase 4 will remove replay and this same test must still pass via slots).

- [ ] **Step 3: Add a side-channel name carrier to MIR lowering**

Inspect `compiler/ruxen_core/src/mir/lower/` to find where identifier expressions become `LocalId`s. Add an optional `original_name: Option<String>` field on the `MirLocal` (or equivalent) struct, populated when the identifier is a free name (not a fresh temporary). Plumb it through to the JIT.

Concrete grep target: `mir/lower/expr/` — look for the call that creates a `Local` from an `Identifier` expression.

- [ ] **Step 4: In the JIT's `use_local` helper, prefer slot load when the name is in the SessionVarMap**

```rust
// src/ruxen_repl/src/jit.rs — modify the use_local helper at ~line 619:
fn use_local(
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    session_vars: &SessionVarMap,
    local_id: LocalId,
    local_name: Option<&str>,   // NEW — source identifier name if any
    builder: &mut FunctionBuilder,
) -> Value {
    // Slot path: session var, addressable by absolute slot address.
    if let Some(name) = local_name {
        if let Some(addr) = session_vars.lookup(name) {
            let addr_v = builder.ins().iconst(types::I64, addr);
            return builder.ins().load(types::I64, MemFlags::trusted(), addr_v, 0);
        }
    }
    // Fallback: stack-slot or SSA variable as before.
    if let Some(&slot) = stack_slots.get(&local_id) {
        builder.ins().stack_load(types::I64, slot, 0)
    } else {
        builder.use_var(var_map[&local_id])
    }
}
```

- [ ] **Step 5: Run the persistence tests + the existing REPL tests, confirm no regression**

```bash
cargo test -p ruxen_repl --lib
```
Expected: all PASS (the slot path is wired but `all_statements` replay is still the primary mechanism — the slot path is dormant when the replayed `let` already defines the binding in-scope).

- [ ] **Step 6: Commit**

```bash
git add -- src/ruxen_repl/src/jit.rs compiler/ruxen_core/src/mir/lower/
git commit -m "repl(jit): lower session-var reads to slot loads (dormant path)

Adds the slot READ path through use_local: when a LocalId carries
the source name of a session variable, emit load.i64 against the
baked-in slot address. Behaviour unchanged until Phase 4 because the
all_statements replay still defines the same name as a real Local."
```

### Task 1.3: Lower session-var WRITES (let RHS and assignment) to slot stores

**Files:**
- Modify: `src/ruxen_repl/src/jit.rs` (the `def_local` helper at ~line 603, and the Assignment instruction arm)

- [ ] **Step 1: Add a failing test for re-assignment across inputs that DOESN'T rely on replay**

```rust
// src/ruxen_repl/src/tests/state_persistence.rs — append:

#[test]
fn assignment_persists_via_slot_only() {
    // Once Phase 3 lands, `counter = counter + 1` in input 2 must
    // STORE to the slot — not be a no-op because the wrapper local
    // went out of scope.
    let outs = run_session(&[
        "let mut counter = 10",
        "counter = counter + 5",
        "counter",
    ]);
    assert!(outs[2].contains("15"), "got: {:?}", outs);
}
```

- [ ] **Step 2: Modify `def_local` to write to the slot when the name matches a SessionVarMap entry**

```rust
// src/ruxen_repl/src/jit.rs — modify def_local at ~line 603:
fn def_local(
    var_map: &mut HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    session_vars: &SessionVarMap,
    builder: &mut FunctionBuilder,
    local_id: LocalId,
    local_name: Option<&str>,   // NEW
    value: Value,
) {
    let widened = widen_i64(builder, value);
    if let Some(name) = local_name {
        if let Some(addr) = session_vars.lookup(name) {
            let addr_v = builder.ins().iconst(types::I64, addr);
            builder.ins().store(MemFlags::trusted(), widened, addr_v, 0);
            return;
        }
    }
    if let Some(&slot) = stack_slots.get(&local_id) {
        builder.ins().stack_store(widened, slot, 0);
    } else {
        let var = var_map.entry(local_id).or_insert_with(|| {
            // … existing var-creation logic …
            unimplemented!("preserve current logic — copy from before");
        });
        builder.def_var(*var, widened);
    }
}
```

- [ ] **Step 3: Update every `def_local` / `use_local` call site to pass the optional `local_name`**

The names come from the MIR side-channel added in Task 1.2 Step 3. Plumb them through every `gen_value` / `gen_instruction` site.

- [ ] **Step 4: Run the full REPL test suite**

```bash
cargo test -p ruxen_repl --lib
```
Expected: all 5 state-persistence tests + 0 single_execution (still ignored) PASS.

- [ ] **Step 5: Commit**

```bash
git add -- src/ruxen_repl/src/jit.rs compiler/ruxen_core/src/mir/lower/
git commit -m "repl(jit): lower session-var writes to slot stores

Completes the slot-load/slot-store pair for session variables. Still
dormant — Phase 4 will remove the replay path that currently shadows
this with in-wrapper locals."
```

---

## Phase 2 — Verify slot pathway works for every supported type

Slots are i64 cells. For 8-byte handles (heap pointers, raw integers, Cranelift-passed-by-pointer types) the read/write path is uniform. We need to confirm every concrete type the stdlib exposes (`String`, `Array[T]`, `Option[T]`, `Result[T,E]`, `HashMap`, user `class`/`struct`) actually round-trips through a slot.

### Task 2.1: Per-type slot round-trip tests

**Files:**
- Modify: `src/ruxen_repl/src/tests/state_persistence.rs`

- [ ] **Step 1: Add one test per type. Each takes the form: declare in input 0, mutate/observe in input 1.**

```rust
// src/ruxen_repl/src/tests/state_persistence.rs — append:

#[test]
fn slot_roundtrip_string() {
    let outs = run_session(&[
        r#"let s = String.from("hello")"#,
        r#"s.length"#,
    ]);
    assert!(outs[1].contains("5"), "got: {:?}", outs);
}

#[test]
fn slot_roundtrip_array() {
    let outs = run_session(&[
        "let mut v = [1, 2, 3]",
        "v.push(4)",
        "v.length",
    ]);
    assert!(outs[2].contains("4"), "got: {:?}", outs);
}

#[test]
fn slot_roundtrip_option() {
    let outs = run_session(&[
        "let o = Option.Some(42)",
        "match o\n  Some(n) -> n\n  None -> 0\nend",
    ]);
    assert!(outs[1].contains("42"), "got: {:?}", outs);
}

#[test]
fn slot_roundtrip_result() {
    let outs = run_session(&[
        "let r: Result[Int, String] = Result.Ok(7)",
        "r.unwrap!",
    ]);
    assert!(outs[1].contains("7"), "got: {:?}", outs);
}

#[test]
fn slot_roundtrip_hashmap() {
    let outs = run_session(&[
        "let mut m = HashMap[String, Int].new",
        r#"m.insert(String.from("a"), 1)"#,
        r#"m.get(&String.from("a"))"#,
    ]);
    let joined: String = outs.concat();
    assert!(joined.contains("1") || joined.contains("Some"),
            "got: {:?}", outs);
}

#[test]
fn slot_roundtrip_user_class() {
    let outs = run_session(&[
        "class Point\n  x: Int\n  y: Int\n  def init(@x: Int, @y: Int) end\nend",
        "let p = Point.new(7, 11)",
        "p.x * p.y",
    ]);
    assert!(outs[2].contains("77"), "got: {:?}", outs);
}
```

- [ ] **Step 2: Run them — all should PASS today (still via replay)**

```bash
cargo test -p ruxen_repl --lib slot_roundtrip
```

- [ ] **Step 3: Commit**

```bash
git add src/ruxen_repl/src/tests/state_persistence.rs
git commit -m "test(repl): per-type slot round-trip coverage

Six tests covering every stdlib type that user code can put in a
session variable. They currently pass via replay; after Phase 4 they
must pass via slot load/store only."
```

---

## Phase 3 — Stop replaying side-effecting statements (the actual fix)

This is the load-bearing change. Once the slot path proven works for every type, we remove the `all_statements.clone()` calls and the `prev_captured_output` line-count diff. Side effects fire exactly once. Capture machinery becomes vestigial.

### Task 3.1: Remove replay from `eval_expression`

**Files:**
- Modify: `src/ruxen_repl/src/eval.rs:128-149`

- [ ] **Step 1: Un-`#[ignore]` the `puts_fires_exactly_once_per_input` and `subprocess_stdout_appears_once` tests**

```rust
// src/ruxen_repl/src/tests/single_execution.rs — remove the
// `#[ignore = "..."]` line from BOTH tests.
```

- [ ] **Step 2: Confirm they now FAIL on the current code (red phase)**

```bash
cargo test -p ruxen_repl --lib single_execution -- --nocapture
```
Expected: both FAIL with duplicate "hello" / "after" counts > 1.

- [ ] **Step 3: Replace the replay in `eval_expression`**

```rust
// src/ruxen_repl/src/eval.rs — replace the body of eval_expression
// (currently 128-150) with:

fn eval_expression(session: &mut ReplSession, raw_input: &str, expr: Expr) -> EvalResult {
    let fn_name = session.next_repl_fn_name();
    let span = expr.span.clone();
    let side_effecting = is_side_effect_expr(&expr);

    // NO REPLAY. The wrapper contains ONLY the new statement. Prior
    // session-var values are read via slot loads; `def`s and type
    // items are still injected via `func_defs` / `type_items` (those
    // are pure declarations, idempotent on re-typecheck — they don't
    // execute side effects). Removing the all_statements clone is
    // what makes 508_command_status, 534_fs_canonicalize, 536_fs_read_link,
    // 727_async_tcp_echo, 727b_async_tcp_read_timeout pass.
    let statements: Vec<Statement> = vec![Statement::Expression(expr.clone())];

    let wrapper = build_program(
        &session.func_defs,
        &session.type_items,
        &fn_name,
        statements,
        &span,
    );

    let hook = if side_effecting {
        Some(CompileHook::RecordStatement(Statement::Expression(expr)))
    } else {
        None
    };
    compile_and_execute(session, raw_input, &fn_name, wrapper, true, hook)
}
```

- [ ] **Step 4: Run the single_execution tests — both must now PASS**

```bash
cargo test -p ruxen_repl --lib single_execution -- --nocapture
```
Expected: 2 PASS.

- [ ] **Step 5: Run the full REPL suite — every state-persistence test must STILL pass via the slot path**

```bash
cargo test -p ruxen_repl --lib
```
Expected: all PASS. If a state-persistence test fails here, the slot path from Phase 1/2 has a gap — fix it before continuing.

- [ ] **Step 6: Commit**

```bash
git add src/ruxen_repl/src/eval.rs src/ruxen_repl/src/tests/single_execution.rs
git commit -m "repl: stop replaying all_statements in eval_expression

Wrapper now contains only the new statement. Session-var reads go
through the slot load path installed in Phase 1; def/type-item
visibility unchanged (those are pure declarations). Side effects
fire exactly once — closes the bug class behind 508, 534, 536,
727, 727b in the REPL parity sweep."
```

### Task 3.2: Remove replay from `eval_statement::Let` and `eval_statement::Expression`

**Files:**
- Modify: `src/ruxen_repl/src/eval.rs:152-220`

- [ ] **Step 1: Add a failing test that mutates the same name twice across inputs**

```rust
// src/ruxen_repl/src/tests/single_execution.rs — append:

#[test]
fn let_rebind_does_not_double_run_rhs() {
    let outs = run_session(&[
        r#"let mut log = String.from("")"#,
        r#"log = log + "a""#,
        r#"log = log + "b""#,
        r#"log"#,
    ]);
    // Without the fix, replay would re-append "a" before "b" on
    // input 2, producing "aab" then "aabb".
    assert!(outs[3].contains(r#""ab""#) || outs[3].contains("ab"),
            "got: {:?}", outs);
}
```

- [ ] **Step 2: Confirm RED**

```bash
cargo test -p ruxen_repl --lib let_rebind_does_not_double_run_rhs
```
Expected: FAIL with "aab" or "aabb".

- [ ] **Step 3: Apply the same no-replay pattern to `eval_statement` arms**

```rust
// src/ruxen_repl/src/eval.rs — in eval_statement, replace:
//   let mut statements: Vec<Statement> = session.all_statements.clone();
//   statements.push(Statement::Let(binding.clone()));
//   …
// with the no-replay version:
let statements: Vec<Statement> = vec![Statement::Let(binding.clone())];
// (Then the trailing identifier-read for the display path stays.)
```
Apply the equivalent change to the `Statement::Expression` arm immediately below.

- [ ] **Step 4: GREEN check**

```bash
cargo test -p ruxen_repl --lib
```
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ruxen_repl/src/eval.rs src/ruxen_repl/src/tests/single_execution.rs
git commit -m "repl: stop replaying all_statements in eval_statement

Mirrors Phase 3.1 for the let-binding and expression-statement
paths. Combined with 3.1 this removes ALL all_statements.clone()
call sites; the field becomes vestigial."
```

### Task 3.3: Delete the now-dead replay scaffolding

**Files:**
- Modify: `src/ruxen_repl/src/session.rs` — drop `all_statements: Vec<Statement>` and `prev_captured_output: String`
- Modify: `src/ruxen_repl/src/eval.rs` — drop the line-count suffix logic at lines 620-652
- Modify: `src/ruxen_repl/src/capture.rs` — `puts`/`print` shims now write straight to `stdout()` (per design Q1)
- Delete (or shrink to no-op): `src/ruxen_repl/src/capture.rs` — see step below

- [ ] **Step 1: Rip out the unused fields and their references**

```bash
# Find every reference to the dead fields.
grep -rn "all_statements\|prev_captured_output\|capture::take_all\|capture::clear" \
  src/ruxen_repl/src/ compiler/ruxen_core/src/
```
Expected: every reference outside of the `eval.rs` accumulation sites is gone. If any survive, follow them and remove.

- [ ] **Step 2: Rewrite the `puts` / `print` shims to flush to real stdout**

```rust
// src/ruxen_repl/src/capture.rs — replace the BUFFER-append version with:
#[no_mangle]
pub extern "C" fn ruxen_repl_puts_shim(s: *const std::ffi::c_char) {
    use std::io::Write;
    let bytes = if s.is_null() {
        b"(nil)\n".to_vec()
    } else {
        let cs = unsafe { std::ffi::CStr::from_ptr(s) };
        let mut v = cs.to_bytes().to_vec();
        v.push(b'\n');
        v
    };
    let stdout = std::io::stdout();
    let _ = stdout.lock().write_all(&bytes);
}
// Apply the same pattern to ruxen_repl_print_shim,
// ruxen_repl_eputs_shim (→ stderr), ruxen_repl_print_int_shim,
// ruxen_repl_print_float_shim.
```

The BUFFER static and `take_all`/`clear`/`append` helpers can now be deleted.

- [ ] **Step 3: Build clean + run all REPL tests + run the targeted release-e2e fixtures**

```bash
cargo build -p ruxen_repl 2>&1 | tail -5
cargo test -p ruxen_repl --lib
rm -f /tmp/ruxen_e2e_*.txt
PHASES=repl CASE_FILTER='508_command_status.rx' \
  RUXEN_WORKSPACE=$PWD ./tests/release-e2e/run.sh 2>&1 | tail -5
```
Expected: build green, REPL unit tests all PASS, fixture 508 PASS.

- [ ] **Step 4: Commit**

```bash
git add -- src/ruxen_repl/src/session.rs src/ruxen_repl/src/eval.rs src/ruxen_repl/src/capture.rs
git commit -m "repl: delete all_statements + capture workaround

Removes:
  - ReplSession::all_statements (cumulative side-effect history)
  - ReplSession::prev_captured_output (line-count diff state)
  - capture::BUFFER + take_all/clear (cumulative output buffer)

Replaces the puts/print shims with direct stdout writes — subprocess
stdout and ruxen puts now follow the same path, matching the
compile-and-run contract.

Fixture 508_command_status now passes in PHASES=repl."
```

---

## Phase 4 — Async executor persistence across inputs

The design call (Q2 = "persistent executor") means a `task::spawn` in input N must still be alive in input N+1. The C runtime already exposes a process-global executor via `library/std/future/runtime/executor.c`. Verify the REPL JIT doesn't reset it between inputs, and add a test.

### Task 4.1: Add a persistent-executor test

**Files:**
- Create: `src/ruxen_repl/src/tests/async_persistence.rs`
- Modify: `src/ruxen_repl/src/tests/mod.rs`

- [ ] **Step 1: Write the test**

```rust
// src/ruxen_repl/src/tests/async_persistence.rs
use super::state_persistence::run_session;

#[test]
fn task_spawned_in_prior_input_can_be_joined_later() {
    let outs = run_session(&[
        "use std.future.task",
        "let handle = task.spawn({ async { 7 } })",
        "block_on(handle.join)",
    ]);
    assert!(outs[2].contains("7"), "got: {:?}", outs);
}
```

```rust
// src/ruxen_repl/src/tests/mod.rs — append:
mod async_persistence;
```

- [ ] **Step 2: Run the test**

```bash
cargo test -p ruxen_repl --lib task_spawned_in_prior_input_can_be_joined_later -- --nocapture
```
- If GREEN: the executor already persists; skip to Step 5.
- If RED: continue to Step 3.

- [ ] **Step 3: Identify whether the runtime resets executor state between JIT modules**

```bash
grep -rn "ruxen_executor_init\|ruxen_executor_reset\|executor.*reset\|EXECUTOR.*static" \
  library/std/future/runtime/ src/ruxen_repl/src/
```
Likely findings: an `init` called from a constructor or `block_on` lazily on first use. If `block_on` re-initializes state each call, change to lazy-once-per-process via `pthread_once` or `__attribute__((constructor))`.

- [ ] **Step 4: Patch the runtime to make the executor process-lifetime**

Concrete change shape (in `library/std/future/runtime/executor.c`):

```c
/* Use a one-shot init so the executor lives for the whole process,
 * not for the duration of a single block_on call. REPL inputs each
 * trigger a fresh JIT module, but the runtime symbols (incl. the
 * executor's static state) live in the host binary, so this is
 * sufficient — no per-input reset is required, just don't
 * reinitialize. */
static pthread_once_t g_executor_once = PTHREAD_ONCE_INIT;
static void executor_init_once(void) { /* …existing init body… */ }

void ruxen_executor_ensure(void) {
    pthread_once(&g_executor_once, executor_init_once);
}
/* Replace the eager init at the top of ruxen_block_on / spawn with
 * a call to ruxen_executor_ensure(). */
```

Rebuild and re-run the test.

- [ ] **Step 5: Run the 727 / 727b fixtures**

```bash
rm -f /tmp/ruxen_e2e_*.txt
PHASES=repl CASE_FILTER='727_async_tcp_echo.rx|727b_async_tcp_read_timeout.rx' \
  RUXEN_WORKSPACE=$PWD ./tests/release-e2e/run.sh 2>&1 | tail -5
```
Expected: both PASS.

- [ ] **Step 6: Commit**

```bash
git add -- library/std/future/runtime/ src/ruxen_repl/src/tests/
git commit -m "repl(async): persistent executor across REPL inputs

Spawned tasks now survive across input boundaries. Closes the bug
behind 727_async_tcp_echo and 727b_async_tcp_read_timeout in the
REPL parity sweep."
```

---

## Phase 5 — Filesystem-fixture verification

`534_fs_canonicalize` and `536_fs_read_link` should pass naturally once Phase 3 removes replay (their failure was the `symlink()` returning `EEXIST` on the second replay). Lock this in with a focused fixture run + a unit test.

### Task 5.1: Direct fixture verification

- [ ] **Step 1: Wipe any leftover /tmp state from prior interrupted runs**

```bash
rm -f /tmp/ruxen_e2e_534_*.txt /tmp/ruxen_e2e_536_*.txt
```

- [ ] **Step 2: Run both fixtures in isolation**

```bash
PHASES=repl CASE_FILTER='534_fs_canonicalize.rx|536_fs_read_link.rx' \
  RUXEN_WORKSPACE=$PWD ./tests/release-e2e/run.sh 2>&1 | tail -5
```
Expected: 2 PASS, 0 FAIL.

- [ ] **Step 3: Add a REPL-level unit test for the failure shape, so a future regression doesn't reintroduce it silently**

```rust
// src/ruxen_repl/src/tests/single_execution.rs — append:

#[test]
fn filesystem_setup_runs_once_not_twice() {
    use std::fs;
    let tmp = std::env::temp_dir()
        .join(format!("ruxen_repl_fs_test_{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let tmp_s = tmp.to_string_lossy().to_string();

    let outs = run_session(&[
        &format!(r#"use std.fs"#),
        &format!(r#"fs.write("{}", "hi").expect!("write")"#, tmp_s),
        &format!(r#"fs.read_to_string("{}").expect!("read").length"#, tmp_s),
    ]);
    // Without the fix, the second input's replay would re-run write()
    // — fine on its own, but a SYMLINK rather than a write would EEXIST.
    // Read length 2 ("hi") is the post-write assertion.
    assert!(outs[2].contains("2"), "got: {:?}", outs);
    let _ = fs::remove_file(&tmp);
}
```

- [ ] **Step 4: Run + commit**

```bash
cargo test -p ruxen_repl --lib filesystem_setup_runs_once_not_twice
git add src/ruxen_repl/src/tests/single_execution.rs
git commit -m "test(repl): pin once-only filesystem setup behaviour"
```

---

## Phase 6 — Final integration verification

### Task 6.1: Full acceptance bar

- [ ] **Step 1: Workspace tests**

```bash
cargo test --workspace --no-fail-fast 2>&1 | tail -20
```
Expected: 0 failures.

- [ ] **Step 2: All 5 historically-failing fixtures via PHASES=repl**

```bash
rm -f /tmp/ruxen_e2e_*.txt
PHASES=repl CASE_FILTER='508_command_status.rx|534_fs_canonicalize.rx|536_fs_read_link.rx|727_async_tcp_echo.rx|727b_async_tcp_read_timeout.rx' \
  RUXEN_WORKSPACE=$PWD ./tests/release-e2e/run.sh 2>&1 | tail -5
```
Expected: 5/5 PASS.

- [ ] **Step 3: Full release-e2e sweep**

```bash
rm -f /tmp/ruxen_e2e_*.txt
RUXEN_WORKSPACE=$PWD ./tests/release-e2e/run.sh 2>&1 | tail -10
```
Expected: `fail: 0` in the bottom line.

- [ ] **Step 4: Interactive sanity (~2 min by hand)**

Open a real `ruxen repl`. Run:
- `let mut x = 0` ; `x += 1` ; `x` → prints `1`
- `def double(n: Int) -> Int; n * 2; end` ; `double(21)` → prints `42`
- `let now = Instant.now` ; (wait 2s) ; `(Instant.now - now).millis` → some non-trivial number (i.e. `now` was NOT re-evaluated)

- [ ] **Step 5: Update CHANGELOG and commit**

```bash
cat > /tmp/changelog-frag <<'EOF'
- `ruxen repl`: rewritten to execute each user input's side-effecting
  statements exactly once. Variable state is preserved across inputs
  via the persistent slot table (already declared on `ReplSession`)
  rather than by replaying the cumulative statement history. Subprocess
  stdout, filesystem writes, and async-task spawns now match
  compile-and-run semantics in the REPL. Fixes the 5 release-e2e
  REPL-parity fixtures (508, 534, 536, 727, 727b).
EOF
# Hand-edit CHANGELOG.md under ## [Unreleased] to include the bullet above.
git add CHANGELOG.md
git commit -m "docs(changelog): REPL state refactor"
```

---

## Risk register

- **R1 — MIR `original_name` plumbing scope.** Phase 1.2 requires every identifier-reading MIR site to carry the source name through to the JIT. If the MIR lowering is opaque about which names are session-vars vs lexical locals, expect 1-2 extra subtasks under Phase 1 to disambiguate. Mitigation: a single `mir_local.original_name: Option<String>` plus a "first ref wins" rule keeps the change localized.
- **R2 — Closure captures.** Slot loads inside a closure body need the same baked-in address treatment. If the JIT closure path does its own LocalId management, expect a Phase 1.4-style task to extend the slot path there too. Mitigation: Phase 2 includes an Array/HashMap test that exercises `.each { |v| … }` — closure capture is the natural place that breaks; surface early.
- **R3 — `Instant.now` non-determinism.** The session.rs slot comment claims slots already prevent re-evaluation of `Instant.now`. Verify with an interactive smoke at Phase 6 Step 4 — if it regresses, Phase 3 inadvertently changed which expressions count as side-effecting.
- **R4 — Async executor lifetime mismatch.** Phase 4 assumes the C-runtime executor is process-global. If it's actually scoped per `block_on` call today, Step 4 of Task 4.1 expands into a larger runtime refactor (multi-day). Mitigation: validate the grep in Task 4.1 Step 3 before committing to the rest of Phase 4 scope.

## Out of scope

- Removing the `func_defs` and `type_items` replay. Those are pure declarations replayed only for typecheck/resolve to see prior `def`s and `class`es; they execute nothing. They stay.
- REPL multi-line continuation UI behaviour (handled by `validate.rs` and rustyline; unaffected).
- Performance optimisation of the slot load/store path. The current i64 cell + `load.i64` from absolute address is fine for v1; profile if it becomes a bottleneck.
