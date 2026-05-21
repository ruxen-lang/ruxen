//! Phase A + Phase B-1/B-2/B-3 pin tests for
//! `docs/specs/types/mixin_vtables.spec.md`.
//!
//! Phase A scope: parser surface + typeck validation. Phase B-1 scope:
//! `ClassInfo.runtime_dispatch_includes` tracks each runtime-dispatch
//! mixin a class includes (DefId list). Phase B-2/B-3: MIR carries
//! `MirVtable`/`MirClassInfo` metadata that codegen emits as static
//! data sections. Phases B-4..B-6 (header alloc, init-time write,
//! pin tests for B-4/B-5) and Phase C/D follow.
//!
//! | Behaviour | Test                                                       | Spec |
//! |-----------|------------------------------------------------------------|------|
//! | B1        | `mixin_dispatch_runtime_modifier_parses`                   | §B1  |
//! | B7 syntax | `dyn_mixin_param_type_parses_and_typechecks`               | §B7  |
//! | E1118     | `amp_mixin_to_static_mixin_emits_e1118`                    | §B7  |
//! | E1117     | `runtime_dispatch_mixin_missing_method_emits_e1117`        | §B1  |
//! | B1        | `class_includes_runtime_mixin_is_tracked_on_classinfo`     | §B-1 |
//! | B-2       | `vtable_struct_emitted_per_implementor`                    | §B3  |
//! | B-3       | `class_info_struct_emitted_per_runtime_dispatch_class`     | §B8  |
//! | B-2/B-3   | `static_mixin_class_produces_no_vtable_or_classinfo`       | §B11 |
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

// ─── Phase B-1 — runtime_dispatch_includes bookkeeping ────────────

/// A class that `include`s a `dispatch runtime` mixin must carry
/// the mixin's DefId on `ClassInfo.runtime_dispatch_includes`.
/// Phases B-2..B-5 consume this list to emit per-mixin vtables,
/// per-class class_info structs, and to inject a class_info_ptr
/// header at allocation time. Without this bookkeeping, codegen
/// can't tell which classes need the header.
#[test]
fn class_includes_runtime_mixin_is_tracked_on_classinfo() {
    let source = rvn("mixin_dispatch_runtime_modifier_parses");
    let mut lx = Lexer::new(&source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);

    // Fixture has `mixin Sized dispatch runtime` + `class Circle;
    // include Sized; ...`. The runtime-dispatch include should land
    // on Circle's ClassInfo.runtime_dispatch_includes.
    let sized_def_id = result
        .symbols
        .iter()
        .find(|d| {
            d.name == "Sized"
                && matches!(
                    &d.kind,
                    riven_core::resolve::symbols::DefKind::Trait { info }
                        if matches!(info.dispatch_mode, DispatchMode::Runtime)
                )
        })
        .map(|d| d.id)
        .expect("mixin `Sized dispatch runtime` should be registered");

    let circle_info = result
        .symbols
        .iter()
        .find(|d| d.name == "Circle")
        .map(|d| match &d.kind {
            riven_core::resolve::symbols::DefKind::Class { info } => info.clone(),
            _ => panic!("Circle is not a class"),
        })
        .expect("class `Circle` should be registered");

    assert!(
        circle_info.runtime_dispatch_includes.contains(&sized_def_id),
        "Circle.runtime_dispatch_includes should contain Sized's DefId; got {:?}",
        circle_info.runtime_dispatch_includes
    );
    // No other includes in this fixture, so list length is 1.
    assert_eq!(
        circle_info.runtime_dispatch_includes.len(),
        1,
        "expected exactly one runtime-dispatch include on Circle, got {:?}",
        circle_info.runtime_dispatch_includes
    );
}

/// Negative: a class that includes only STATIC mixins (`include Foo`
/// where Foo has no `dispatch runtime`) gets an empty list — the
/// existing static-dispatch path is untouched. No vtable header,
/// no class_info struct emission.
#[test]
fn class_with_only_static_mixin_has_empty_runtime_dispatch_list() {
    let source = "
        mixin Plain
          def speak -> Int
        end

        class Bob
          include Plain

          def speak -> Int
            1
          end
        end
    ";
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);

    let bob_info = result
        .symbols
        .iter()
        .find(|d| d.name == "Bob")
        .map(|d| match &d.kind {
            riven_core::resolve::symbols::DefKind::Class { info } => info.clone(),
            _ => panic!(),
        })
        .expect("class `Bob` should be registered");

    assert!(
        bob_info.runtime_dispatch_includes.is_empty(),
        "static-only mixin includes should produce empty runtime_dispatch_includes, got {:?}",
        bob_info.runtime_dispatch_includes
    );
}

// ─── Phase B-2/B-3 — MIR vtable + class_info metadata ─────────────

/// Helper: parse, typecheck, and MIR-lower a fixture, asserting clean
/// typecheck. Returns the lowered `MirProgram`.
fn lower_fixture(name: &str) -> riven_core::mir::nodes::MirProgram {
    let source = rvn(name);
    let prog = parse(&source);
    let result = typeck::type_check(&prog);
    let errs: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errs.is_empty(), "fixture {} typecheck errors: {:?}", name, errs);
    let mut lowerer = riven_core::mir::lower::Lowerer::new(&result.symbols);
    lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed")
}

/// Phase B-2: every class that `include`s a `dispatch runtime` mixin
/// gets exactly one `MirVtable` per `(class, mixin)` pair. The
/// vtable's `method_symbols` are the mangled `<Class>_<method>`
/// callees, ordered by the mixin's `required_methods` declaration.
#[test]
fn vtable_struct_emitted_per_implementor() {
    let mir = lower_fixture("mixin_dispatch_runtime_modifier_parses");

    // Fixture: `mixin Sized dispatch runtime do def size -> Int end`
    // + `class Circle do include Sized; def size -> Int ... end end`.
    // Expect exactly one vtable for (Circle, Sized) holding [Circle_size].
    let circle_sized: Vec<_> = mir
        .vtables
        .iter()
        .filter(|v| v.class_name == "Circle" && v.mixin_name == "Sized")
        .collect();
    assert_eq!(
        circle_sized.len(),
        1,
        "expected exactly one vtable for (Circle, Sized), got: {:?}",
        mir.vtables
            .iter()
            .map(|v| (&v.class_name, &v.mixin_name))
            .collect::<Vec<_>>()
    );
    let vt = circle_sized[0];
    assert_eq!(
        vt.method_symbols,
        vec!["Circle_size".to_string()],
        "vtable method_symbols should be the mangled <Class>_<method> list",
    );
    assert_eq!(
        vt.symbol(),
        "__rvn_vtable_Sized_for_Circle",
        "vtable symbol name follows spec §B3 convention",
    );
}

/// Phase B-3: every class with non-empty `runtime_dispatch_includes`
/// produces exactly one `MirClassInfo` whose `vtable_symbols` are
/// listed in mixin-inclusion order. For the single-mixin fixture, the
/// info has one slot pointing at the one vtable.
#[test]
fn class_info_struct_emitted_per_runtime_dispatch_class() {
    let mir = lower_fixture("mixin_dispatch_runtime_modifier_parses");

    let circle_ci: Vec<_> = mir
        .class_infos
        .iter()
        .filter(|c| c.class_name == "Circle")
        .collect();
    assert_eq!(
        circle_ci.len(),
        1,
        "expected exactly one class_info for Circle, got {}",
        circle_ci.len()
    );
    let ci = circle_ci[0];
    assert_eq!(
        ci.vtable_symbols,
        vec!["__rvn_vtable_Sized_for_Circle".to_string()],
        "class_info should point at the (Circle, Sized) vtable",
    );
    assert_eq!(
        ci.symbol(),
        "__rvn_classinfo_Circle",
        "class_info symbol name follows spec §B8 convention",
    );
}

/// Negative: a class that only includes statically-dispatched mixins
/// (no `dispatch runtime`) produces ZERO vtable and ZERO class_info
/// entries — its layout stays flat, no header is added, and codegen
/// emits no extra data sections. Spec §B11: the layout change only
/// affects classes that opt in.
#[test]
fn static_mixin_class_produces_no_vtable_or_classinfo() {
    // Single static-mixin class — no `dispatch runtime` anywhere.
    let mir = lower_fixture("static_mixin_no_vtable_emission");
    assert!(
        mir.vtables.is_empty(),
        "static-only-mixin program should emit no vtables, got: {:?}",
        mir.vtables.iter().map(|v| v.symbol()).collect::<Vec<_>>()
    );
    assert!(
        mir.class_infos.is_empty(),
        "static-only-mixin program should emit no class_infos, got: {:?}",
        mir.class_infos
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
    );
}
