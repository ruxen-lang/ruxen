# Phase 2 — `RuntimeAbi` table: one `callee_ownership` lookup (drops + method_call) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`)
> syntax for tracking. This is Phase 2 of `2026-06-04-thermonuke-master.md`.

**Goal:** Replace the six overlapping, self-contradicting string-literal classifier sets in
`mir/lower/drops.rs` (`FRESH_ALLOC_CALLEES` ~110 entries, `is_runtime_consume_helper`,
`is_runtime_borrow_helper` rebound 3×, `is_move_by_ffi_callee`, `is_pointer_store_helper`,
`borrows_first_arg`, plus `transfer_indices`) **and** the duplicated static-constructor knowledge
spread across `mir/lower/expr/method_call.rs` and `mir/lower/util.rs` with ONE data-driven
`fn callee_ownership(callee: &str) -> CalleeOwnership` sourced from a single declarative table in a
new file `mir/lower/runtime_abi.rs`.

**Architecture:** A single source of truth for "what does this runtime callee do with its result and
its arguments." The current `Call` arm in `drops.rs` answers that question by funnelling the callee
name through six independent, partly-contradicting `matches!`/`contains`/`starts_with` predicates whose
combination has already produced one documented contradiction (`ruxen_string_into_bytes` is in BOTH
`FRESH_ALLOC_CALLEES` *and* `is_runtime_consume_helper` — it is simultaneously claimed to return fresh
heap AND to consume its arg; the second-arg loop only ever reads it as a consume-helper, so the
`FRESH_ALLOC_CALLEES` membership is dead and misleading). One `callee_ownership(&str) -> CalleeOwnership
{ result: ResultOwnership, arg_transfer: ArgMask }` collapses all six into one ordered classification
whose precedence is explicit in one place. **This is a UAF/double-free-class change** (master risk
register, Phase 2 row): a wrong table entry is a double-free or use-after-free. Therefore Task 1 is a
characterization/parity harness that captures EVERY current predicate's answer for EVERY symbol any of
the six sets mentions; the table is then asserted to reproduce all those answers 1:1, and no symbol may
change category without a visible test diff.

**Tech Stack:** Rust 1.91, `cargo test -p ruxen_core`. No new dependencies.

> **Per-task direction-check (maintainer-mandated):** after the commit step of EVERY task below, run
> the `thermonuke` skill scoped to that task's diff: invoke it with arg
> `git diff HEAD~1..HEAD` and confirm (a) lines moved in the intended direction (net reduction once the
> port tasks land), (b) **no new `_ =>` catch-all** in the table or the `Call` arm (the table's
> `_ => CalleeOwnership::default()` fallthrough is the ONE deliberate, documented default and must not
> proliferate), (c) no new god-predicate or special-case re-introduced in `drops.rs`, (d) the task's
> structural goal was met — **a classifier set was DELETED, not added**. If it flags drift, STOP and
> surface it. The full multi-agent sweep runs in Task 7. Each task's checkbox list ends with a
> `- [ ] Direction-check (/thermonuke on this task's diff)` step.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `compiler/ruxen_core/src/mir/lower/runtime_abi.rs` | `CalleeOwnership` / `ResultOwnership` / `ArgMask` types + the single declarative table + `callee_ownership` | **Create** |
| `compiler/ruxen_core/src/mir/lower/mod.rs` | lowerer module tree | Modify (add `pub(crate) mod runtime_abi;`) |
| `compiler/ruxen_core/src/mir/lower/drops.rs` | `Call` arm (`676-1269`): replace the 6 sets + `transfer_indices` with one lookup | Modify (delete sets, call table) |
| `compiler/ruxen_core/src/mir/lower/expr/method_call.rs` | static-ctor fast-path booleans (`162-214`) + the duplicated class list (`276-328`) | Modify (route through table) |
| `compiler/ruxen_core/src/mir/lower/util.rs` | `is_builtin_static_method` (`74-132`) — the divergent duplicate | Modify (delegate to table) |

**Module placement rationale:** `runtime_abi.rs` lives in `mir/lower/` because every consumer
(`drops.rs`, `expr/method_call.rs`, `util.rs`) is under that subtree, and the data it encodes is the
runtime ABI contract the lowerer enforces (the contract document is `ABI.md` / the per-spec stdlib
docs already cited in the existing comments). One file owns the obligation: "every runtime symbol the
lowerer special-cases appears here exactly once, with one classification."

---

## Task 1: Characterization harness — capture every predicate's answer for every symbol

**Files:**
- Test: create `compiler/ruxen_core/tests/runtime_abi_parity.rs`
- Read-only this task: `mir/lower/drops.rs:676-1269`, `util.rs:74-132`, `method_call.rs:162-214`

This task introduces NO production code. It pins the current behaviour of the six `drops.rs`
predicates + `transfer_indices` + the two static-ctor classifiers so the later port tasks have a
red→green parity oracle. Because the predicates are currently *local `let` bindings inside the `Call`
match arm* (not callable functions), the harness re-encodes them as free reference functions in the
test file, transcribed verbatim from the source, and the parity assertion in Task 3 will compare the
**table** against these reference functions.

- [ ] **Step 1: Enumerate the universe of symbols**

Create `compiler/ruxen_core/tests/runtime_abi_parity.rs`. At the top, build one `const SYMBOLS: &[&str]`
that is the UNION of every string literal mentioned by any of the six sets + `transfer_indices` + the
static-ctor lists. Transcribe them directly from source — do not summarise:

> **Implementer obligation (explicit sub-step, not a placeholder):** open `drops.rs` and copy, verbatim,
> every `&str` literal appearing in: `FRESH_ALLOC_CALLEES` (`694-934`), `borrows_first_arg` (`967-984`),
> `is_runtime_consume_helper` (`1003-1043`), `is_move_by_ffi_callee` (`1068-1088`),
> `is_command_terminal_or_accessor` (`1123-1139`), `is_command_builder_method` (`1158-1168`),
> `is_pointer_store_helper` (`1180`), and the `transfer_indices` method-name cases (`1232-1245`). Then
> add the prefix-probe witnesses for `is_runtime_borrow_helper` (`1091-1107`): at minimum one symbol per
> prefix (`ruxen_vec_push`, `Vec_push`, `Vec[String]_push`, `Array_len`, `Hash_get`, `HashMap_insert`,
> `Map_get`, `Set_contains`, `HashSet_insert`, `String_len`, `&str_len`) plus a user-callee witness
> (`Point_new`, `MyClass_method`) and a `_init` witness (`Point_init`). This SYMBOLS list IS the audit
> surface — if a symbol isn't here, the parity test can't catch a regression on it.

- [ ] **Step 2: Transcribe the six predicates as reference functions**

In the same test file, transcribe each predicate verbatim as a free `fn` taking `callee: &str`. These
are the GOLDEN oracle — they must be byte-identical logic to the `let` bindings in `drops.rs`. Encode
the *final* rebound value of `is_runtime_borrow_helper` (after all three rebinds at `1089`, `1140`,
`1169`), i.e. the effective predicate:

```rust
// VERBATIM transcription of drops.rs Call-arm predicates as of <commit sha>.
// These are the parity oracle. Do NOT "clean them up" — they must reproduce
// the exact current answers, contradictions included, so Task 3 can prove the
// table matches before Task 3b is allowed to intentionally change any answer.

fn ref_returns_fresh_alloc(c: &str) -> bool {
    // transcribe FRESH_ALLOC_CALLEES.contains(&c) — paste the full slice.
    const FRESH_ALLOC_CALLEES: &[&str] = &[/* paste 694-934 verbatim */];
    FRESH_ALLOC_CALLEES.contains(&c)
}

fn ref_borrows_first_arg(c: &str) -> bool {
    c.ends_with("_init")
        || c == "ruxen_dealloc"
        || c == "ruxen_string_free"
        || c == "ruxen_vec_free"
        || c == "ruxen_vec_drop_string"
        || c == "ruxen_vec_drop_vec"
        || c == "ruxen_hash_free"
        || c == "ruxen_set_free"
        || c == "ruxen_hash_drop_string_v"
        || c == "ruxen_hash_drop_v_string"
        || c == "ruxen_hash_drop_string_string"
        || c == "ruxen_hash_drop_v_vec"
        || c == "ruxen_set_drop_string"
}

fn ref_is_runtime_consume_helper(c: &str) -> bool {
    matches!(c,
        "ruxen_dealloc" | "ruxen_string_free" | "ruxen_vec_free"
        | "ruxen_vec_drop_string" | "ruxen_vec_drop_vec" | "ruxen_hash_free"
        | "ruxen_set_free" | "ruxen_hash_drop_string_v" | "ruxen_hash_drop_v_string"
        | "ruxen_hash_drop_string_string" | "ruxen_hash_drop_v_vec" | "ruxen_set_drop_string"
        | "ruxen_string_into_bytes" | "String_into_bytes"
        | "ruxen_vec_from_iter" | "Vec_from_iter")
}

fn ref_is_move_by_ffi_callee(c: &str) -> bool {
    matches!(c,
        "ruxen_executor_spawn" | "Task_spawn_raw"
        | "ruxen_thread_spawn" | "Thread_spawn" | "Thread_spawn_raw")
}

fn ref_is_command_terminal_or_accessor(c: &str) -> bool {
    matches!(c,
        "Command_status" | "ruxen_command_status" | "Command_output" | "ruxen_command_output"
        | "Output_status" | "ruxen_output_status" | "Output_stdout" | "ruxen_output_stdout"
        | "Output_stderr" | "ruxen_output_stderr" | "ExitStatus_code" | "ruxen_exit_status_code"
        | "ExitStatus_success" | "ruxen_exit_status_success")
}

fn ref_is_command_builder_method(c: &str) -> bool {
    matches!(c,
        "Command_arg" | "ruxen_command_arg" | "Command_args" | "ruxen_command_args"
        | "Command_env" | "ruxen_command_env" | "Command_current_dir" | "ruxen_command_current_dir")
}

fn ref_is_pointer_store_helper(c: &str) -> bool { c == "ruxen_store_ptr" }

// Effective is_runtime_borrow_helper AFTER all three rebinds (1089/1140/1169):
//   base = !consume && !move_by_ffi && (prefix match)
//   then ||= command_terminal_or_accessor
//   then &&= !command_builder_method
fn ref_is_runtime_borrow_helper(c: &str) -> bool {
    let base = !ref_is_runtime_consume_helper(c)
        && !ref_is_move_by_ffi_callee(c)
        && (c.starts_with("ruxen_")
            || c.starts_with("Vec_") || c.starts_with("Vec[")
            || c.starts_with("Array_") || c.starts_with("Array[")
            || c.starts_with("Hash_") || c.starts_with("Hash[")
            || c.starts_with("HashMap_") || c.starts_with("HashMap[")
            || c.starts_with("Map_") || c.starts_with("Map[")
            || c.starts_with("Set_") || c.starts_with("Set[")
            || c.starts_with("HashSet_") || c.starts_with("HashSet[")
            || c.starts_with("String_") || c.starts_with("&str_"));
    let with_terminal = base || ref_is_command_terminal_or_accessor(c);
    with_terminal && !ref_is_command_builder_method(c)
}

fn ref_transfer_indices(c: &str) -> &'static [usize] {
    use ruxen_core::codegen::runtime::extract_method_name;
    let m = extract_method_name(c);
    let is_vec = c.starts_with("Vec_") || c.starts_with("Vec[")
        || c.starts_with("Array_") || c.starts_with("Array[");
    let is_hash = c.starts_with("Hash_") || c.starts_with("Hash[")
        || c.starts_with("HashMap_") || c.starts_with("HashMap[")
        || c.starts_with("Map_") || c.starts_with("Map[");
    let is_set = c.starts_with("Set_") || c.starts_with("Set[")
        || c.starts_with("HashSet_") || c.starts_with("HashSet[");
    match (is_vec, is_hash, is_set, m) {
        (true, _, _, "push") => &[1],
        (true, _, _, "insert") => &[2],
        (_, true, _, "insert") => &[1, 2],
        (_, _, true, "insert") => &[1],
        _ if c == "ruxen_vec_push" => &[1],
        _ if c == "ruxen_vec_insert" => &[2],
        _ if c == "ruxen_hash_insert" => &[1, 2],
        _ if c == "ruxen_set_insert" => &[1],
        _ => &[],
    }
}
```

> Verify `ruxen_core::codegen::runtime::extract_method_name` is `pub` (it is `pub use`d at
> `drops.rs:1216` via `use crate::codegen::runtime::extract_method_name`). If it is only `pub(crate)`,
> the parity reference for `transfer_indices` must instead live in an inline `#[cfg(test)]` module in
> `runtime_abi.rs` (Task 3) rather than the integration test; note that and adjust.

- [ ] **Step 3: Write the contradiction-witness test (documents the known bug)**

Add a test that ASSERTS the current contradiction so the port task's behaviour change is visible:

```rust
#[test]
fn documents_into_bytes_fresh_alloc_vs_consume_contradiction() {
    // ruxen_string_into_bytes is in BOTH FRESH_ALLOC_CALLEES and
    // is_runtime_consume_helper. The arg loop only reads consume; the
    // FRESH_ALLOC membership is dead+misleading. Pin the CURRENT answers
    // so Task 3b's resolution (one category) shows up as a test diff.
    assert!(ref_returns_fresh_alloc("ruxen_string_into_bytes"));
    assert!(ref_is_runtime_consume_helper("ruxen_string_into_bytes"));
    assert!(ref_returns_fresh_alloc("String_into_bytes"));
    assert!(ref_is_runtime_consume_helper("String_into_bytes"));
}
```

- [ ] **Step 4: Run to confirm the harness compiles and the witness passes**

Run: `cargo test -p ruxen_core --test runtime_abi_parity 2>&1 | tee tmp/test-cache/phase2-task1-green.log`
Expected: PASS (the contradiction witness + any reference-sanity asserts). There is no production code
yet, so there is no red here — this task's deliverable is the oracle. (The red→green parity assertion
against the table arrives in Task 3, Step 1.)

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/tests/runtime_abi_parity.rs
git commit -m "test(mir): characterization oracle for runtime callee ownership

Transcribe the 6 drops.rs Call-arm predicates + transfer_indices verbatim as
reference fns over a union SYMBOLS list, and pin the documented
ruxen_string_into_bytes fresh-vs-consume contradiction. This is the parity
oracle the upcoming runtime_abi table must reproduce 1:1; no symbol may change
category without a visible diff to this file.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — invoke `thermonuke` with
  `git diff HEAD~1..HEAD`. Confirm: pure test addition, no production `_ =>` introduced, no classifier
  set deleted yet (that is Task 3). This task adds the oracle that licenses later deletions.

---

## Task 2: Define `CalleeOwnership` + the declarative table (no consumer yet)

**Files:**
- Create: `compiler/ruxen_core/src/mir/lower/runtime_abi.rs`
- Modify: `compiler/ruxen_core/src/mir/lower/mod.rs` (add `pub(crate) mod runtime_abi;`)
- Test: inline `#[cfg(test)] mod tests` in `runtime_abi.rs`

- [ ] **Step 1: Write the failing test (table parity vs the oracle's CONSUME/BORROW/FRESH answers)**

End `runtime_abi.rs` with an inline test that asserts the table reproduces the three result/arg
classifications for a representative slice (the full union-parity check lives in Task 3, which can see
the integration-test oracle; this inline test is the fast unit gate):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_alloc_constructors_classify_as_fresh() {
        assert_eq!(callee_ownership("Vec_new").result, ResultOwnership::Fresh);
        assert_eq!(callee_ownership("ruxen_hash_new").result, ResultOwnership::Fresh);
        assert_eq!(callee_ownership("Command_new").result, ResultOwnership::Fresh);
        assert_eq!(callee_ownership("File_open").result, ResultOwnership::Fresh);
        assert_eq!(callee_ownership("Mutex_lock_raw").result, ResultOwnership::Fresh);
    }

    #[test]
    fn free_family_consumes_first_arg_and_returns_nothing() {
        let o = callee_ownership("ruxen_string_free");
        assert_eq!(o.result, ResultOwnership::None);
        // free helpers BORROW arg0 in the drop-pass sense (the arg loop
        // `continue`s on arg0 so it is NOT tainted) — encoded as ArgMask::none()
        // gated by the borrows_first_arg flag. See the field doc.
        assert!(o.borrows_first_arg);
    }

    #[test]
    fn vec_push_transfers_arg1() {
        assert_eq!(callee_ownership("ruxen_vec_push").arg_transfer, ArgMask::single(1));
        assert_eq!(callee_ownership("Vec_push").arg_transfer, ArgMask::single(1));
    }

    #[test]
    fn hashmap_insert_transfers_arg1_and_arg2() {
        assert_eq!(callee_ownership("ruxen_hash_insert").arg_transfer, ArgMask::pair(1, 2));
        assert_eq!(callee_ownership("HashMap_insert").arg_transfer, ArgMask::pair(1, 2));
    }

    #[test]
    fn move_by_ffi_does_not_borrow_args() {
        let o = callee_ownership("ruxen_executor_spawn");
        assert!(!o.args_are_borrowed); // default-taint path runs → spawned future moved
    }

    #[test]
    fn pointer_store_transfers_arg1() {
        assert_eq!(callee_ownership("ruxen_store_ptr").arg_transfer, ArgMask::single(1));
    }

    #[test]
    fn user_callee_is_fully_conservative() {
        let o = callee_ownership("MyClass_method");
        assert_eq!(o.result, ResultOwnership::None); // dest tainted
        assert!(!o.args_are_borrowed);               // every Use(arg) tainted
        assert!(!o.borrows_first_arg);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ruxen_core --lib mir::lower::runtime_abi 2>&1 | tee tmp/test-cache/phase2-task2-red.log`
Expected: FAIL to compile — `callee_ownership` / `CalleeOwnership` / `ResultOwnership` / `ArgMask` not
found.

- [ ] **Step 3: Write the types + the table + `callee_ownership`**

At the top of `compiler/ruxen_core/src/mir/lower/runtime_abi.rs`:

```rust
//! The single source of truth for runtime-callee ownership in MIR lowering.
//!
//! Every runtime symbol the drop-elaboration pass (`drops.rs`) or the
//! static-constructor fast path (`expr/method_call.rs`) special-cases is
//! classified HERE, exactly once, by `callee_ownership`. This replaces the six
//! overlapping, partly-contradicting predicates that previously lived inline in
//! the `drops.rs` `Call` arm (FRESH_ALLOC_CALLEES, is_runtime_consume_helper,
//! is_runtime_borrow_helper ×3 rebinds, is_move_by_ffi_callee,
//! is_pointer_store_helper, borrows_first_arg) plus the duplicated static-ctor
//! lists in method_call.rs / util.rs.
//!
//! ABI contract: the C side's ownership semantics are documented per-symbol in
//! the stdlib specs cited inline (task_spawn.spec.md §B10, the Command builder
//! notes, mutex.c, scheduler.c:150-152). A WRONG entry here is a double-free or
//! use-after-free — every change must move a row, with a visible diff to the
//! parity oracle (tests/runtime_abi_parity.rs).

use crate::codegen::runtime::extract_method_name;

/// What the callee does with the value it RETURNS into `dest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultOwnership {
    /// Returns a fresh heap allocation owned exclusively by `dest`
    /// (drop-elaborate `dest` at scope exit). Old: FRESH_ALLOC_CALLEES.
    Fresh,
    /// No owning result to track (Unit/Int return, or a free helper).
    /// `dest` is conservatively tainted (dropped from alloc_rooted).
    None,
}

/// Which positional arguments have their ownership TRANSFERRED to the callee
/// (the arg's local must be tainted / removed from `alloc_rooted`). Small,
/// bounded set — at most arg0..arg2 ever transfer in v1, so a u8 bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ArgMask(u8);

impl ArgMask {
    pub(crate) const fn none() -> Self { ArgMask(0) }
    pub(crate) const fn single(i: usize) -> Self { ArgMask(1 << i) }
    pub(crate) const fn pair(i: usize, j: usize) -> Self { ArgMask((1 << i) | (1 << j)) }
    pub(crate) fn contains(self, i: usize) -> bool { self.0 & (1 << i) != 0 }
}

/// The full ownership verdict for one callee. Mirrors the four decisions the
/// `drops.rs` arg loop makes, in the SAME precedence order it applied them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CalleeOwnership {
    /// Result-into-`dest` ownership.
    pub(crate) result: ResultOwnership,
    /// arg0 is borrowed even though the callee otherwise transfers (the
    /// `borrows_first_arg` rule: `_init` ctors + the free/drop family). The
    /// arg loop `continue`s on arg0 when this is set. HIGHEST arg precedence.
    pub(crate) borrows_first_arg: bool,
    /// Args that are explicitly transferred regardless of `args_are_borrowed`
    /// (Vec/Hash/Set push/insert value+key slots; ruxen_store_ptr arg1). These
    /// are tainted before the borrow check. Old: transfer_indices +
    /// is_pointer_store_helper.
    pub(crate) arg_transfer: ArgMask,
    /// All remaining pointer args are BORROWED (not transferred) — the runtime
    /// helper reads/mutates in place. Old: effective is_runtime_borrow_helper.
    /// When false (user callees, move-by-FFI), every remaining Use(arg) is
    /// tainted. LOWEST arg precedence.
    pub(crate) args_are_borrowed: bool,
}

impl Default for CalleeOwnership {
    /// The conservative default for an unknown / user-defined callee: no owning
    /// result to track, every arg transferred (tainted). This is the ONE
    /// fallthrough — the table's `_ =>` arm — and is intentionally pessimistic
    /// (over-tainting a primitive temp is a no-op; under-tainting is a UAF).
    fn default() -> Self {
        CalleeOwnership {
            result: ResultOwnership::None,
            borrows_first_arg: false,
            arg_transfer: ArgMask::none(),
            args_are_borrowed: false,
        }
    }
}
```

Then the classifier. It preserves the EXACT precedence the old arm applied (consume/move-by-FFI
suppress borrow; command-builder suppresses borrow; transfer_indices override; pointer-store override;
borrows_first_arg override on arg0):

```rust
/// THE single ownership lookup. Ordered exactly like the old `Call` arm so the
/// parity oracle (tests/runtime_abi_parity.rs) reproduces 1:1.
pub(crate) fn callee_ownership(callee: &str) -> CalleeOwnership {
    // --- result ownership (old FRESH_ALLOC_CALLEES) ---
    let result = if FRESH_ALLOC_CALLEES.contains(&callee) {
        ResultOwnership::Fresh
    } else {
        ResultOwnership::None
    };

    // --- arg0 borrow (old borrows_first_arg) ---
    let borrows_first_arg = callee.ends_with("_init") || FREE_FAMILY.contains(&callee);

    // --- explicit per-arg transfers (old transfer_indices + pointer-store) ---
    let arg_transfer = arg_transfer_mask(callee);

    // --- remaining-arg borrow (old effective is_runtime_borrow_helper) ---
    let consumes = CONSUME_HELPERS.contains(&callee);
    let move_by_ffi = MOVE_BY_FFI.contains(&callee);
    let builder = COMMAND_BUILDER.contains(&callee);
    let prefix_borrow = has_runtime_prefix(callee);
    let terminal = COMMAND_TERMINAL_OR_ACCESSOR.contains(&callee);
    let args_are_borrowed =
        ((!consumes && !move_by_ffi && prefix_borrow) || terminal) && !builder;

    CalleeOwnership { result, borrows_first_arg, arg_transfer, args_are_borrowed }
}

fn arg_transfer_mask(callee: &str) -> ArgMask {
    if callee == "ruxen_store_ptr" {
        return ArgMask::single(1);
    }
    let m = extract_method_name(callee);
    let is_vec = callee.starts_with("Vec_") || callee.starts_with("Vec[")
        || callee.starts_with("Array_") || callee.starts_with("Array[");
    let is_hash = callee.starts_with("Hash_") || callee.starts_with("Hash[")
        || callee.starts_with("HashMap_") || callee.starts_with("HashMap[")
        || callee.starts_with("Map_") || callee.starts_with("Map[");
    let is_set = callee.starts_with("Set_") || callee.starts_with("Set[")
        || callee.starts_with("HashSet_") || callee.starts_with("HashSet[");
    match (is_vec, is_hash, is_set, m) {
        (true, _, _, "push") => ArgMask::single(1),
        (true, _, _, "insert") => ArgMask::single(2),
        (_, true, _, "insert") => ArgMask::pair(1, 2),
        (_, _, true, "insert") => ArgMask::single(1),
        _ if callee == "ruxen_vec_push" => ArgMask::single(1),
        _ if callee == "ruxen_vec_insert" => ArgMask::single(2),
        _ if callee == "ruxen_hash_insert" => ArgMask::pair(1, 2),
        _ if callee == "ruxen_set_insert" => ArgMask::single(1),
        _ => ArgMask::none(),
    }
}

fn has_runtime_prefix(c: &str) -> bool {
    c.starts_with("ruxen_")
        || c.starts_with("Vec_") || c.starts_with("Vec[")
        || c.starts_with("Array_") || c.starts_with("Array[")
        || c.starts_with("Hash_") || c.starts_with("Hash[")
        || c.starts_with("HashMap_") || c.starts_with("HashMap[")
        || c.starts_with("Map_") || c.starts_with("Map[")
        || c.starts_with("Set_") || c.starts_with("Set[")
        || c.starts_with("HashSet_") || c.starts_with("HashSet[")
        || c.starts_with("String_") || c.starts_with("&str_")
}
```

> **Implementer obligation (explicit sub-step):** define the five `const &[&str]` slices
> (`FRESH_ALLOC_CALLEES`, `FREE_FAMILY`, `CONSUME_HELPERS`, `MOVE_BY_FFI`, `COMMAND_BUILDER`,
> `COMMAND_TERMINAL_OR_ACCESSOR`) by transcribing the corresponding source literals VERBATIM:
> `FRESH_ALLOC_CALLEES` ← `drops.rs:694-934`; `FREE_FAMILY` ← the 13 `c == "..."` arms inside
> `borrows_first_arg` (`drops.rs:968-984`, excluding the `_init` suffix which stays as the
> `ends_with` check); `CONSUME_HELPERS` ← `drops.rs:1003-1043`; `MOVE_BY_FFI` ← `drops.rs:1068-1088`;
> `COMMAND_BUILDER` ← `drops.rs:1158-1168`; `COMMAND_TERMINAL_OR_ACCESSOR` ← `drops.rs:1123-1139`.
> These slices are NOT invented — they are a 1:1 move of existing literals into one file. Keep the
> `into_bytes` contradiction AS-IS in this task (it stays in both `FRESH_ALLOC_CALLEES` and
> `CONSUME_HELPERS`); resolving it is a deliberate, separately-committed change in Task 3b.

- [ ] **Step 3b: Register the module**

In `compiler/ruxen_core/src/mir/lower/mod.rs`, alongside the other `mod` decls:

```rust
pub(crate) mod runtime_abi;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ruxen_core --lib mir::lower::runtime_abi 2>&1 | tee tmp/test-cache/phase2-task2-green.log`
Expected: PASS (the 7 inline tests). No consumer is wired yet — `callee_ownership` is dead-but-tested.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/mir/lower/runtime_abi.rs compiler/ruxen_core/src/mir/lower/mod.rs
git commit -m "feat(mir): add runtime_abi callee_ownership table (no consumer yet)

Single CalleeOwnership { result, borrows_first_arg, arg_transfer,
args_are_borrowed } sourced from one declarative table. Verbatim 1:1 port of
the six drops.rs Call-arm predicates into one file; precedence preserved
exactly. The into_bytes fresh-vs-consume contradiction is carried over as-is
(resolved separately in the next commit). Wiring drops.rs to it is the next task.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — `git diff HEAD~1..HEAD`. This task is
  net-additive (new file) by design; confirm the table has exactly ONE `_ =>` (the `Default` impl +
  the two `_ => ArgMask::none()`/prefix fallthroughs which mirror existing arms) and that no NEW
  catch-all logic beyond the transcribed originals snuck in. The deletion lands in Task 3.

---

## Task 3: Port the `drops.rs` `Call` arm to one `callee_ownership` lookup (delete the 6 sets)

**Files:**
- Modify: `compiler/ruxen_core/src/mir/lower/drops.rs:676-1269`
- Modify: `compiler/ruxen_core/tests/runtime_abi_parity.rs` (add the full union-parity assertion)

- [ ] **Step 1: Write the failing full-union parity assertion**

Append to `tests/runtime_abi_parity.rs` — assert the public table reproduces every oracle answer for
every symbol in `SYMBOLS`:

```rust
use ruxen_core::mir::lower::runtime_abi::{callee_ownership, ArgMask, ResultOwnership};

#[test]
fn table_reproduces_every_oracle_answer_for_every_symbol() {
    for &c in SYMBOLS {
        let o = callee_ownership(c);

        // result == FRESH_ALLOC_CALLEES membership
        assert_eq!(
            o.result == ResultOwnership::Fresh, ref_returns_fresh_alloc(c),
            "result mismatch for {c:?}");

        // borrows_first_arg
        assert_eq!(o.borrows_first_arg, ref_borrows_first_arg(c),
            "borrows_first_arg mismatch for {c:?}");

        // args_are_borrowed == effective is_runtime_borrow_helper
        assert_eq!(o.args_are_borrowed, ref_is_runtime_borrow_helper(c),
            "args_are_borrowed mismatch for {c:?}");

        // arg_transfer == transfer_indices (plus pointer-store arg1)
        let mut expected = ArgMask::none();
        for &i in ref_transfer_indices(c) { expected = with_index(expected, i); }
        if ref_is_pointer_store_helper(c) { expected = with_index(expected, 1); }
        assert_eq!(o.arg_transfer, expected, "arg_transfer mismatch for {c:?}");
    }
}
```

> `runtime_abi::*` must be reachable from an integration test — add `pub(crate) mod runtime_abi;` →
> if integration tests can't see `pub(crate)`, change the `mod` decl in `mir/lower/mod.rs` to `pub mod
> runtime_abi;` and the type/field visibility to `pub` (the master plan allows no behaviour change;
> widening visibility for a parity test is acceptable, but prefer keeping the parity test as an inline
> `#[cfg(test)]` module inside `runtime_abi.rs` if widening is undesirable — implementer picks one and
> notes it). `with_index` is a 1-line test helper mirroring `ArgMask::single`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --test runtime_abi_parity 2>&1 | tee tmp/test-cache/phase2-task3-red.log`
Expected: PASS actually — the table was a verbatim port, so parity already holds. If it FAILS, a
transcription error exists in Task 2; fix Task 2's slices until this is green. (This "should-be-green"
parity is the SAFETY GATE: it proves the table is byte-equivalent to the old predicates BEFORE we
delete them.)

- [ ] **Step 3: Replace the six predicates + transfer_indices with the lookup**

In `drops.rs`, inside `MirInst::Call { dest, callee, args }` (`676`), DELETE the entire predicate
block (`FRESH_ALLOC_CALLEES` const + `returns_fresh_alloc` `935`, `borrows_first_arg` `967-984`,
`is_runtime_consume_helper` `1003-1043`, `is_move_by_ffi_callee` `1068-1088`,
`is_runtime_borrow_helper` and its 3 rebinds `1089-1170`, `is_command_terminal_or_accessor`,
`is_command_builder_method`, `is_pointer_store_helper` `1180`, `transfer_indices` `1215-1247`) and
replace with:

```rust
                    use crate::mir::lower::runtime_abi::{callee_ownership, ResultOwnership};
                    let abi = callee_ownership(callee.as_str());

                    if let Some(d) = dest {
                        if abi.result == ResultOwnership::Fresh && !tainted_perm.contains(d) {
                            alloc_rooted.insert(*d);
                        } else {
                            tainted_perm.insert(*d);
                            alloc_rooted.remove(d);
                        }
                    }
                    for (idx, arg) in args.iter().enumerate() {
                        if let MirValue::Use(l) = arg {
                            if abi.borrows_first_arg && idx == 0 {
                                continue;
                            }
                            if abi.arg_transfer.contains(idx) {
                                tainted_perm.insert(*l);
                                alloc_rooted.remove(l);
                                continue;
                            }
                            if abi.args_are_borrowed {
                                continue;
                            }
                            tainted_perm.insert(*l);
                            alloc_rooted.remove(l);
                        }
                    }
```

> This reproduces the old arg loop (`1248-1269`) exactly: arg0-borrow `continue` first, then explicit
> transfer (which now folds in `is_pointer_store_helper`'s `idx == 1` case via the `ArgMask`), then
> borrow `continue`, then default taint. The `dest` handling reproduces `936-943`. The large
> explanatory comment block that justified each old predicate moves to `runtime_abi.rs`'s doc comments
> — keep a 2-line pointer comment in `drops.rs` (`// Runtime callee ownership: see runtime_abi.rs.`).

- [ ] **Step 4: Run tests to verify green**

Run: `cargo test -p ruxen_core --test runtime_abi_parity 2>&1 | tee tmp/test-cache/phase2-task3-green.log`
Expected: PASS (parity holds — same oracle, now also driving production). Then the three pin tests:
- `cargo test -p ruxen_core --test drop_fixtures 2>&1 | tee tmp/test-cache/phase2-task3-drop.log`
- `cargo test -p ruxen_core --test task_spawn_ownership 2>&1 | tee tmp/test-cache/phase2-task3-spawn.log`
- `cargo test -p ruxen_core --test ffi_alias_single_entry 2>&1 | tee tmp/test-cache/phase2-task3-ffi.log`
Expected: all PASS — drop balance preserved (no new leak/double-free), spawn move semantics preserved.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/mir/lower/drops.rs compiler/ruxen_core/tests/runtime_abi_parity.rs
git commit -m "refactor(mir): drops.rs Call arm uses one callee_ownership lookup

Delete the 6 inline predicates (FRESH_ALLOC_CALLEES, is_runtime_consume_helper,
is_runtime_borrow_helper ×3 rebinds, is_move_by_ffi_callee,
is_pointer_store_helper, borrows_first_arg) + transfer_indices from the Call
arm; route through runtime_abi::callee_ownership. Parity test asserts 1:1
reproduction of every old answer for every symbol. No behaviour change — drop
balance + Task.spawn move semantics pinned green.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — `git diff HEAD~1..HEAD`. Confirm:
  large NET REDUCTION in `drops.rs` (~−400 in the Call arm), the six classifier blocks are GONE (not
  duplicated), no new `_ =>` introduced in `drops.rs`, the lookup is the single decision point.

---

## Task 3b: Resolve the `into_bytes` fresh-vs-consume contradiction (the one intentional behaviour change)

**Files:**
- Modify: `compiler/ruxen_core/src/mir/lower/runtime_abi.rs` (`FRESH_ALLOC_CALLEES`)
- Modify: `compiler/ruxen_core/tests/runtime_abi_parity.rs` (flip the witness)

This is the ONLY task in Phase 2 that intentionally changes a symbol's category, and it does so as a
visible test diff per the master risk-register mandate.

- [ ] **Step 1: Update the contradiction witness to assert the RESOLVED state**

In `tests/runtime_abi_parity.rs`, change `documents_into_bytes_..._contradiction` to assert the
resolved behaviour and rename it:

```rust
#[test]
fn into_bytes_is_consume_only_not_fresh_alloc() {
    // RESOLVED: ruxen_string_into_bytes / String_into_bytes consume their arg
    // (the runtime frees the source char* internally) and the dest aliases the
    // produced Vec — it is NOT an independent fresh allocation to additionally
    // root. Membership in FRESH_ALLOC_CALLEES was dead (the arg loop only read
    // the consume classification) AND misleading. Drop it.
    assert_eq!(callee_ownership("ruxen_string_into_bytes").result, ResultOwnership::None);
    assert_eq!(callee_ownership("String_into_bytes").result, ResultOwnership::None);
    // still consume-classified (args_are_borrowed false → arg tainted/moved):
    assert!(!callee_ownership("ruxen_string_into_bytes").args_are_borrowed);
}
```

Also update the oracle: in `ref_returns_fresh_alloc`'s transcribed `FRESH_ALLOC_CALLEES`, remove the
two `into_bytes` entries so the union-parity test still passes against the changed table.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --test runtime_abi_parity 2>&1 | tee tmp/test-cache/phase2-task3b-red.log`
Expected: FAIL — table still returns `Fresh` for the two `into_bytes` symbols.

- [ ] **Step 3: Remove the two `into_bytes` entries from the table's `FRESH_ALLOC_CALLEES`**

In `runtime_abi.rs`, delete `"String_into_bytes"` and `"ruxen_string_into_bytes"` from
`FRESH_ALLOC_CALLEES` (they remain in `CONSUME_HELPERS`, which is their true single category). Add a
one-line comment recording the resolution.

- [ ] **Step 4: Run tests to verify green**

Run: `cargo test -p ruxen_core --test runtime_abi_parity 2>&1 | tee tmp/test-cache/phase2-task3b-green.log`
Then the leak backstop (this is the one behaviour-affecting change — exercise the drop path):
`cargo test -p ruxen_core --test drop_fixtures 2>&1 | tee tmp/test-cache/phase2-task3b-drop.log`
Expected: both PASS. If a `drop_fixtures` `into_bytes` case exists and flips, INVESTIGATE — that is the
signal the old `Fresh` membership was load-bearing after all; if so, revert this task and document why
the contradiction must stay (a `# TODO(<ticket>)` with the reason), surfacing to the maintainer.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/mir/lower/runtime_abi.rs compiler/ruxen_core/tests/runtime_abi_parity.rs
git commit -m "fix(mir): into_bytes is consume-only, drop dead FRESH_ALLOC membership

ruxen_string_into_bytes / String_into_bytes were in BOTH FRESH_ALLOC_CALLEES
and the consume-helper set — a contradiction. The arg loop only ever read the
consume classification, so the fresh-alloc membership was dead and misleading.
Resolve to the single true category (consume). Visible test diff per the UAF
risk-register mandate; drop_fixtures stays balanced.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — `git diff HEAD~1..HEAD`. Confirm a
  symbol moved category with a corresponding test diff, net reduction (two entries gone), no new
  catch-all.

---

## Task 4: Route `method_call.rs` static-ctor classification through the table

**Files:**
- Modify: `compiler/ruxen_core/src/mir/lower/expr/method_call.rs:162-214` (the `is_*_static_ctor`
  booleans + `is_collection_ctor`) and the duplicated base-type list `276-328`
- Test: `compiler/ruxen_core/tests/ffi_alias_single_entry.rs` (pin) + a focused unit test

The static-ctor knowledge is a SECOND duplication: `method_call.rs` decides "is this a static
constructor that dispatches directly to a runtime symbol" via a cascade of
`is_file_static_ctor`/`is_duration_static_ctor`/`is_tcp_*_static_ctor`/`is_bufio_static_ctor`/… plus a
literal `matches!(base_type, "Vec" | "Array" | "Hash" | ... )` block — and `util.rs` encodes the SAME
fact in `is_builtin_static_method` with a DIFFERENT (already-diverged) list (e.g. `util.rs` knows
`Thread`/`Mutex`/`Arc`/`SharedSync` and `String.from`/`from_bytes`; `method_call.rs` does not list
`Thread`/`Mutex` but lists `Formatter`/`Command`/`OpenOptions`). Phase 2 unifies the
"is-static-constructor" predicate into the table so both sites read one function.

> **DESIGN CONSTRAINT (read before coding):** the master goal for Phase 2 names `callee_ownership` as
> the table's primary export, but the static-ctor predicate answers a DIFFERENT question
> ("classify-as-static at the method-call site, keyed on `(type_name, method_name)`") than
> `callee_ownership` ("ownership of a mangled callee at the drop site, keyed on the mangled string").
> Per global rules 4 & 37 (no premature abstraction; explicit over clever) DO NOT force these two into
> one function. Add a SECOND, sibling function to `runtime_abi.rs`:
> `pub(crate) fn is_static_constructor(type_name: &str, method_name: &str) -> bool`, sourced from one
> declarative `(base_type, &[method]) ` table. This is the single source the THREE sites
> (`method_call.rs` fast-path gate, `method_call.rs` base-type list, `util.rs::is_builtin_static_method`)
> all read. Reconciling the two diverged lists into one is the whole point — the union must be taken
> deliberately and pinned by a test (next step), because a divergence here is a "synthesises a phantom
> `self` arg → Cranelift arg-count rejection" link/verify failure (documented at `util.rs:98-103`).

- [ ] **Step 1: Write the failing reconciliation test**

Add an inline `#[cfg(test)]` test to `runtime_abi.rs` capturing the UNION the two sites must agree on:

```rust
#[test]
fn static_constructor_union_is_reconciled() {
    // From method_call.rs fast path:
    assert!(is_static_constructor("File", "open"));
    assert!(is_static_constructor("Duration", "from_secs"));
    assert!(is_static_constructor("Instant", "now"));
    assert!(is_static_constructor("TcpListener", "bind"));
    assert!(is_static_constructor("TcpStream", "connect"));
    assert!(is_static_constructor("BufReader", "new"));
    assert!(is_static_constructor("BufWriter", "with_capacity"));
    // From util.rs::is_builtin_static_method (the diverged sibling):
    assert!(is_static_constructor("String", "from"));
    assert!(is_static_constructor("String", "from_bytes"));
    assert!(is_static_constructor("Thread", "spawn"));
    assert!(is_static_constructor("Mutex", "new"));
    assert!(is_static_constructor("Arc", "new"));
    assert!(is_static_constructor("SharedSync", "new"));
    // Generic-base handling (Vec[T] → Vec):
    assert!(is_static_constructor("Vec[String]", "with_capacity"));
    assert!(is_static_constructor("HashMap[K, V]", "from_iter"));
    // The universal `new` rule that method_call.rs's is_collection_ctor applies:
    assert!(is_static_constructor("AnyUserClass", "new"));
    // Negatives:
    assert!(!is_static_constructor("Vec", "len"));
    assert!(!is_static_constructor("File", "read"));
}
```

> **Implementer obligation (explicit sub-step):** before writing `is_static_constructor`, READ both
> `util.rs:74-132` and `method_call.rs:162-214` and produce the UNION table. Two reconciliation
> decisions you MUST make explicitly and record in a comment:
> 1. `method_call.rs`'s `is_collection_ctor` treats `method_name == "new"` as static for ANY type
>    (`197`); `util.rs` does NOT. The union must keep the "any `.new` is a static ctor" rule (it is the
>    behaviour `method_call.rs` actually ships) — that is why the `AnyUserClass.new` assertion is
>    present. Confirm against the `ffi_alias_single_entry` pin (Step 4) that this doesn't reroute a
>    user class with a real `init`.
> 2. `String.from`/`from_bytes`/`from_iter` and `Thread`/`Mutex`/`Arc`/`SharedSync` exist in `util.rs`
>    but not the `method_call.rs` base-type list. Decide per call-site whether each site needs them:
>    `util.rs`'s consumers (static-vs-instance dispatch) DO; `method_call.rs`'s direct-dispatch list is
>    gated additionally by `lookup_ffi_alias` (`241`) and `is_module_nested_class` (`274`), so the base
>    list there is a fast-path allowlist, not the authority. KEEP `method_call.rs`'s fast-path list
>    behaviourally identical (route it through `is_static_constructor` only where the predicate is a
>    pure superset); if a base type would CHANGE the fast path, leave that base in an explicit local
>    list and note it. Do not silently widen the fast path.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --lib mir::lower::runtime_abi 2>&1 | tee tmp/test-cache/phase2-task4-red.log`
Expected: FAIL — `is_static_constructor` not found.

- [ ] **Step 3: Implement `is_static_constructor` + rewire the two sites**

Add to `runtime_abi.rs`:

```rust
/// Is `type_name::method_name` a static (no-`self`) constructor that dispatches
/// directly to a runtime symbol? Single source for the static-vs-instance
/// decision shared by `util.rs::is_builtin_static_method` and the
/// `method_call.rs` fast-path gate. Reconciles the two formerly-diverged lists.
pub(crate) fn is_static_constructor(type_name: &str, method_name: &str) -> bool {
    // Any `.new` is a static constructor (method_call.rs is_collection_ctor rule).
    if method_name == "new" {
        return true;
    }
    let base = match type_name.find('[') {
        Some(pos) => &type_name[..pos],
        None => type_name,
    };
    STATIC_CTORS
        .iter()
        .any(|(b, methods)| *b == base && methods.contains(&method_name))
}

/// (base_type, static-constructor method names). UNION of the two formerly
/// duplicated lists (util.rs:74-132 + method_call.rs:162-214). One source.
const STATIC_CTORS: &[(&str, &[&str])] = &[
    ("String", &["from", "with_capacity", "from_iter", "from_bytes"]),
    ("Vec", &["with_capacity", "from_iter"]),
    ("Array", &["with_capacity", "from_iter"]),
    ("Hash", &["with_capacity", "from_iter"]),
    ("HashMap", &["with_capacity", "from_iter"]),
    ("Map", &["with_capacity", "from_iter"]),
    ("Set", &["with_capacity", "from_iter"]),
    ("HashSet", &["with_capacity", "from_iter"]),
    ("Thread", &["spawn", "current", "sleep", "yield_now"]),
    ("Mutex", &[]),                 // only `.new`, handled above
    ("Arc", &[]),                   // only `.new`
    ("SharedSync", &[]),            // only `.new`
    ("Duration", &["from_secs", "from_millis", "from_micros", "from_nanos"]),
    ("Instant", &["now"]),
    ("TcpListener", &["bind"]),
    ("TcpStream", &["connect"]),
    ("File", &["open", "create", "append", "open_options"]),
    ("BufReader", &["with_capacity"]),
    ("BufWriter", &["with_capacity"]),
];
```

Then:
1. In `util.rs:74-132`, replace the whole `is_builtin_static_method` BODY with
   `crate::mir::lower::runtime_abi::is_static_constructor(type_name, method_name)`. Keep the function
   signature (callers unchanged).
2. In `method_call.rs:197-214`, replace the `is_collection_ctor` boolean cascade
   (`is_file_static_ctor` / `is_duration_static_ctor` / `is_instant_static_ctor` /
   `is_tcp_*_static_ctor` / `is_bufio_static_ctor` / the `with_capacity` base check) with
   `let is_collection_ctor = crate::mir::lower::runtime_abi::is_static_constructor(&type_name, &method_name);`
   and delete the now-dead `is_*_static_ctor` `let` bindings (`162-196`).

> Leave the `276-328` base-type fast-path `matches!` block AS-IS for now if and only if routing it
> through `is_static_constructor` would change the fast path (per the Step-1 obligation #2). If it is a
> pure superset, replace its condition with `is_static_constructor(base_type, &method_name)`; otherwise
> keep it and add a `// fast-path allowlist; authority is is_static_constructor` comment. The
> `bufio_suffix`/`runtime_base`/alias-resolution machinery below it is unaffected.

- [ ] **Step 4: Run tests to verify green**

Run: `cargo test -p ruxen_core --lib mir::lower::runtime_abi 2>&1 | tee tmp/test-cache/phase2-task4-green.log`
Then the pin + a compile-and-run smoke over the touched constructors:
- `cargo test -p ruxen_core --test ffi_alias_single_entry 2>&1 | tee tmp/test-cache/phase2-task4-ffi.log`
- `cargo test -p ruxen_core --test drop_fixtures 2>&1 | tee tmp/test-cache/phase2-task4-drop.log`
Expected: PASS. (drop_fixtures exercises Vec/Hash/String constructors end-to-end; if a static-ctor
reclassification slipped, a constructor would mis-dispatch and the fixture would fail to link/run.)

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/mir/lower/runtime_abi.rs \
        compiler/ruxen_core/src/mir/lower/expr/method_call.rs \
        compiler/ruxen_core/src/mir/lower/util.rs
git commit -m "refactor(mir): one is_static_constructor table for both call sites

Reconcile the two diverged static-ctor lists (util.rs::is_builtin_static_method
and the method_call.rs is_*_static_ctor cascade) into one declarative
STATIC_CTORS table in runtime_abi.rs. util.rs delegates; method_call.rs's
is_collection_ctor cascade collapses to one call. Union pinned by test;
ffi_alias_single_entry + drop_fixtures green.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — `git diff HEAD~1..HEAD`. Confirm: net
  reduction across `method_call.rs` + `util.rs` (~−350 target), the divergent duplicate is GONE (one
  table), no new `_ =>` catch-all, the static-ctor decision is now a single function.

---

## Task 5: Phase-2 final integration

**Files:** none (verification only).

- [ ] **Step 1: Run the full compiler-crate suite once**

Run: `cargo test -p ruxen_core 2>&1 | tee tmp/test-cache/phase2-final.log`
Expected: all green. Per global rule 41/42 this is the ONLY full-suite run in Phase 2; intermediate
tasks ran only their narrow tests + the three pins.

- [ ] **Step 1b: Full multi-agent `/thermonuke` sweep (phase gate)**

Invoke the `thermonuke` skill on the whole Phase 2 diff (`git diff <phase2-base>..HEAD`). Authoritative
phase gate — must confirm: the SIX `drops.rs` classifier sets + `transfer_indices` are *reduced to* one
`callee_ownership` lookup (not added alongside); the static-ctor duplication across
`method_call.rs`/`util.rs` is *reduced to* one `is_static_constructor`; the `into_bytes` contradiction
is resolved with a visible test diff; net deletion is in the −750 range; no new structural debt.
Surface the report to the maintainer.

- [ ] **Step 2: Confirm no catch-all sneaked into the table or the Call arm**

Run: `grep -nE '_ =>' compiler/ruxen_core/src/mir/lower/runtime_abi.rs`
Expected: only the documented fallthroughs — `arg_transfer_mask`'s final `_ => ArgMask::none()` and the
`is_static_constructor` table's `.any(...)` (no wildcard). The `CalleeOwnership::default()` is the ONE
intentional unknown-callee verdict and is in an `impl Default`, not a `match _`.
Run: `grep -nE 'FRESH_ALLOC_CALLEES|is_runtime_consume_helper|is_runtime_borrow_helper|is_move_by_ffi_callee|is_pointer_store_helper|is_command_(builder|terminal)' compiler/ruxen_core/src/mir/lower/drops.rs`
Expected: ZERO matches — every predicate is gone from `drops.rs` (they live only in `runtime_abi.rs`'s
table data and the parity-test oracle).

- [ ] **Step 3: Report**

Report to maintainer: net line delta (`git diff --stat <phase2-base>..HEAD`), the parity test green
(`tmp/test-cache/phase2-task3-green.log`), the three pin tests green, full suite green
(`tmp/test-cache/phase2-final.log`), and the statement: "No behaviour changed except the
`into_bytes` fresh-vs-consume contradiction resolution (Task 3b), which is pinned by a visible test
diff; every other migration is behaviour-preserving, proven by the union-parity test." Await go-ahead
for Phase 5.

---

## Self-Review (run before handing off)

**Spec coverage:** Root Cause B (drops.rs) primary target — the 6 overlapping classifier sets +
`transfer_indices` collapsed to `callee_ownership` (✓ Tasks 2,3); the duplicated static-ctor knowledge
in `method_call.rs` + `util.rs` collapsed to `is_static_constructor` (✓ Task 4); the documented
`ruxen_string_into_bytes` fresh-vs-consume contradiction resolved with a visible test diff
(✓ Task 3b). The sibling Root-Cause-B target (the ~1,090-arm method-resolver ladder in
`typeck/method_resolvers`) is Phase 5, not a Phase 2 gap.

**Placeholder scan:** No `todo!()` in production code. Three explicit implementer transcription
sub-steps (Task 1 Step 1–2 verbatim oracle; Task 2 Step 3 the six `const &[&str]` slices; Task 4 Step 1
the union reconciliation) name the exact source line ranges to copy, because the data is a 1:1 move of
existing literals — transcribing in the plan would risk a copy error that the parity test specifically
exists to catch, and pasting ~110 string literals into the plan would be invented redundancy, not
precision. Every other code block (types, `callee_ownership`, `arg_transfer_mask`, `has_runtime_prefix`,
`is_static_constructor`, the rewritten `drops.rs` arg loop) is complete and real.

**Type consistency:** `callee_ownership(&str) -> CalleeOwnership` matches its use in `drops.rs`
(Task 3) and its inline tests (Task 2); `ResultOwnership::{Fresh,None}` is exhaustive (the `dest`
handler branches on `== Fresh`); `ArgMask` constructors (`none`/`single`/`pair`/`contains`) match every
call site; `CalleeOwnership` fields (`result`, `borrows_first_arg`, `arg_transfer`, `args_are_borrowed`)
map 1:1 onto the four decisions the old arg loop made, in the same precedence order. The parity oracle
in `runtime_abi_parity.rs` reproduces the *effective* (post-3-rebinds) `is_runtime_borrow_helper`, which
is the value the old `1263` `if is_runtime_borrow_helper` actually tested. `extract_method_name` is the
same `crate::codegen::runtime::extract_method_name` used at the old `drops.rs:1216`.

**Visibility caveat (flagged for implementer):** the union-parity integration test (Task 3) needs
`callee_ownership` + the types reachable from `tests/`. Default plan: keep parity as an inline
`#[cfg(test)]` module inside `runtime_abi.rs` and only the contradiction/static-ctor witnesses in the
integration test, so `pub(crate)` visibility is sufficient and no API is widened for testing. If the
implementer prefers the integration-test form, widen to `pub` and note it in the commit — both satisfy
the no-behaviour-change bar.
