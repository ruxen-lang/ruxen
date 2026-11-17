//! The ruxenc integration tests, one binary: each file in this directory
//! is a module. Run one with `cargo test -p ruxenc --test it -- <module>::`.
//!
//! Tests here share one process: never mutate process-wide state
//! (environment variables, the current directory).

mod cache_integration;
mod installed_binary;
