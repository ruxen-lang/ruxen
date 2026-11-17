//! `std` module tree registration.
//!
//! Task #17 (Phase D-4 of #06.95 — auto-derived): the std-submodule
//! list is now derived from `bootstrap::BOOTSTRAP_FILES` rather than
//! hand-maintained here. Each package entry in BOOTSTRAP_FILES
//! (basename of `<pkg>/src/lib.rx`) becomes one `std.<pkg>` empty
//! `DefKind::Module`; `auto_populate_std_submodules_from_packages`
//! then fills the items list from the matching package after the
//! bootstrap merge. Adding a new stdlib package = one line in
//! BOOTSTRAP_FILES; `use std.<new>.X` resolves automatically.
//!
//! Every `std.<pkg>` submodule comes from a bootstrap package; there
//! are no synthetic namespaces.

use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

use super::super::symbols::*;
use super::super::Resolver;

pub(super) fn register_modules(r: &mut Resolver) {
    let span = Span {
        start: 0,
        end: 0,
        line: 0,
        column: 0,
    };

    let mut submodule_names: Vec<&str> = Vec::new();
    for name in crate::resolve::bootstrap::bootstrap_package_names() {
        if !submodule_names.contains(&name) {
            submodule_names.push(name);
        }
    }
    let std_items: Vec<_> = submodule_names
        .iter()
        .map(|name| {
            r.symbols.define(
                (*name).to_string(),
                DefKind::Module { items: vec![] },
                Visibility::Public,
                span.clone(),
            )
        })
        .collect();
    let std_id = r.symbols.define(
        "std".to_string(),
        DefKind::Module { items: std_items },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("std".to_string(), std_id);
    r.type_registry.insert("std".to_string(), std_id);
}
