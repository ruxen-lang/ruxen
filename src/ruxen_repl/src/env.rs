//! REPL variable environment — heap-allocated storage for live variables.
//!
//! Phase 1 MVP uses `HashMap<String, Box<dyn Any>>` for safety.
//! Variable state (Live/Moved/Dropped) is tracked separately.

use std::any::Any;
use std::collections::HashMap;

use ruxen_core::hir::types::Ty;

/// State of a variable in the REPL environment.
#[derive(Debug, Clone)]
pub enum VarState {
    /// Variable is live and owns its value.
    Live { ty: Ty },
    /// Variable has been moved (value transferred to another variable).
    Moved {
        ty: Ty,
        moved_to: String,
        moved_at: u32,
    },
    /// Variable has been explicitly dropped or reassigned.
    Dropped,
}

/// Runtime environment for REPL variable storage.
///
/// MVP: safe `HashMap`-based storage (same approach as evcxr).
pub struct ReplEnv {
    /// Stored variable values (boxed for type erasure).
    values: HashMap<String, Box<dyn Any>>,
    /// Ownership state for each variable.
    states: HashMap<String, VarState>,
}

impl ReplEnv {
    pub fn new() -> Self {
        ReplEnv {
            values: HashMap::new(),
            states: HashMap::new(),
        }
    }

    /// Store a value for a variable. If the variable already exists,
    /// the old value is dropped (reassignment semantics, not shadowing).
    pub fn set_value(&mut self, name: &str, value: Box<dyn Any>, ty: Ty) {
        self.values.insert(name.to_string(), value);
        self.states.insert(name.to_string(), VarState::Live { ty });
    }

    /// Store a raw i64 value (most common case for REPL results).
    pub fn set_i64(&mut self, name: &str, value: i64, ty: Ty) {
        self.set_value(name, Box::new(value), ty);
    }

    /// Get a value reference by name.
    pub fn get_value(&self, name: &str) -> Option<&Box<dyn Any>> {
        self.values.get(name)
    }

    /// Get the state of a variable.
    pub fn get_state(&self, name: &str) -> Option<&VarState> {
        self.states.get(name)
    }

    /// Mark a variable as moved.
    pub fn mark_moved(&mut self, name: &str, moved_to: &str, input_num: u32) {
        if let Some(VarState::Live { ty }) = self.states.get(name) {
            let ty = ty.clone();
            self.states.insert(
                name.to_string(),
                VarState::Moved {
                    ty,
                    moved_to: moved_to.to_string(),
                    moved_at: input_num,
                },
            );
        }
    }

    /// Check if a variable is live.
    pub fn is_live(&self, name: &str) -> bool {
        matches!(self.states.get(name), Some(VarState::Live { .. }))
    }

    /// Get all live variables with their types.
    pub fn live_variables(&self) -> Vec<(&str, &Ty)> {
        self.states
            .iter()
            .filter_map(|(name, state)| {
                if let VarState::Live { ty } = state {
                    Some((name.as_str(), ty))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Clear all state (for :reset command).
    pub fn reset(&mut self) {
        self.values.clear();
        self.states.clear();
    }

    /// Get all variable states (for :env command).
    pub fn all_states(&self) -> &HashMap<String, VarState> {
        &self.states
    }
}
