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
