//! Pin tests for #06.8 Wave 1 Task 0a: the `as "<c-symbol>"` rename
//! clause on FFI defs inside `lib "X" ... end` blocks.
//!
//! The clause lets a Riven-side identifier bind to a verbatim C symbol
//! whose name differs from the Riven name — the per-decl rename surface
//! stdlib self-hosting needs so `File.open` can bind to `riven_file_open`
//! without forcing the Riven method name to be `riven_file_open`.
//!
//! Scope of THIS commit: parser + AST only. The alias is captured on
//! `FfiFunction.c_symbol` and round-tripped by the formatter. Resolver,
//! MIR, and codegen consumption of the alias is intentionally deferred
//! to a follow-up commit (it requires plumbing the field through ~20
//! `FnSignature` construction sites and adding the missing
//! `LibDecl → FfiLib` MIR-lowering bridge that does not exist today).

use riven_core::lexer::Lexer;
use riven_core::parser::ast::{FfiFunction, LibDecl, TopLevelItem};
use riven_core::parser::Parser;

fn parse_fixture(path: &str) -> Result<riven_core::parser::ast::Program, String> {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {}", path, e));
    let mut lexer = Lexer::new(&source);
    let tokens = lexer
        .tokenize()
        .map_err(|d| format!("lexer errors: {:?}", d))?;
    let mut parser = Parser::new(tokens);
    parser
        .parse()
        .map_err(|d| format!("parser errors: {:?}", d))
}

fn only_lib_decl(program: &riven_core::parser::ast::Program) -> &LibDecl {
    let libs: Vec<&LibDecl> = program
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Lib(l) => Some(l),
            _ => None,
        })
        .collect();
    assert_eq!(
        libs.len(),
        1,
        "expected exactly one `lib` decl, found {}",
        libs.len()
    );
    libs[0]
}

fn find_fn<'a>(lib: &'a LibDecl, riven_name: &str) -> &'a FfiFunction {
    lib.functions
        .iter()
        .find(|f| f.name == riven_name)
        .unwrap_or_else(|| panic!("no FFI fn named `{}` in lib", riven_name))
}

#[test]
fn lib_ffi_def_with_alias_stores_c_symbol() {
    let program = parse_fixture("tests/fixtures/riven/lib_c_symbol_alias_present.rvn")
        .expect("fixture should parse");
    let lib = only_lib_decl(&program);
    let f = find_fn(lib, "add_one");
    assert_eq!(
        f.c_symbol.as_deref(),
        Some("riven_test_add_one"),
        "expected `as \"riven_test_add_one\"` to land on FfiFunction.c_symbol"
    );
}

#[test]
fn lib_ffi_def_without_alias_leaves_c_symbol_none() {
    let program = parse_fixture("tests/fixtures/riven/lib_c_symbol_alias_absent.rvn")
        .expect("fixture should parse");
    let lib = only_lib_decl(&program);
    let f = find_fn(lib, "add_one");
    assert!(
        f.c_symbol.is_none(),
        "no `as \"...\"` clause should leave c_symbol = None; got {:?}",
        f.c_symbol
    );
}

#[test]
fn lib_ffi_block_mixes_aliased_and_unaliased_defs() {
    // The alias is per-decl, not per-block. A block may mix defs with
    // and without `as "..."` freely — stdlib will do exactly this when
    // a wrapper-only Riven method sits next to a directly-bound one.
    let program = parse_fixture("tests/fixtures/riven/lib_c_symbol_alias_mixed.rvn")
        .expect("fixture should parse");
    let lib = only_lib_decl(&program);

    let add_one = find_fn(lib, "add_one");
    assert_eq!(add_one.c_symbol.as_deref(), Some("riven_test_add_one"));

    let double = find_fn(lib, "double");
    assert!(
        double.c_symbol.is_none(),
        "`double` has no `as` clause → c_symbol should be None"
    );

    let sub = find_fn(lib, "sub");
    assert_eq!(sub.c_symbol.as_deref(), Some("riven_test_sub"));
}

#[test]
fn lib_ffi_def_alias_must_be_string_literal() {
    // `def f as 42(...)` is a parser error — the alias must be a string
    // literal. The error is recoverable (parser carries on so it still
    // emits a Program), but the diagnostic must mention the malformed
    // alias clause so a contributor can self-serve the fix.
    let source = std::fs::read_to_string("tests/fixtures/riven/lib_c_symbol_alias_not_string.rvn")
        .expect("fixture should exist");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer should succeed");
    let mut parser = Parser::new(tokens);
    let result = parser.parse();
    assert!(
        result.is_err(),
        "non-string alias should produce a parser diagnostic; got Ok"
    );
    let diags = result.unwrap_err();
    let messages: String = diags
        .iter()
        .map(|d| format!("{:?}", d))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("string literal") && messages.contains("as"),
        "diagnostic should mention `as` and a string literal requirement; got:\n{}",
        messages
    );
}

#[test]
fn formatter_round_trips_c_symbol_alias() {
    // The formatter must preserve `as "<c-symbol>"` clauses so that
    // pretty-printing a parsed `lib` block (e.g. via `rivenc fmt`)
    // does not silently strip the binding and link the wrong symbol.
    let source = std::fs::read_to_string("tests/fixtures/riven/lib_c_symbol_alias_present.rvn")
        .expect("fixture should exist");
    let result = riven_core::formatter::format(&source);
    assert!(
        result.errors.is_empty(),
        "formatter should not produce diagnostics on this fixture; got {:?}",
        result.errors
    );
    assert!(
        result.output.contains("as \"riven_test_add_one\""),
        "formatted output should preserve the `as \"...\"` alias clause; got:\n{}",
        result.output
    );
}
