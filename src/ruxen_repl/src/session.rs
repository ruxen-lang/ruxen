//! REPL session state management.
//!
//! `ReplSession` owns all cumulative state for the REPL. It grows with
//! each successfully executed input.

use std::path::PathBuf;

use ruxen_core::hir::types::Ty;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::ast::{FuncDef, LetBinding, Statement, TopLevelItem};
use ruxen_core::parser::Parser;

use crate::env::ReplEnv;
use crate::jit::JITCodeGen;

/// Number of persistent variable slots. Each session variable gets one
/// 8-byte slot in a stable Rust-owned region; the JIT'd wrapper reads
/// and writes them by baked-in address. 256 is plenty for an
/// interactive session and keeps the region a fixed (non-reallocating,
/// address-stable) allocation.
pub const REPL_MAX_SLOTS: usize = 256;

/// A persisted session variable: its name, declared type, and the
/// fixed slot index holding its current value (an i64 word — a pointer
/// for heap types, the value itself for scalars).
#[derive(Clone)]
pub struct VarSlot {
    pub name: String,
    pub ty: Ty,
    pub idx: usize,
    /// True if the original Ruxen declaration was `var` (mutable);
    /// false for `let` (immutable). The synthetic slot-load binding
    /// in `eval::slot_load_let` propagates this flag so subsequent
    /// inputs' typechecker rejects user `name = expr` assignments
    /// to immutable lets but allows them on `var` decls — matching
    /// the source's declared mutability after Phase 2.5's replay
    /// filter dropped the original `let`/`var` from the wrapper body.
    pub mutable: bool,
}

/// Complete state for a REPL session.
pub struct ReplSession {
    /// All source inputs so far (for error reporting and :save).
    pub source_history: Vec<String>,
    /// Input counter (for generating unique function names: __repl_0, __repl_1, ...).
    pub input_counter: u32,
    /// JIT code generator (persistent Cranelift module).
    pub jit: JITCodeGen,
    /// The heap-allocated environment holding live variable values.
    pub env: ReplEnv,
    /// Function definitions declared in prior inputs (replayed into every
    /// new program so typecheck/resolve can see them).
    pub func_defs: Vec<FuncDef>,
    /// Let bindings declared in prior inputs (replayed into every wrapper
    /// body so subsequent inputs can reference the bound names).
    pub let_bindings: Vec<LetBinding>,
    /// Captured stdout from the most recent input's wrapper execution.
    /// Populated by `eval::compile_and_execute` after draining the
    /// `capture::BUFFER` shim sink; the interactive main loop reads
    /// nothing from it (compile_and_execute also re-emits to the real
    /// stdout), but the test harness `run_session` snapshots this so
    /// tests can assert what `puts` produced per input.
    pub last_output: String,
    /// Chronologically ordered replay history. Contains every prior
    /// `let` binding AND every prior side-effecting expression
    /// statement that mutates a session-bound name (assignment,
    /// compound-assignment, method call whose receiver is a known
    /// session var, etc.). Interleaved in input order so an
    /// intervening `let n = h.len` between two `h.insert(...)` calls
    /// captures the correct intermediate state.
    ///
    /// Note (Task 3 — replay-suppression flag): the replay portion of
    /// each input's wrapper runs with the runtime
    /// `ruxen_repl_is_replaying` flag set, so any embedded puts /
    /// fs.write / subprocess spawn inside a replayed mutation
    /// silently no-ops at the C-runtime layer. The classifier can
    /// therefore be aggressive about what counts as a mutation
    /// without spurious stdout duplicates.
    pub session_var_mutations: Vec<Statement>,
    /// Type-level items (class, struct, enum, trait, impl, const,
    /// type-alias, newtype, module, use, lib, extern) declared in prior
    /// inputs. Replayed into every new program.
    pub type_items: Vec<TopLevelItem>,
    /// History file path for persistence.
    pub history_path: PathBuf,
    /// Stable, address-fixed region of persistent variable slots. The
    /// JIT'd wrapper loads/stores variable values here by baked-in
    /// address, so state persists across inputs WITHOUT re-executing the
    /// statements that produced it (no replay of side effects, no
    /// re-evaluation of non-deterministic bindings like `Instant.now()`).
    pub slots: Box<[i64]>,
    /// Session variables that currently have a persisted slot, in
    /// declaration order. A name may appear once; rebinding updates its
    /// type in place and reuses the slot.
    pub var_slots: Vec<VarSlot>,
    /// Pre-parsed `lib "ruxen_repl" ... end` block declaring the two
    /// FFI shims (`__slot_load_i64`, `__slot_store_i64`) that the
    /// synthetic prefix/suffix injection in `eval::build_program` calls
    /// to read/write session-variable slots. Parsed once at session
    /// creation so every input cheaply prepends it to the wrapper
    /// program. The C symbols `ruxen_repl_slot_load_i64` /
    /// `ruxen_repl_slot_store_i64` are registered as JIT symbols in
    /// `jit::new_jit_module_builder` — the lib block here only tells
    /// the typechecker and resolver that callable Ruxen-side names
    /// exist with the right signatures.
    pub repl_slot_lib: TopLevelItem,
}

impl ReplSession {
    /// Create a new REPL session with fresh state.
    pub fn new() -> Result<Self, String> {
        let jit = JITCodeGen::new()?;

        // History path: ~/.config/ruxen/history
        let history_path = dirs_path().join("history");

        Ok(ReplSession {
            source_history: Vec::new(),
            input_counter: 0,
            jit,
            env: ReplEnv::new(),
            func_defs: Vec::new(),
            let_bindings: Vec::new(),
            session_var_mutations: Vec::new(),
            last_output: String::new(),
            type_items: Vec::new(),
            history_path,
            slots: vec![0i64; REPL_MAX_SLOTS].into_boxed_slice(),
            var_slots: Vec::new(),
            repl_slot_lib: parse_repl_slot_lib(),
        })
    }

    /// Base address of the persistent slot region (stable for the
    /// session's lifetime).
    pub fn slots_base_addr(&self) -> i64 {
        self.slots.as_ptr() as i64
    }

    /// Byte address of slot `idx`.
    pub fn slot_addr(&self, idx: usize) -> i64 {
        self.slots_base_addr() + (idx as i64) * 8
    }

    /// Find the slot for a live session variable by name.
    pub fn find_var_slot(&self, name: &str) -> Option<&VarSlot> {
        self.var_slots.iter().find(|v| v.name == name)
    }

    /// Drop the slot entry for a session variable name, if present.
    /// Returns `true` if an entry was removed.
    ///
    /// Used by `eval_statement` when a `let` shadow widens an existing
    /// slot-backed Int to a non-slot-eligible type (String, Array, etc.).
    /// Merely updating the existing entry's `ty` is not enough — the
    /// `collect_replay_statements` filter keys off the *presence* of the
    /// name in `var_slots`, not its eligibility, so the prior
    /// `let name = <Int expr>` history entries would stay filtered out
    /// and the next input that READS `name` would find it undefined
    /// (no slot prefix is generated for the now-non-eligible type, AND
    /// no replay statement restores the binding). Dropping the entry
    /// flips the filter so the historical lets replay, the user's
    /// shadow runs after them, and subsequent reads see the new
    /// binding. We deliberately do NOT reclaim the underlying slot
    /// index — the persistent slot region is append-only and forward-
    /// compatible with future re-registration of the same name.
    pub fn unregister_var(&mut self, name: &str) -> bool {
        let before = self.var_slots.len();
        self.var_slots.retain(|v| v.name != name);
        self.var_slots.len() != before
    }

    /// Register (or re-register) a session variable, returning its slot
    /// index. Rebinding an existing name reuses its slot and updates the
    /// type. New names get the next free slot.
    pub fn register_var(&mut self, name: &str, ty: Ty, mutable: bool) -> Result<usize, String> {
        if let Some(existing) = self.var_slots.iter_mut().find(|v| v.name == name) {
            existing.ty = ty;
            // Rebinding refreshes mutability — `var foo = ...` after a
            // prior `let foo = ...` is a fresh binding in the source
            // (Pattern::Identifier carries its own `mutable` flag), and
            // the slot now represents the latest decl.
            existing.mutable = mutable;
            return Ok(existing.idx);
        }
        let idx = self.var_slots.len();
        if idx >= REPL_MAX_SLOTS {
            return Err(format!(
                "REPL session variable limit ({}) reached",
                REPL_MAX_SLOTS
            ));
        }
        self.var_slots.push(VarSlot {
            name: name.to_string(),
            ty,
            idx,
            mutable,
        });
        Ok(idx)
    }

    /// Get the next REPL wrapper function name and increment the counter.
    pub fn next_repl_fn_name(&mut self) -> String {
        let name = format!("__repl_{}", self.input_counter);
        self.input_counter += 1;
        name
    }

    /// Record a successfully executed input.
    pub fn record_input(&mut self, input: &str) {
        self.source_history.push(input.to_string());
    }

    /// Reset all state (for :reset command).
    pub fn reset(&mut self) -> Result<(), String> {
        self.source_history.clear();
        self.input_counter = 0;
        self.env.reset();
        self.func_defs.clear();
        self.let_bindings.clear();
        self.session_var_mutations.clear();
        self.last_output.clear();
        self.type_items.clear();
        // Zero the persistent slot region and forget all variable→slot
        // mappings. (The heap region itself is reused — its address stays
        // stable; we just clear the stored pointers/values.)
        for s in self.slots.iter_mut() {
            *s = 0;
        }
        self.var_slots.clear();
        crate::capture::clear();
        // Recreate JIT module (old one can't be reused after reset)
        self.jit = JITCodeGen::new()?;
        Ok(())
    }
}

/// Parse the REPL-internal `lib "ruxen_repl" ... end` block declaring
/// the two slot helpers. Returning `TopLevelItem::Lib(_)` lets
/// `build_program` cheaply prepend it to every input's program so the
/// resolver/typechecker see the FFI fn names in scope. We parse a
/// hardcoded source snippet rather than hand-constructing the AST so
/// the layered FfiFunction/TypeExpr/Span fields stay in sync with
/// whatever the parser emits (Approach A fallback per Task 1.2).
fn parse_repl_slot_lib() -> TopLevelItem {
    // Two FFI shims for the REPL-internal slot read/write helpers
    // registered in `jit::new_jit_module_builder`. `as "ruxen_repl_*"`
    // pins the linked C symbol; the bare Ruxen names are what the
    // synthetic prefix/suffix call sites spell.
    // The two `__repl_set_replaying` / `__repl_get_replaying` decls
    // expose Task 3's runtime replay-suppression flag (see
    // `library/std/core/runtime/repl_replay.c`). The synthetic
    // wrapper body in `eval::build_program` calls
    // `__repl_set_replaying(1)` immediately before the replayed
    // `let_bindings + session_var_mutations` block and
    // `__repl_set_replaying(0)` immediately after, so every non-
    // idempotent runtime helper (puts/print/Command.status/fs.write/
    // …) early-returns a benign value during replay and only fires
    // once per input on the user's new-statement path.
    let src = "lib \"ruxen_repl\"\n\
               def __slot_load_i64 as \"ruxen_repl_slot_load_i64\"(addr: Int) -> Int\n\
               def __slot_store_i64 as \"ruxen_repl_slot_store_i64\"(addr: Int, val: Int)\n\
               def __repl_set_replaying as \"ruxen_repl_set_replaying\"(v: Int) -> Int\n\
               def __repl_get_replaying as \"ruxen_repl_get_replaying\"() -> Int\n\
               end\n";
    let tokens = Lexer::new(src)
        .tokenize()
        .expect("REPL slot-lib snippet should tokenize");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("REPL slot-lib snippet should parse");
    program
        .items
        .into_iter()
        .find(|it| matches!(it, TopLevelItem::Lib(_)))
        .expect("REPL slot-lib snippet should produce a `lib` item")
}

/// Get the Ruxen config directory, creating it if needed.
fn dirs_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = PathBuf::from(home).join(".config").join("ruxen");
    let _ = std::fs::create_dir_all(&path);
    path
}
#[cfg(test)]
mod tests {
    use super::*;
    use ruxen_core::hir::types::Ty;
    use ruxen_core::lexer::Lexer;
    use ruxen_core::parser::ast::{ReplInput, ReplParseResult, Statement};
    use ruxen_core::parser::Parser;

    /// Parse a REPL input and extract a single `LetBinding` — handy
    /// when the test needs to stuff a realistic binding into the
    /// session instead of hand-crafting the AST.
    fn parse_let_binding(src: &str) -> LetBinding {
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize().expect("lex");
        let mut parser = Parser::new(tokens);
        match parser.parse_repl_input() {
            ReplParseResult::Complete(ReplInput::Statement(Statement::Let(b))) => b,
            other => panic!("expected let binding, got {:?}", other),
        }
    }

    #[test]
    fn new_session_has_empty_history() {
        let s = ReplSession::new().expect("create session");
        assert!(s.source_history.is_empty());
        assert!(s.func_defs.is_empty());
        assert!(s.let_bindings.is_empty());
        assert!(s.type_items.is_empty());
        assert_eq!(s.input_counter, 0);
    }

    #[test]
    fn new_session_history_path_points_into_ruxen_config() {
        let s = ReplSession::new().expect("create session");
        let path_str = s.history_path.to_string_lossy().into_owned();
        assert!(path_str.contains(".config"));
        assert!(path_str.contains("ruxen"));
        assert!(path_str.ends_with("history"));
    }

    #[test]
    fn next_repl_fn_name_increments_counter() {
        let mut s = ReplSession::new().expect("create session");
        assert_eq!(s.next_repl_fn_name(), "__repl_0");
        assert_eq!(s.next_repl_fn_name(), "__repl_1");
        assert_eq!(s.next_repl_fn_name(), "__repl_2");
        assert_eq!(s.input_counter, 3);
    }

    #[test]
    fn record_input_appends_to_history() {
        let mut s = ReplSession::new().expect("create session");
        s.record_input("1 + 2");
        s.record_input("let x = 3");
        assert_eq!(
            s.source_history,
            vec!["1 + 2".to_string(), "let x = 3".to_string()]
        );
    }

    #[test]
    fn reset_clears_all_accumulated_state() {
        let mut s = ReplSession::new().expect("create session");
        // Seed every field so the reset can be observed clearing each one.
        s.source_history.push("1 + 1".into());
        s.input_counter = 7;
        s.let_bindings.push(parse_let_binding("let x = 42"));
        s.env.set_i64("x", 42, Ty::Int);

        s.reset().expect("reset");

        assert!(
            s.source_history.is_empty(),
            "source_history should be cleared"
        );
        assert_eq!(s.input_counter, 0, "input_counter should reset to 0");
        assert!(s.let_bindings.is_empty(), "let_bindings should be cleared");
        assert!(s.func_defs.is_empty(), "func_defs should be cleared");
        assert!(s.type_items.is_empty(), "type_items should be cleared");
        assert!(!s.env.is_live("x"), "env should be reset (no live vars)");
    }

    #[test]
    fn reset_keeps_history_path_stable() {
        let mut s = ReplSession::new().expect("create session");
        let before = s.history_path.clone();
        s.reset().expect("reset");
        assert_eq!(s.history_path, before, "history_path should survive :reset");
    }

    #[test]
    fn next_repl_fn_name_restarts_after_reset() {
        let mut s = ReplSession::new().expect("create session");
        s.next_repl_fn_name();
        s.next_repl_fn_name();
        s.reset().expect("reset");
        assert_eq!(s.next_repl_fn_name(), "__repl_0");
    }

    /// Smoke test: once the session has been reset, the empty state should
    /// match a freshly constructed session field-for-field (exceptions:
    /// JIT module and history path are intentionally out of scope).
    #[test]
    fn reset_state_matches_fresh_session() {
        let mut s = ReplSession::new().expect("create session");
        s.record_input("foo");
        s.input_counter = 99;
        s.reset().expect("reset");

        let fresh = ReplSession::new().expect("fresh session");
        assert_eq!(s.source_history, fresh.source_history);
        assert_eq!(s.input_counter, fresh.input_counter);
        assert_eq!(s.let_bindings.len(), fresh.let_bindings.len());
        assert_eq!(s.func_defs.len(), fresh.func_defs.len());
        assert_eq!(s.type_items.len(), fresh.type_items.len());
    }

    /// Tests that pushing a hand-built binding into the session and
    /// then calling `reset` drops it. We parse a real `let` rather than
    /// hand-constructing the AST, matching the guideline from the task.
    #[test]
    fn pushing_let_binding_then_reset_clears_it() {
        let mut s = ReplSession::new().expect("create session");
        s.let_bindings.push(parse_let_binding("let a = 1"));
        assert_eq!(s.let_bindings.len(), 1);
        s.reset().expect("reset");
        assert!(s.let_bindings.is_empty());
    }

    #[test]
    fn new_session_has_empty_cumulative_state() {
        let s = ReplSession::new().expect("create session");
        assert!(s.session_var_mutations.is_empty());
        let _ = Ty::Unit;
    }

    #[test]
    fn reset_clears_session_var_mutations() {
        let mut s = ReplSession::new().expect("create session");
        s.session_var_mutations
            .push(Statement::Let(parse_let_binding("let a = 1")));
        assert_eq!(s.session_var_mutations.len(), 1);
        s.reset().expect("reset");
        assert!(s.session_var_mutations.is_empty());
    }

    #[test]
    fn reset_also_clears_global_capture_buffer() {
        // The capture buffer is process-global — serialize against other
        // capture tests so we don't race.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        crate::capture::clear();
        // Prime the buffer by writing to it through the shim.
        let cs = std::ffi::CString::new("marker").unwrap();
        crate::capture::ruxen_repl_puts_shim(cs.as_ptr());
        let mut s = ReplSession::new().expect("create session");
        s.reset().expect("reset");
        assert_eq!(crate::capture::take_all(), "");
    }
}
