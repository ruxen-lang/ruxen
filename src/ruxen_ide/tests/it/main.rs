//! The ruxen_ide integration tests, one binary: each file in this directory
//! is a module. Run one with `cargo test -p ruxen_ide --test it -- <module>::`.
//!
//! Tests here share one process: never mutate process-wide state
//! (environment variables, the current directory).

mod autocall_recognition;
mod code_actions_basics;
mod completion_basics;
mod document_symbols_basics;
mod folding_basics;
mod format_basics;
mod highlight_basics;
mod hover_parity;
mod inlay_hints_basics;
mod integration;
mod references_basics;
mod rename_basics;
mod signature_help_basics;
mod syntax_parity_ide;
mod type_def_basics;
mod use_index_basics;
mod workspace_symbols_basics;
