//! Compiler driver — orchestrates the phase crates.
//!
//! Phase F of #06.75 introduced this crate as the public face of the
//! Ruxen compiler.  For the present cut, `ruxen_driver` is a thin
//! re-export of `ruxen_core`: every phase still lives in a single
//! `ruxen_core` crate, and the per-phase split into sibling crates
//! (`ruxen_lexer`, `ruxen_parser`, `ruxen_resolve`, …) is tracked as a
//! follow-up.  Future code should `use ruxen_driver::resolve::…`
//! instead of `use ruxen_core::resolve::…` so the eventual per-phase
//! split doesn't require touching every call site.

pub use ruxen_core::*;
