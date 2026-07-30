//! HIR (High-level Intermediate Representation) for the Ruxen compiler.
//!
//! The HIR is a typed, desugared version of the AST. It is produced by
//! name resolution and type checking, and consumed by the borrow checker
//! (Phase 4) and code generation (Phase 5).

pub mod context;
pub mod nodes;
#[cfg(test)]
mod tests;
pub mod types;

/// The builtin I/O functions whose single argument must be TEXT (a `char*`)
/// by the time it reaches the runtime.
///
/// SINGLE SOURCE, deliberately. Two stages act on this list and they must not
/// drift:
///
///   * `typeck::infer` converts a non-String argument through its universal
///     `to_s` — the first line of defence.
///   * `mir::lower`'s `lower_fn_call` re-checks against the MIR local's type
///     and formats through the Display ladder — the backstop, which exists
///     because the HIR type the typeck pass consults can be wrong (B02, and
///     see B90 for the underlying mistyping).
///
/// Drift in EITHER direction is a bug, which is why this is one list rather
/// than two matching literals. A name only this list's consumers know about
/// would reach `ruxen_puts` unconverted and be dereferenced as a pointer; a
/// name here that does NOT take text would have its argument silently
/// stringified.
///
/// It lives in `hir` because that is the layer both `typeck` and `mir` already
/// depend on — `typeck` must not grow a dependency on `mir`.
pub const IO_TEXT_BUILTINS: &[&str] = &["puts", "eputs", "print", "println", "eprintln"];

pub use context::TypeContext;
pub use nodes::*;
pub use types::{MixinRef, MoveSemantics, Ty, TypeId};
