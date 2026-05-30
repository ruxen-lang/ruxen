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
//! Two invariants pinned here:
//!
//! 1. After anchor merge, the type-scope `String` binding is STILL a
//!    `DefKind::TypeAlias` (not replaced by `DefKind::Class`).
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
fn anchor_mode_preserves_type_alias_for_string() {
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

    // Invariant 1: `String` is still a TypeAlias (canonical Ty::String).
    // Without anchor mode the bootstrap class arm would have replaced
    // this with a fresh `DefKind::Class { ... }` — flip that and the
    // assertion below fails immediately, which is the whole point.
    let symbols = &result.symbols;
    let string_defs: Vec<_> = (0..)
        .take_while(|&i| symbols.get(i as u32).is_some())
        .filter_map(|i| symbols.get(i as u32).map(|d| (i, d)))
        .filter(|(_, d)| d.name == "String")
        .collect();
    let type_alias_count = string_defs
        .iter()
        .filter(|(_, d)| matches!(d.kind, DefKind::TypeAlias { .. }))
        .count();
    let class_count = string_defs
        .iter()
        .filter(|(_, d)| matches!(d.kind, DefKind::Class { .. }))
        .count();
    assert!(
        type_alias_count >= 1,
        "expected at least one `String` DefKind::TypeAlias in symbol table; got defs = {:?}",
        string_defs
            .iter()
            .map(|(i, d)| (
                *i,
                format!("{:?}", d.kind).chars().take(40).collect::<String>()
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        class_count, 0,
        "anchor mode must NOT create a fresh `DefKind::Class` for `String`; got {} class def(s) — \
         this means register_top_level_type_with_ffi fell through the non-anchor branch",
        class_count
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
