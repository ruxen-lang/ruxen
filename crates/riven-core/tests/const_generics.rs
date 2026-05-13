//! Pin tests for `docs/specs/types/const-generics.spec.md`.
//!
//! **Stage 1 (parser):** `const N: Type` is accepted in generic-param
//! positions on every declarator that takes generic parameters today
//! (`struct`, `class`, `enum`, `def`, `impl`, `trait`).  The AST stores
//! the result as `GenericParam::Const { name, ty, span }`.  No semantic
//! checks yet — resolve / typeck / monomorphization land in S3-S6.

use riven_core::lexer::Lexer;
use riven_core::parser::ast::{GenericParam, Program, TopLevelItem, TypeExpr};
use riven_core::parser::Parser;

fn parse(src: &str) -> Program {
    let mut lx = Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    match p.parse() {
        Ok(prog) => prog,
        Err(diags) => panic!("parse failed: {:#?}", diags),
    }
}

fn first_struct_generic_params(prog: &Program, name: &str) -> Vec<GenericParam> {
    for item in &prog.items {
        if let TopLevelItem::Struct(s) = item {
            if s.name == name {
                return s
                    .generic_params
                    .as_ref()
                    .map(|gp| gp.params.clone())
                    .unwrap_or_default();
            }
        }
    }
    panic!("no struct `{}` in program", name);
}

fn first_class_generic_params(prog: &Program, name: &str) -> Vec<GenericParam> {
    for item in &prog.items {
        if let TopLevelItem::Class(c) = item {
            if c.name == name {
                return c
                    .generic_params
                    .as_ref()
                    .map(|gp| gp.params.clone())
                    .unwrap_or_default();
            }
        }
    }
    panic!("no class `{}` in program", name);
}

fn first_fn_generic_params(prog: &Program, name: &str) -> Vec<GenericParam> {
    for item in &prog.items {
        if let TopLevelItem::Function(f) = item {
            if f.name == name {
                return f
                    .generic_params
                    .as_ref()
                    .map(|gp| gp.params.clone())
                    .unwrap_or_default();
            }
        }
    }
    panic!("no def `{}` in program", name);
}

fn assert_const_param(p: &GenericParam, expected_name: &str, expected_ty_name: &str) {
    match p {
        GenericParam::Const { name, ty, .. } => {
            assert_eq!(name, expected_name, "const param name mismatch");
            match ty {
                TypeExpr::Named(path) => {
                    assert_eq!(
                        path.segments.last().map(|s| s.as_str()),
                        Some(expected_ty_name),
                        "const param type path tail mismatch"
                    );
                }
                other => panic!(
                    "expected const param `{}` to have a Named type, got {:?}",
                    expected_name, other
                ),
            }
        }
        other => panic!(
            "expected GenericParam::Const {{ name: {:?}, ... }}, got {:?}",
            expected_name, other
        ),
    }
}

/// B1 (struct): `struct Vector[T, const N: USize]` parses; the second
/// generic param is a `Const` with name `N` and type `USize`.
#[test]
fn parse_const_generic_param_on_struct() {
    let src = r#"
struct Vector[T, const N: USize]
  data: USize
end
"#;
    let prog = parse(src);
    let params = first_struct_generic_params(&prog, "Vector");
    assert_eq!(params.len(), 2, "expected 2 generic params, got {:?}", params);
    match &params[0] {
        GenericParam::Type { name, .. } => assert_eq!(name, "T"),
        other => panic!("expected first param to be Type {{ name: \"T\" }}, got {:?}", other),
    }
    assert_const_param(&params[1], "N", "USize");
}

/// B1 (class): same form on `class`.
#[test]
fn parse_const_generic_param_on_class() {
    let src = r#"
class SmallVec[T, const N: USize]
  cap: USize
end
"#;
    let prog = parse(src);
    let params = first_class_generic_params(&prog, "SmallVec");
    assert_eq!(params.len(), 2);
    assert_const_param(&params[1], "N", "USize");
}

/// B1 (fn): same form on `def`.
#[test]
fn parse_const_generic_param_on_fn() {
    let src = r#"
def rotate[const K: USize](x: Int) -> Int
  x
end
"#;
    let prog = parse(src);
    let params = first_fn_generic_params(&prog, "rotate");
    assert_eq!(params.len(), 1);
    assert_const_param(&params[0], "K", "USize");
}

/// B1 multi: multiple const params in the same brackets.
#[test]
fn parse_multiple_const_generic_params_typecheck_position() {
    let src = r#"
struct Matrix[T, const M: USize, const N: USize]
  rows: USize
end
"#;
    let prog = parse(src);
    let params = first_struct_generic_params(&prog, "Matrix");
    assert_eq!(params.len(), 3, "expected 3 params, got {:?}", params);
    match &params[0] {
        GenericParam::Type { name, .. } => assert_eq!(name, "T"),
        other => panic!("expected first to be Type, got {:?}", other),
    }
    assert_const_param(&params[1], "M", "USize");
    assert_const_param(&params[2], "N", "USize");
}

/// B1 mixed: type and const params may interleave at the parser layer.
/// Canonical style (types first, consts after) is a formatter concern,
/// not a parser hard rule — the parser accepts arbitrary order.
#[test]
fn parse_mixed_type_and_const_generic_params() {
    let src = r#"
struct Buffer[const CAP: USize, T]
  len: USize
end
"#;
    let prog = parse(src);
    let params = first_struct_generic_params(&prog, "Buffer");
    assert_eq!(params.len(), 2);
    assert_const_param(&params[0], "CAP", "USize");
    match &params[1] {
        GenericParam::Type { name, .. } => assert_eq!(name, "T"),
        other => panic!("expected Type as second param, got {:?}", other),
    }
}

// ── Stage 2: use-site integer-literal generic args ──────────────────

/// Walk a `TypeExpr` down through references / arrays to the first
/// `Named` path and return a clone — the field-type AST is a
/// reference type in some fixtures, so we strip wrappers.
fn into_named<'a>(ty: &'a TypeExpr) -> &'a riven_core::parser::ast::TypePath {
    match ty {
        TypeExpr::Named(path) => path,
        other => panic!("expected Named, got {:?}", other),
    }
}

/// B2: a struct field annotated as `Vector[Int, 4]` parses with the
/// second generic argument captured as `TypeExpr::ConstLit(4, _)`.
/// Stage 2 is parser-only — resolve / typeck reject in S5 if the
/// literal lands against a type parameter; here we just assert the
/// AST shape.
#[test]
fn parse_const_lit_as_generic_arg() {
    let src = r#"
struct Holder
  v: Vector[Int, 4]
end
"#;
    let prog = parse(src);
    let mut found_struct = None;
    for item in &prog.items {
        if let TopLevelItem::Struct(s) = item {
            if s.name == "Holder" {
                found_struct = Some(s);
                break;
            }
        }
    }
    let s = found_struct.expect("no Holder struct");
    assert_eq!(s.fields.len(), 1);
    let path = into_named(&s.fields[0].type_expr);
    let args = path
        .generic_args
        .as_ref()
        .expect("Vector should have generic args");
    assert_eq!(args.len(), 2, "expected 2 generic args, got {:?}", args);
    match &args[0] {
        TypeExpr::Named(p) => assert_eq!(p.segments.last().map(|s| s.as_str()), Some("Int")),
        other => panic!("first arg expected Named(Int), got {:?}", other),
    }
    match &args[1] {
        TypeExpr::ConstLit { value, .. } => assert_eq!(*value, 4),
        other => panic!(
            "second arg expected TypeExpr::ConstLit(4, _), got {:?}",
            other
        ),
    }
}

/// B2: multiple const literals in a single arg list.
#[test]
fn parse_multiple_const_lits_as_generic_args() {
    let src = r#"
struct Holder
  m: Matrix[Float, 3, 4]
end
"#;
    let prog = parse(src);
    let s = prog
        .items
        .iter()
        .find_map(|i| match i {
            TopLevelItem::Struct(s) if s.name == "Holder" => Some(s),
            _ => None,
        })
        .expect("no Holder struct");
    let path = into_named(&s.fields[0].type_expr);
    let args = path.generic_args.as_ref().expect("Matrix generic args");
    assert_eq!(args.len(), 3);
    match (&args[1], &args[2]) {
        (TypeExpr::ConstLit { value: m, .. }, TypeExpr::ConstLit { value: n, .. }) => {
            assert_eq!(*m, 3);
            assert_eq!(*n, 4);
        }
        other => panic!("expected two ConstLits, got {:?}", other),
    }
}

// ── Stage 3: resolver registers DefKind::ConstParam ────────────────

/// B3 (S3 minimal): declaring a struct / class / enum / fn with a
/// const generic param typechecks cleanly.  Regression canary:
/// confirms the resolver registers `GenericParam::Const` without
/// emitting spurious diagnostics (treating it as an unknown
/// identifier or expecting trait bounds).
#[test]
fn const_generic_declarations_typecheck_clean() {
    let src = r#"
struct Vector[T, const N: USize]
  data: USize
end

class Matrix[T, const M: USize, const N: USize]
  rows: USize
end

def rotate[const K: USize](x: Int) -> Int
  x
end
"#;
    let mut lx = riven_core::lexer::Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = riven_core::parser::Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = riven_core::typeck::type_check(&prog);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "expected zero typeck errors for const-generic declarations; got: {:?}",
        errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// B3 (S3 deliverable): after typeck, the symbol table contains a
/// `DefKind::ConstParam` entry for every `const NAME: Type` generic
/// parameter declared in the program.  This is the load-bearing
/// claim of S3 — later stages (S4 HIR ConstExpr, S5 typeck
/// unification) look these defs up via `symbols.iter()`.
#[test]
fn const_generic_param_registered_in_symbol_table() {
    use riven_core::resolve::symbols::DefKind;

    let src = r#"
struct Vector[T, const N: USize]
  data: USize
end

class Matrix[T, const M: USize, const N: USize]
  rows: USize
end
"#;
    let mut lx = riven_core::lexer::Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = riven_core::parser::Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = riven_core::typeck::type_check(&prog);

    let const_params: Vec<&str> = result
        .symbols
        .iter()
        .filter(|d| matches!(d.kind, DefKind::ConstParam { .. }))
        .map(|d| d.name.as_str())
        .collect();

    // Expect exactly three const params: N (struct), M, N (class).
    assert_eq!(
        const_params.len(),
        3,
        "expected 3 ConstParam defs, got {:?}",
        const_params
    );
    assert!(const_params.iter().any(|n| *n == "N"), "missing `N`: {:?}", const_params);
    assert!(const_params.iter().any(|n| *n == "M"), "missing `M`: {:?}", const_params);
}

/// B2: integer-literal generic args coexist with type-args on either side.
/// `Foo[Int, 8, Bar]` — type / const / type ordering — must round-trip.
#[test]
fn parse_const_lit_with_trailing_type_arg() {
    let src = r#"
struct Holder
  x: Foo[Int, 8, Bar]
end
"#;
    let prog = parse(src);
    let s = prog
        .items
        .iter()
        .find_map(|i| match i {
            TopLevelItem::Struct(s) if s.name == "Holder" => Some(s),
            _ => None,
        })
        .expect("no Holder struct");
    let path = into_named(&s.fields[0].type_expr);
    let args = path.generic_args.as_ref().expect("Foo generic args");
    assert_eq!(args.len(), 3);
    match &args[1] {
        TypeExpr::ConstLit { value, .. } => assert_eq!(*value, 8),
        other => panic!("expected ConstLit(8), got {:?}", other),
    }
    match &args[2] {
        TypeExpr::Named(p) => assert_eq!(p.segments.last().map(|s| s.as_str()), Some("Bar")),
        other => panic!("expected Named(Bar), got {:?}", other),
    }
}
