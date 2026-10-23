//! The ruxen_repl integration tests, one binary: each file in this directory
//! is a module. Run one with `cargo test -p ruxen_repl --test it -- <module>::`.
//!
//! Tests here share one process: never mutate process-wide state
//! (environment variables, the current directory).

// ruxen_core's shared test helpers: the front end and the Cranelift
// compile-and-run the JIT parity lane compares against.
#[path = "../../../../compiler/ruxen_core/tests/common/mod.rs"]
mod common;
mod installed_repl;
mod jit_parity;
