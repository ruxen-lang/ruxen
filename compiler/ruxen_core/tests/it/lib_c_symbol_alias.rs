//! Pin tests for #06.8 Wave 1 Task 0a: the `as "<c-symbol>"` rename
//! clause on FFI defs inside `lib "X" ... end` blocks.
//!
//! The clause lets a Ruxen-side identifier bind to a verbatim C symbol
//! whose name differs from the Ruxen name — the per-decl rename surface
//! stdlib self-hosting needs so `File.open` can bind to `ruxen_file_open`
//! without forcing the Ruxen method name to be `ruxen_file_open`.
//!
//! Scope of THIS commit: parser + AST only. The alias is captured on
//! `FfiFunction.c_symbol` and round-tripped by the formatter. Resolver,
//! MIR, and codegen consumption of the alias is intentionally deferred
//! to a follow-up commit (it requires plumbing the field through ~20
//! `FnSignature` construction sites and adding the missing
//! `LibDecl → FfiLib` MIR-lowering bridge that does not exist today).

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::ast::{FfiFunction, LibDecl, TopLevelItem};
use ruxen_core::parser::Parser;

fn parse_fixture(path: &str) -> Result<ruxen_core::parser::ast::Program, String> {
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

fn only_lib_decl(program: &ruxen_core::parser::ast::Program) -> &LibDecl {
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

fn find_fn<'a>(lib: &'a LibDecl, ruxen_name: &str) -> &'a FfiFunction {
    lib.functions
        .iter()
        .find(|f| f.name == ruxen_name)
        .unwrap_or_else(|| panic!("no FFI fn named `{}` in lib", ruxen_name))
}

#[test]
fn lib_ffi_def_with_alias_stores_c_symbol() {
    let program = parse_fixture("tests/fixtures/ruxen/lib_c_symbol_alias_present.rx")
        .expect("fixture should parse");
    let lib = only_lib_decl(&program);
    let f = find_fn(lib, "add_one");
    assert_eq!(
        f.c_symbol.as_deref(),
        Some("ruxen_test_add_one"),
        "expected `as \"ruxen_test_add_one\"` to land on FfiFunction.c_symbol"
    );
}

#[test]
fn lib_ffi_def_without_alias_leaves_c_symbol_none() {
    let program = parse_fixture("tests/fixtures/ruxen/lib_c_symbol_alias_absent.rx")
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
    // a wrapper-only Ruxen method sits next to a directly-bound one.
    let program = parse_fixture("tests/fixtures/ruxen/lib_c_symbol_alias_mixed.rx")
        .expect("fixture should parse");
    let lib = only_lib_decl(&program);

    let add_one = find_fn(lib, "add_one");
    assert_eq!(add_one.c_symbol.as_deref(), Some("ruxen_test_add_one"));

    let double = find_fn(lib, "double");
    assert!(
        double.c_symbol.is_none(),
        "`double` has no `as` clause → c_symbol should be None"
    );

    let sub = find_fn(lib, "sub");
    assert_eq!(sub.c_symbol.as_deref(), Some("ruxen_test_sub"));
}

#[test]
fn lib_ffi_def_alias_must_be_string_literal() {
    // `def f as 42(...)` is a parser error — the alias must be a string
    // literal. The error is recoverable (parser carries on so it still
    // emits a Program), but the diagnostic must mention the malformed
    // alias clause so a contributor can self-serve the fix.
    let source = std::fs::read_to_string("tests/fixtures/ruxen/lib_c_symbol_alias_not_string.rx")
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
    // pretty-printing a parsed `lib` block (e.g. via `ruxenc fmt`)
    // does not silently strip the binding and link the wrong symbol.
    let source = std::fs::read_to_string("tests/fixtures/ruxen/lib_c_symbol_alias_present.rx")
        .expect("fixture should exist");
    let result = ruxen_core::formatter::format(&source);
    assert!(
        result.errors.is_empty(),
        "formatter should not produce diagnostics on this fixture; got {:?}",
        result.errors
    );
    assert!(
        result.output.contains("as \"ruxen_test_add_one\""),
        "formatted output should preserve the `as \"...\"` alias clause; got:\n{}",
        result.output
    );
}

/// `def consume drop as "..."` declares a drop that takes the whole handle
/// over and releases its storage: the resolver records a consuming receiver
/// and `SymbolTable::drop_consumes_self` answers true, so drop elaboration
/// emits no storage free after it. The stdlib's refcounted handles use it;
/// a drop that only cleans up resources (`MutexGuard`, `File`) does not.
#[test]
fn def_consume_drop_marks_a_drop_that_releases_its_storage() {
    let source = "\
class Handle
  lib \"runtime/handle.c\"
    def self.open as \"handle_open\" -> Handle
    def consume drop as \"handle_release\" -> nil
  end
end

class Plain
  lib \"runtime/plain.c\"
    def self.open as \"plain_open\" -> Plain
    def drop as \"plain_close\" -> nil
  end
end
";
    let tokens = Lexer::new(source).tokenize().expect("lex");
    let program = Parser::new(tokens).parse().expect("parse");
    let formatted = ruxen_core::formatter::format(source).output;
    assert!(
        formatted.contains("def consume drop as \"handle_release\" -> nil"),
        "fmt dropped `consume`:\n{formatted}"
    );
    let result = ruxen_core::typeck::type_check(&program);
    assert!(result.symbols.drop_consumes_self("Handle"));
    assert!(!result.symbols.drop_consumes_self("Plain"));
    for name in ["Mutex", "SharedSync", "Sender", "Receiver", "AtomicI64"] {
        assert!(result.symbols.drop_consumes_self(name), "{name}");
    }
    for name in ["MutexGuard", "File", "JoinHandle"] {
        assert!(!result.symbols.drop_consumes_self(name), "{name}");
    }
}

/// `-> owned T` on a `lib` declaration says the C function returns a value
/// the caller owns outright (F-25's ownership markers, I-19): it parses, the
/// formatter keeps it, it reaches the MIR declaration, and the runtime ABI
/// reads it before its hand-listed table. `Proc#clone` (`class Closure`,
/// std/core) declares its C share this way instead of a table row.
#[test]
fn owned_result_marker_parses_round_trips_and_classifies_fresh() {
    use ruxen_core::mir::lower::runtime_abi::{
        callee_ownership, result_ownership, DeclaredResults, ResultOwnership,
    };
    let source = "\
class Handle
  lib \"runtime/handle.c\"
    def self.open as \"handle_open\" -> owned Handle
    def peek as \"handle_peek\" -> Handle
  end
end
";
    let tokens = Lexer::new(source).tokenize().expect("lex");
    let program = Parser::new(tokens).parse().expect("parse");
    let formatted = ruxen_core::formatter::format(source).output;
    assert!(
        formatted.contains("def self.open as \"handle_open\" -> owned Handle"),
        "fmt dropped `owned`:\n{formatted}"
    );
    let result = ruxen_core::typeck::type_check(&program);
    let mir =
        ruxen_core::mir::lower::lower_program(&result.program, &result.symbols).expect("lower");
    let declared = DeclaredResults::from_libs(&mir.ffi_libs);
    assert_eq!(
        result_ownership("handle_open", &declared),
        ResultOwnership::Fresh
    );
    assert_eq!(
        result_ownership("handle_peek", &declared),
        ResultOwnership::None
    );
    // The closure share is declared owned in std/core, not listed.
    assert_eq!(
        callee_ownership("ruxen_closure_share").result,
        ResultOwnership::None
    );
    assert_eq!(
        result_ownership("ruxen_closure_share", &declared),
        ResultOwnership::Fresh
    );
}
