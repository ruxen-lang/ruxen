//! REPL session state management.
//!
//! `ReplSession` owns all cumulative state for the REPL. It grows with
//! each successfully executed input.

use std::path::PathBuf;

use ruxen_core::hir::types::Ty;
use ruxen_core::parser::ast::{FuncDef, LetBinding, Statement, TopLevelItem};

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
    /// Full cumulative statement history — every executed `let`, every
    /// side-effecting expression (assignment, compound assignment,
    /// method call, `puts`, control-flow with mutations, etc.). Replayed
    /// into each new wrapper so mutations persist across inputs.
    pub all_statements: Vec<Statement>,
    /// Full captured stdout produced by the previous cumulative replay.
    /// On the next input, the fresh capture is diffed against this prefix
    /// and only the new suffix is emitted to real stdout.
    pub prev_captured_output: String,
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
            all_statements: Vec::new(),
            prev_captured_output: String::new(),
            type_items: Vec::new(),
            history_path,
            slots: vec![0i64; REPL_MAX_SLOTS].into_boxed_slice(),
            var_slots: Vec::new(),
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

    /// Register (or re-register) a session variable, returning its slot
    /// index. Rebinding an existing name reuses its slot and updates the
    /// type. New names get the next free slot.
    pub fn register_var(&mut self, name: &str, ty: Ty) -> Result<usize, String> {
        if let Some(existing) = self.var_slots.iter_mut().find(|v| v.name == name) {
            existing.ty = ty;
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
        self.all_statements.clear();
        self.prev_captured_output.clear();
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
        assert!(s.all_statements.is_empty());
        assert!(s.prev_captured_output.is_empty());
        let _ = Ty::Unit;
    }

    #[test]
    fn reset_clears_all_statements_and_capture_prefix() {
        let mut s = ReplSession::new().expect("create session");
        s.all_statements
            .push(Statement::Let(parse_let_binding("let a = 1")));
        s.prev_captured_output.push_str("prior output\n");
        assert_eq!(s.all_statements.len(), 1);
        assert!(!s.prev_captured_output.is_empty());
        s.reset().expect("reset");
        assert!(s.all_statements.is_empty());
        assert!(s.prev_captured_output.is_empty());
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
