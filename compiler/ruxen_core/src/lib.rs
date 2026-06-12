// Doc comments in this crate use aligned annotation tables and hanging
// bullet continuations (see e.g. typeck/mixins.rs `method_home_key`); the
// pedantic doc-list indentation lints fight that deliberate style.
#![allow(clippy::doc_overindented_list_items)]
#![allow(clippy::doc_lazy_continuation)]

pub mod async_lowering;
pub mod borrow_check;
pub mod codegen;
pub mod diagnostics;
pub mod formatter;
pub mod hir;
pub mod implicit_includes;
pub mod lexer;
pub mod mir;
pub mod no_std;
pub mod parser;
pub mod resolve;
pub mod typeck;
