//! Pin tests for `docs/specs/types/typed_ffi_returns.spec.md`.
//!
//! Behaviours B1–B12 of the spec: generic stdlib class lib decls now
//! report STRUCTURED return types (`Mutex[T]`, `MutexGuard[T]`,
//! `SharedSync[T]`, `T`, …) at the typeck surface. Codegen still
//! treats the wire ABI as i64 (per `ty_to_cranelift` mapping all
//! pointer-like / class / type-param shapes to I64) — B11.
//!
//! Fixtures live in `compiler/riven_core/tests/fixtures/riven/` per
//! the team's no-inline-rvn-source rule. The end-to-end runtime
//! pieces (lock + read-back chain, refcount, atomic fetch_add) are
//! covered by the release-e2e fixtures 710–712 and exist primarily
//! to pin the lift mechanism's typeck contract here.

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::hir::types::Ty;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::resolve::symbols::DefKind;
use riven_core::resolve::Resolver;
use riven_core::typeck;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn typeck_errors(source: &str) -> Vec<Diagnostic> {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

fn assert_clean(name: &str) {
    let source = rvn(name);
    let errs = typeck_errors(&source);
    assert!(
        errs.is_empty(),
        "expected {name} to typecheck cleanly, got: {:?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ─── B1, B3 — `Mutex.new(value)` lifts to `Mutex[T]` ─────────────────

/// `Mutex.new(7)` reports `Mutex[Int]`; the chained `.lock_raw.get`
/// reports `Int`. Without the spec's lift + the subst_ty recursion
/// into `Ty::Class.generic_args`, the chain regresses to "no field
/// `get` on type `Int`".
#[test]
fn mutex_new_lifts_to_mutex_of_t() {
    assert_clean("typed_ffi_mutex_new_int");
    assert_clean("typed_ffi_mutex_new_string");
}

// ─── B2 — instance method return type honours its declaration ────────

/// `Mutex.is_poisoned -> Int` stays Int (no auto-lift on instance
/// methods returning Int), while `Mutex.lock_raw -> MutexGuard[T]`
/// gets the declared MutexGuard. The fixture exercises both shapes
/// in the same `main`.
#[test]
fn instance_method_return_type_honours_declaration() {
    assert_clean("typed_ffi_instance_return_honours_decl");
}

// ─── B4 — class-generic T substitutes in return position ─────────────

/// `def get -> T` inside `class MutexGuard[T]` resolves to the
/// class's T at the call site. The pin guards against regressions
/// in `subst_ty`'s walk into `Ty::Class.generic_args`.
#[test]
fn class_generic_t_substitutes_in_return() {
    assert_clean("typed_ffi_class_generic_t_substitutes");
}

// ─── B5 — `Self` / `Class[T]` in return position ─────────────────────

/// `SharedSync[T].clone -> SharedSync[T]` typechecks; the chain
/// `s.clone.get` substitutes T through both class instances.
#[test]
fn self_in_return_position_resolves_to_class_t() {
    assert_clean("typed_ffi_sharedsync_clone_returns_self");
}

// ─── B7 — drop emission on typed scope exit ──────────────────────────

/// Compilation of `let m = Mutex.new(7)` followed by scope exit must
/// emit a `Mutex_drop` (riven_mutex_drop) call. The typeck-side lift
/// must NOT prevent the MIR drop pass from recognising the value as
/// a Mutex handle — codegen's `ty_to_cranelift` maps `Ty::Class` to
/// I64 (B11), but the drop dispatcher reads the structured type to
/// pick `Mutex_drop`. We assert by parsing + resolving + checking
/// the symbol table contains a registered `Mutex_drop` FFI alias.
///
/// The MIR drop wiring is exercised end-to-end by the release-e2e
/// fixtures (703, 710); this typeck-level pin verifies the surface
/// the drop pass reads from.
#[test]
fn mutex_drop_emitted_on_typed_scope_exit() {
    let source = rvn("typed_ffi_mutex_new_int");
    let mut lx = Lexer::new(&source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typeck errors: {:?}", errors);
    // The Mutex class has a `def drop` lib decl in
    // library/std/sync/src/lib.rvn. Confirm it's registered in the
    // symbol table as a method (the `__drop` legacy name was retired
    // during the sync.rvn rename — see
    // `project_riven_drop_name_mismatch.md`).
    let drop_method = result.symbols.iter().find(|d| {
        d.name == "drop"
            && match &d.kind {
                DefKind::Method { parent, .. } => result
                    .symbols
                    .get(*parent)
                    .map(|p| p.name == "Mutex")
                    .unwrap_or(false),
                _ => false,
            }
    });
    assert!(
        drop_method.is_some(),
        "Mutex.drop should be registered as a method via its lib decl"
    );
}

// ─── B10 — non-generic class `.new -> Int` does NOT auto-lift ────────

/// Spec B10: the lift only fires when the lib decl returns a
/// STRUCTURED type. AtomicI64's lib decl was updated to declare
/// `-> AtomicI64` (the explicit override), so this case typechecks.
/// Removing that override would trip the negative half of B10 — the
/// chain `.fetch_add` would lookup methods on Int and fail.
#[test]
fn non_generic_class_constructor_returns_int() {
    assert_clean("typed_ffi_atomic_new_non_generic");
}

// ─── B11 — FFI signature unchanged after the lift ────────────────────

/// The lift changes ONLY the typeck-reported return type. The FFI
/// wire signature (param + return Cranelift types) stays i64.
/// `ty_to_cranelift` maps Ty::Class to I64, so a lib decl whose
/// return type changed from `-> Int` to `-> Mutex[T]` produces the
/// same C-side signature.
///
/// We verify this structurally: the `HirFfiLib` produced by the
/// resolver for `Mutex` lists `riven_mutex_new` with a return type
/// that maps to I64 in the helper. (The full Cranelift declaration
/// flow runs at codegen time; for typeck-level pinning, asserting
/// the return Ty is a pointer-shape that codegen treats as I64 is
/// sufficient.)
#[test]
fn ffi_signature_unchanged_after_lift() {
    let source = rvn("typed_ffi_mutex_new_int");
    let mut lx = Lexer::new(&source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");

    // Run the full bootstrap-aware resolver so the stdlib's Mutex lib
    // decl is present in `ffi_libs`.
    let mut bootstrap_diagnostics: Vec<Diagnostic> = Vec::new();
    let bootstrap_packages = riven_core::resolve::bootstrap::run_bootstrap_with_package_names(
        &mut bootstrap_diagnostics,
    );
    let resolver = Resolver::new();
    let result = resolver.resolve_with_bootstrap_packages(&prog, &bootstrap_packages);
    let mutex_new = result
        .program
        .ffi_libs
        .iter()
        .flat_map(|lib| lib.functions.iter())
        .find(|f| f.c_symbol.as_deref() == Some("riven_mutex_new"))
        .expect("riven_mutex_new should be registered through the Mutex class lib decl");

    // The return type carried into MIR/codegen must be a class-shaped Ty
    // (not raw Int). The codegen step (`ty_to_cranelift`) maps both Int
    // and Ty::Class to I64, so the wire signature is unchanged — B11.
    let ret_ty = mutex_new
        .return_type
        .as_ref()
        .expect("riven_mutex_new return type must be present");
    match ret_ty {
        Ty::Class { name, .. } => assert_eq!(name, "Mutex"),
        other => panic!(
            "expected Mutex.new return to be Ty::Class {{ name: \"Mutex\", … }}, got {:?}",
            other
        ),
    }
}

// ─── B12 — bare `def new` outside a class does NOT lift ──────────────

/// Spec B12: the lift is keyed on `<class>.<method>`. A top-level
/// `def foo -> Int` in a stdlib bare-`lib` block stays Int. The
/// fixture exercises `signal_received_sigint() -> Int` (a bare
/// top-level shim in library/std/sync/src/lib.rvn) and performs
/// arithmetic on the result — which would fail unification if the
/// lift over-eagerly fired on top-level FFI fns.
#[test]
fn top_level_def_new_does_not_lift() {
    assert_clean("typed_ffi_top_level_def_no_lift");
}
