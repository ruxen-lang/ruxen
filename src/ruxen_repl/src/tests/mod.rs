//! Crate-level test modules for `ruxen_repl`.
//!
//! `state_persistence` pins the user-visible state-persistence
//! contract across the upcoming `all_statements`-replay removal.
//! `split_chunks` covers the piped-stdin chunker that used to live
//! inline at the bottom of `lib.rs`.

mod single_execution;
mod split_chunks;
mod state_persistence;
