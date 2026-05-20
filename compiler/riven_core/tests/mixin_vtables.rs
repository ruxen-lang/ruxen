//! Phase A pin tests for `docs/specs/types/mixin_vtables.spec.md`.
//!
//! Phase A scope: parser surface + typeck validation. Codegen of the
//! per-implementor vtable, class-info struct, and dynamic dispatch
//! helper is Phase B / C. The pin tests below cover:
//!
//! | Behaviour | Test                                                | Spec |
//! |-----------|-----------------------------------------------------|------|
//! | B1        | `mixin_dispatch_runtime_modifier_parses`            | §B1  |
//! | B7 syntax | `dyn_mixin_param_type_parses_and_typechecks`        | §B7  |
//! | E1118     | `amp_mixin_to_static_mixin_emits_e1118`             | §B7  |
//! | E1117     | `runtime_dispatch_mixin_missing_method_emits_e1117` | §B1  |
//!
//! Discipline: all Riven source goes through `.rvn` fixtures
//! (`feedback_no_inline_rvn_in_pin_tests`).

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::ast::{DispatchMode, MixinDef, TopLevelItem};
use riven_core::parser::Parser;
use riven_core::typeck;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn parse(source: &str) -> riven_core::parser::ast::Program {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    p.parse().expect("parse")
}

fn errors(source: &str) -> Vec<Diagnostic> {
    let prog = parse(source);
    let result = typeck::type_check(&prog);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

fn only_mixin(program: &riven_core::parser::ast::Program) -> &MixinDef {
    let mixins: Vec<&MixinDef> = program
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Mixin(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(
        mixins.len(),
        1,
        "expected exactly one mixin, found {}",
        mixins.len()
    );
    mixins[0]
}

// ─── B1: parser captures `dispatch runtime` modifier ────────────────

#[test]
fn mixin_dispatch_runtime_modifier_parses() {
    let source = rvn("mixin_dispatch_runtime_modifier_parses");
    let program = parse(&source);
    let mixin = only_mixin(&program);
    assert_eq!(mixin.name, "Sized");
    assert_eq!(
        mixin.dispatch_mode,
        DispatchMode::Runtime,
        "expected `dispatch runtime` modifier to set dispatch_mode = Runtime",
    );

    // And: a class that includes it and implements the required method
    // typechecks clean (no E1117, no E1118).
    let errs = errors(&source);
    assert!(
        errs.is_empty(),
        "expected no diagnostics, got: {:?}",
        errs.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

#[test]
fn mixin_without_dispatch_runtime_defaults_to_static() {
    // Sanity / regression: ordinary mixins (no modifier) keep the
    // default `DispatchMode::Static`. The fixture is the existing
    // `mixin_and_include.rvn` which predates this feature.
    let source = rvn("mixin_and_include");
    let program = parse(&source);
    let mixin = only_mixin(&program);
    assert_eq!(
        mixin.dispatch_mode,
        DispatchMode::Static,
        "mixins without `dispatch runtime` must stay statically dispatched",
    );
}

// ─── B7 syntax: `&Mixin` / `&var Mixin` parameter types ─────────────

#[test]
fn dyn_mixin_param_type_parses_and_typechecks() {
    let source = rvn("dyn_mixin_param_type_parses_and_typechecks");
    // Parse succeeds (already guaranteed by `parse()` panicking on err).
    let _ = parse(&source);
    // And typeck reports no errors — the dyn-shape reference is valid
    // because `Drawable` is `dispatch runtime`.
    let errs = errors(&source);
    assert!(
        errs.is_empty(),
        "expected typecheck to accept `&Drawable` / `&var Drawable`, got: {:?}",
        errs.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

// ─── E1118: `&Mixin` to a non-runtime-dispatch mixin ────────────────

#[test]
fn amp_mixin_to_static_mixin_emits_e1118() {
    let source = rvn("dyn_mixin_ref_to_static_mixin_emits_e1118");
    let errs = errors(&source);
    let count = errs
        .iter()
        .filter(|d| d.code.as_deref() == Some("E1118"))
        .count();
    assert_eq!(
        count, 1,
        "expected exactly one E1118 for `&Drawable` against a statically-dispatched mixin, \
         got: {:?}",
        errs.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

// ─── E1117: runtime-dispatch mixin missing required method ──────────

#[test]
fn runtime_dispatch_mixin_missing_method_emits_e1117() {
    let source = rvn("runtime_mixin_missing_method_emits_e1117");
    let errs = errors(&source);
    let count = errs
        .iter()
        .filter(|d| d.code.as_deref() == Some("E1117"))
        .count();
    assert_eq!(
        count, 1,
        "expected exactly one E1117 for missing `draw` on Triangle, got: {:?}",
        errs.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    // And the message should name the missing method so the user can
    // act on it.
    let msg = errs
        .iter()
        .find(|d| d.code.as_deref() == Some("E1117"))
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("size"),
        "E1117 message should name the missing method `size`, got: {}",
        msg
    );
}
