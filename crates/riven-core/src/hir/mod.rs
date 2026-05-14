//! HIR (High-level Intermediate Representation) for the Riven compiler.
//!
//! The HIR is a typed, desugared version of the AST. It is produced by
//! name resolution and type checking, and consumed by the borrow checker
//! (Phase 4) and code generation (Phase 5).

pub mod context;
pub mod nodes;
#[cfg(test)]
mod tests;
pub mod types;

pub use context::TypeContext;
pub use nodes::*;
pub use types::{MoveSemantics, MixinRef, Ty, TypeId};
