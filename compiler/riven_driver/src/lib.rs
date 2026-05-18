//! Compiler driver — orchestrates the phase crates.
//!
//! Phase F of #06.75 introduced this crate as the public face of the
//! Riven compiler.  For the present cut, `riven_driver` is a thin
//! re-export of `riven_core`: every phase still lives in a single
//! `riven_core` crate, and the per-phase split into sibling crates
//! (`riven_lexer`, `riven_parser`, `riven_resolve`, …) is tracked as a
//! follow-up.  Future code should `use riven_driver::resolve::…`
//! instead of `use riven_core::resolve::…` so the eventual per-phase
//! split doesn't require touching every call site.

pub use riven_core::*;
