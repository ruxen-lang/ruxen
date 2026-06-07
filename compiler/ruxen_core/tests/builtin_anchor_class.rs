//! Pin tests for #06.8 T#21: namespace-anchor mode for builtin class
//! names in bootstrap `.rx` files.
//!
//! `register_builtins` (resolve/stdlib/mod.rs) registers `String` as a
//! `DefKind::TypeAlias { target: Ty::String }` in the type scope.
//! Plain bootstrap class registration would `HashMap::insert` over that
//! binding with a fresh `DefKind::Class`, which silently changes what
//! `let s: String = ...` resolves to: instead of `Ty::String`
//! (canonical, codegen-aware) it would be `Ty::Class { name: "String"
//! ... }` — a different runtime layout for the whole compilation.
//!
//! T#21 introduces an **anchor mode**: a bootstrap `class Foo do lib
//! ... end end` whose name already lives in the type scope reuses the
//! existing DefId instead of redefining it. The class-body `lib` FFI
//! decls still get registered (so they appear in `HirProgram.ffi_libs`
//! and therefore in MIR's `ffi_alias_map`), but the type-scope binding
//! is untouched.
//!
//! Invariants pinned here (post zero-Rust-stdlib migration, Phase B/M3):
//!
//! 1. After the anchor merge + `resolve_class`, the `String` binding is a
//!    `DefKind::Class` (its METHOD-HOME) — BUT the `String` type
//!    annotation still resolves to the primitive `Ty::String` head, not
//!    `Ty::Class { name: "String" }`. The conversion changed the
//!    method-home, NOT the value representation.
//! 2. The class-body `lib` decl lands on `HirProgram.ffi_libs` with the
//!    mangled `String_<method>` ruxen_name and the C-symbol alias —
//!    proving the FFI route is wired all the way through to MIR.

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::ast::Program;
use ruxen_core::parser::Parser;
use ruxen_core::resolve::symbols::DefKind;
use ruxen_core::resolve::Resolver;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn parse_fixture(name: &str) -> Program {
    let path = format!(
        "{}/compiler/ruxen_core/tests/fixtures/ruxen/{}.rx",
        workspace_root().display(),
        name
    );
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    parser.parse().expect("parse")
}

#[test]
fn string_is_class_but_resolves_to_primitive_ty() {
    // Zero-Rust-stdlib migration (Phase B / M3): `String` is now a real
    // `DefKind::Class` (its method-home, so `register_classes_from_registry`
    // lifts its `.rx` methods into `type_methods["String"]` for the general
    // `lookup_method_with_args` path). The CRITICAL INVARIANT is that this
    // changed only the METHOD-HOME, not the VALUE REPRESENTATION: the
    // `String` type annotation must STILL resolve to the primitive
    // `Ty::String` head (not `Ty::Class { name: "String" }`), preserving
    // the runtime layout / C ABI / every `Ty::String`-matching consumer.
    // `resolve_type_expr` enforces this via its `name == "String"`
    // normalization arm (resolve/types.rs).
    use ruxen_core::hir::types::Ty;
    use ruxen_core::resolve::symbols::FnSignature;

    let bootstrap_program = parse_fixture("builtin_anchor_string_lib");
    let user_program = parse_fixture("builtin_anchor_string_caller");

    let resolver = Resolver::new();
    let result = resolver.resolve_with_bootstrap(&user_program, &[bootstrap_program]);

    let errors: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "resolver should not produce errors on anchored bootstrap class; got {:?}",
        errors
    );

    let symbols = &result.symbols;

    // Invariant 1 (NEW): the anchored `String` binding is now a
    // `DefKind::Class` carrying its `.rx` methods — the method-home.
    let string_class = symbols
        .iter()
        .any(|d| d.name == "String" && matches!(d.kind, DefKind::Class { .. }));
    assert!(
        string_class,
        "expected `String` to be a DefKind::Class (method-home) after the \
         zero-Rust-stdlib conversion; it is no longer kept as a TypeAlias"
    );

    // Invariant 2 (the load-bearing one): the `String` *annotation* on
    // `takes_string(s: String)` still resolved to the primitive
    // `Ty::String` head — NOT `Ty::Class { name: "String" }`. This is the
    // representation invariant the conversion must not break.
    let sig: &FnSignature = symbols
        .iter()
        .find_map(|d| match &d.kind {
            DefKind::Function { signature } if d.name == "takes_string" => Some(signature),
            _ => None,
        })
        .expect("takes_string should be registered");
    let param_ty = &sig.params.first().expect("one param").ty;
    assert_eq!(
        param_ty,
        &Ty::String,
        "the `String` annotation must resolve to the primitive `Ty::String` \
         head even though `String` is now a DefKind::Class; got {param_ty:?}"
    );
}

#[test]
fn anchor_mode_routes_class_body_lib_decl_into_ffi_libs() {
    let bootstrap_program = parse_fixture("builtin_anchor_string_lib");
    let user_program = parse_fixture("builtin_anchor_string_caller");

    let resolver = Resolver::new();
    let result = resolver.resolve_with_bootstrap(&user_program, &[bootstrap_program]);

    // Invariant 2: the lib decl flows through `register_class_lib_method`
    // even though we reused the existing TypeAlias DefId as parent.
    // The mangled ruxen_name is `String_len_via_anchor` (the parent name
    // is taken from the source `class String`, not the existing DefKind);
    // the c_symbol is the verbatim alias from the lib block.
    let string_lib = result
        .program
        .ffi_libs
        .iter()
        .find(|lib| lib.name == "String")
        .unwrap_or_else(|| {
            panic!(
                "expected an HirFfiLib named 'String' from the anchored class body; \
                 got libs = {:?}",
                result
                    .program
                    .ffi_libs
                    .iter()
                    .map(|l| &l.name)
                    .collect::<Vec<_>>()
            )
        });
    let anchored = string_lib
        .functions
        .iter()
        .find(|f| f.ruxen_name == "String_len_via_anchor")
        .unwrap_or_else(|| {
            panic!(
                "expected ruxen_name 'String_len_via_anchor' in String ffi lib; got fns = {:?}",
                string_lib
                    .functions
                    .iter()
                    .map(|f| &f.ruxen_name)
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        anchored.c_symbol.as_deref(),
        Some("ruxen_string_len"),
        "anchored FFI decl must carry the verbatim C-symbol alias from the lib block"
    );
}
