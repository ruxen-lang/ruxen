//! REPL variable environment — heap-allocated storage for live variables.
//!
//! Phase 1 MVP uses `HashMap<String, Box<dyn Any>>` for safety.
//!
//! Production REPL flow keeps live values in the JIT slot table
//! (`slots.rs`), not here; `ReplSession` still owns a `ReplEnv` and
//! clears it on `:reset`. The value-seeding surface
//! (`set_value`/`set_i64`/`is_live`) is therefore exercised only by the
//! reset test that proves `ReplSession::reset` clears `env`, so it is
//! gated behind `cfg(test)` rather than shipped as dead weight.

use std::any::Any;
use std::collections::HashMap;

/// Runtime environment for REPL variable storage.
///
/// MVP: safe `HashMap`-based storage (same approach as evcxr).
pub struct ReplEnv {
    /// Stored variable values (boxed for type erasure). A key's presence
    /// means the variable is live.
    values: HashMap<String, Box<dyn Any>>,
}

impl ReplEnv {
    pub fn new() -> Self {
        ReplEnv {
            values: HashMap::new(),
        }
    }

    /// Clear all state (for :reset command).
    pub fn reset(&mut self) {
        self.values.clear();
    }

    /// Store a value for a variable. If the variable already exists,
    /// the old value is dropped (reassignment semantics, not shadowing).
    #[cfg(test)]
    pub fn set_value(&mut self, name: &str, value: Box<dyn Any>) {
        self.values.insert(name.to_string(), value);
    }

    /// Store a raw i64 value (most common case for REPL results).
    #[cfg(test)]
    pub fn set_i64(&mut self, name: &str, value: i64) {
        self.set_value(name, Box::new(value));
    }

    /// Check if a variable is live (has a stored value).
    #[cfg(test)]
    pub fn is_live(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }
}
