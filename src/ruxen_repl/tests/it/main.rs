//! The ruxen_repl integration tests, one binary: each file in this directory
//! is a module. Run one with `cargo test -p ruxen_repl --test it -- <module>::`.
//!
//! Tests here share one process: never mutate process-wide state
//! (environment variables, the current directory).

mod installed_repl;
