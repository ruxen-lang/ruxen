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
//! SYNTHETIC_STD_SUBMODULES carries the namespaces that do NOT have
//! a same-named bootstrap package — `thread` and `signal` re-export
//! sync.rx's bare-fn shims (`sleep`, `signal_install_sigint`,
//! `signal_received_sigint`) under the legacy import paths
//! `use std.thread.sleep` / `use std.signal.*`. The resolver's
//! auto-population skips them silently (no matching package in
//! `bootstrap_auto_packages`) and they stay empty modules — fine
//! for namespace tokenisation; callers go through the global-prelude
//! entries for the actual fn resolution.
//!
//! Ordering: bootstrap-package basenames first (in BOOTSTRAP_FILES
//! order), then synthetic. Duplicates between the two sets are
//! dropped (the synthetic list wins only if the name is unique).

use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

use super::super::symbols::*;
use super::super::Resolver;

const SYNTHETIC_STD_SUBMODULES: &[&str] = &["thread", "signal"];

pub(super) fn register_modules(r: &mut Resolver) {
    let span = Span {
        start: 0,
        end: 0,
        line: 0,
        column: 0,
    };

    let bootstrap_pkg_names = crate::resolve::bootstrap::bootstrap_package_names();
    let mut submodule_names: Vec<&str> =
        Vec::with_capacity(bootstrap_pkg_names.len() + SYNTHETIC_STD_SUBMODULES.len());
    for name in &bootstrap_pkg_names {
        if !submodule_names.contains(name) {
            submodule_names.push(*name);
        }
    }
    for name in SYNTHETIC_STD_SUBMODULES {
        if !submodule_names.contains(name) {
            submodule_names.push(*name);
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
