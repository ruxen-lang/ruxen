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

// ── Stage 7: ConstExpr evaluator + layout integration ───────────────

/// B7 (S7 minimal): `ConstExpr::eval` resolves `Lit` directly and
/// `Param` against a binding map.  Unresolved params return an
/// `Err`; arithmetic is deferred to S8.
#[test]
fn const_expr_eval_lit_and_param() {
    use riven_core::hir::types::ConstExpr;
    use std::collections::HashMap;

    let empty: HashMap<String, u64> = HashMap::new();
    let mut bound: HashMap<String, u64> = HashMap::new();
    bound.insert("N".to_string(), 4);

    assert_eq!(ConstExpr::Lit(7).eval(&empty), Ok(7));
    assert_eq!(ConstExpr::Lit(0).eval(&empty), Ok(0));
    assert_eq!(ConstExpr::Param("N".to_string()).eval(&bound), Ok(4));
    assert!(ConstExpr::Param("M".to_string()).eval(&bound).is_err());
    assert!(ConstExpr::Error.eval(&empty).is_err());
}

/// B7: `layout_of(Ty::Array(Int, ConstExpr::Lit(4)))` returns 32
/// bytes (4 × 8) — the array layout consults the const expression
/// rather than the pre-S4 `usize` field.
#[test]
fn array_layout_evaluates_const_expr_lit() {
    use riven_core::codegen::layout::layout_of;
    use riven_core::hir::types::{ConstExpr, Ty};
    use riven_core::resolve::symbols::SymbolTable;

    let symbols = SymbolTable::new();
    let layout = layout_of(
        &Ty::Array(Box::new(Ty::Int), ConstExpr::Lit(4)),
        &symbols,
    );
    assert_eq!(layout.size, 32);
    assert_eq!(layout.alignment, 8);
}

// ── Stage 6: distinct const args produce distinct types ─────────────

/// B6 (S6 minimal): `Vector[Int, 3]` and `Vector[Int, 4]` resolve to
/// **distinct** types.  Today (before S6) they both collapse to
/// `Ty::Class { name: "Vector", generic_args: [Ty::Int, Ty::Error] }`
/// and compare equal — a soundness gap.
#[test]
fn distinct_const_args_produce_distinct_types() {
    use riven_core::hir::types::{ConstExpr, Ty};

    // Build the type-expression shape the resolver would produce.
    let three = Ty::ConstArg(ConstExpr::Lit(3));
    let four = Ty::ConstArg(ConstExpr::Lit(4));
    let a = Ty::Class {
        name: "Vector".to_string(),
        generic_args: vec![Ty::Int, three.clone()],
    };
    let b = Ty::Class {
        name: "Vector".to_string(),
        generic_args: vec![Ty::Int, four.clone()],
    };
    let c = Ty::Class {
        name: "Vector".to_string(),
        generic_args: vec![Ty::Int, three.clone()],
    };

    assert_ne!(a, b, "Vector[Int, 3] must differ from Vector[Int, 4]");
    assert_eq!(a, c, "two Vector[Int, 3] values must compare equal");
}

/// B6: in a real source program, two fn signatures returning
/// different `Vector[Int, N]` types end up with different resolved
/// `Ty::Class` arg lists.  The HIR of two distinct-arg fns must
/// contain distinct return-type values.
#[test]
fn const_args_thread_into_class_generic_args() {
    use riven_core::hir::types::{ConstExpr, Ty};

    let src = r#"
class Vector[T, const N: USize]
  data: USize

  def init(@data: USize)
  end
end

def make_three -> Vector[Int, 3]
  Vector.new(0)
end

def make_four -> Vector[Int, 4]
  Vector.new(0)
end
"#;
    let mut lx = riven_core::lexer::Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = riven_core::parser::Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = riven_core::typeck::type_check(&prog);

    let return_tys: Vec<Ty> = result
        .program
        .items
        .iter()
        .filter_map(|item| match item {
            riven_core::hir::nodes::HirItem::Function(f) => Some(f.return_ty.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(return_tys.len(), 2, "expected two fns, got: {:?}", return_tys);
    assert_ne!(
        return_tys[0], return_tys[1],
        "make_three and make_four must return distinct types: {:?} vs {:?}",
        return_tys[0], return_tys[1]
    );
    // And specifically the const args differ.
    let arg_three = match &return_tys[0] {
        Ty::Class { generic_args, .. } => generic_args.get(1).cloned(),
        _ => None,
    };
    assert_eq!(
        arg_three,
        Some(Ty::ConstArg(ConstExpr::Lit(3))),
        "make_three should carry ConstArg(Lit(3)) at slot 1; got {:?}",
        return_tys[0]
    );
}

// ── Stage 5: typeck unification + E0704 kind mismatch ───────────────
//
// (Historical note: the kind-mismatch slot was E0700 from S5 ship
// through 2026-05-14; the typeck iterator-`sum` validator was
// already squatting on E0700, so the spec was amended and this
// code moved to E0704.)

/// B5 (S5 minimal): passing an integer literal where the declared
/// param is a type → E0704 kind mismatch.
#[test]
fn const_lit_against_type_param_emits_e0704() {
    let src = r#"
class OnlyType[T]
  data: USize

  def init(@data: USize)
  end
end

def main
  let _x: OnlyType[4] = OnlyType.new(0)
end
"#;
    let mut lx = riven_core::lexer::Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = riven_core::parser::Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = riven_core::typeck::type_check(&prog);
    let codes: Vec<&str> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .filter_map(|d| d.code.as_deref())
        .collect();
    assert!(
        codes.contains(&"E0704"),
        "expected E0704 for ConstLit on Type param; got codes: {:?}, all diagnostics: {:?}",
        codes,
        result.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// B5 (S5 minimal): passing a literal where the param is declared
/// `const` and types match → no diagnostic.
#[test]
fn const_lit_against_const_param_typechecks_clean() {
    let src = r#"
class Wrap[const N: USize]
  data: USize

  def init(@data: USize)
  end
end

def use_wrap -> Wrap[7]
  Wrap.new(0)
end
"#;
    let mut lx = riven_core::lexer::Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = riven_core::parser::Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = riven_core::typeck::type_check(&prog);
    let kind_mismatch_diags: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .filter(|d| d.code.as_deref() == Some("E0704"))
        .collect();
    assert!(
        kind_mismatch_diags.is_empty(),
        "ConstLit on a Const param must not emit E0704; got: {:?}",
        kind_mismatch_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ── Stage 4: HIR ConstExpr + Ty::Array carries it ───────────────────

/// B4 (S4 minimal): `Ty::Array` now carries a `ConstExpr` rather
/// than a bare `usize`.  Constructing an array type from a literal
/// integer wraps it as `ConstExpr::Lit(n)`.
#[test]
fn ty_array_carries_const_expr_lit() {
    use riven_core::hir::types::{ConstExpr, Ty};

    let ty = Ty::Array(Box::new(Ty::Int), ConstExpr::Lit(4));
    match &ty {
        Ty::Array(elem, size) => {
            assert!(matches!(**elem, Ty::Int));
            assert!(matches!(size, ConstExpr::Lit(4)));
        }
        other => panic!("expected Ty::Array, got {:?}", other),
    }
    // Display fmt should still print `[Int; 4]`.
    assert_eq!(format!("{}", ty), "[Int; 4]");
}

/// B4: `ConstExpr::Param(name)` represents an unresolved const-param
/// reference.  Two `Ty::Array`s with the same param name compare
/// equal; with different names they don't.
#[test]
fn const_expr_param_equality() {
    use riven_core::hir::types::{ConstExpr, Ty};

    let a = Ty::Array(Box::new(Ty::Int), ConstExpr::Param("N".to_string()));
    let b = Ty::Array(Box::new(Ty::Int), ConstExpr::Param("N".to_string()));
    let c = Ty::Array(Box::new(Ty::Int), ConstExpr::Param("M".to_string()));
    assert_eq!(a, b);
    assert_ne!(a, c);
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

// ── Stage 8: arithmetic in const exprs ──────────────────────────────
//
// B8 (S8.S1): `ConstExpr::Op` evaluator.  Recurses on both sides,
// applies checked `u64` arithmetic, and propagates Unresolved /
// Malformed errors from inner sub-trees.  Overflow on `+ - *` and
// `_ / 0` surface as `ConstEvalError::Overflow` and
// `ConstEvalError::DivisionByZero` respectively.  Parser-side
// arithmetic (`Vector[Int, M + N]`, `[T; A * B]`) lands in
// later S8 substages.

#[test]
fn const_expr_eval_op_basic_arithmetic() {
    use riven_core::hir::types::{ConstExpr, ConstOp};
    use std::collections::HashMap;

    let empty: HashMap<String, u64> = HashMap::new();
    let mk = |a: u64, op: ConstOp, b: u64| {
        ConstExpr::Op(
            Box::new(ConstExpr::Lit(a)),
            op,
            Box::new(ConstExpr::Lit(b)),
        )
    };

    assert_eq!(mk(2, ConstOp::Add, 3).eval(&empty), Ok(5));
    assert_eq!(mk(7, ConstOp::Sub, 3).eval(&empty), Ok(4));
    assert_eq!(mk(4, ConstOp::Mul, 5).eval(&empty), Ok(20));
    assert_eq!(mk(10, ConstOp::Div, 2).eval(&empty), Ok(5));
}

#[test]
fn const_expr_eval_op_with_param() {
    use riven_core::hir::types::{ConstExpr, ConstOp};
    use std::collections::HashMap;

    let mut bound: HashMap<String, u64> = HashMap::new();
    bound.insert("N".to_string(), 4);

    // N + 1 = 5
    let expr = ConstExpr::Op(
        Box::new(ConstExpr::Param("N".to_string())),
        ConstOp::Add,
        Box::new(ConstExpr::Lit(1)),
    );
    assert_eq!(expr.eval(&bound), Ok(5));
}

#[test]
fn const_expr_eval_op_nested_precedence_preserved() {
    use riven_core::hir::types::{ConstExpr, ConstOp};
    use std::collections::HashMap;

    let empty: HashMap<String, u64> = HashMap::new();
    // (2 + 3) * 4 = 20 — the parser is responsible for grouping; the
    // evaluator just walks the tree it gets.
    let expr = ConstExpr::Op(
        Box::new(ConstExpr::Op(
            Box::new(ConstExpr::Lit(2)),
            ConstOp::Add,
            Box::new(ConstExpr::Lit(3)),
        )),
        ConstOp::Mul,
        Box::new(ConstExpr::Lit(4)),
    );
    assert_eq!(expr.eval(&empty), Ok(20));

    // 2 + 3 * 4 = 14 (right side grouped)
    let expr = ConstExpr::Op(
        Box::new(ConstExpr::Lit(2)),
        ConstOp::Add,
        Box::new(ConstExpr::Op(
            Box::new(ConstExpr::Lit(3)),
            ConstOp::Mul,
            Box::new(ConstExpr::Lit(4)),
        )),
    );
    assert_eq!(expr.eval(&empty), Ok(14));
}

#[test]
fn const_expr_eval_op_division_by_zero() {
    use riven_core::hir::types::{ConstEvalError, ConstExpr, ConstOp};
    use std::collections::HashMap;

    let empty: HashMap<String, u64> = HashMap::new();
    let expr = ConstExpr::Op(
        Box::new(ConstExpr::Lit(7)),
        ConstOp::Div,
        Box::new(ConstExpr::Lit(0)),
    );
    assert_eq!(expr.eval(&empty), Err(ConstEvalError::DivisionByZero));

    // 0 / 0 also surfaces as DivisionByZero, not "indeterminate".
    let expr = ConstExpr::Op(
        Box::new(ConstExpr::Lit(0)),
        ConstOp::Div,
        Box::new(ConstExpr::Lit(0)),
    );
    assert_eq!(expr.eval(&empty), Err(ConstEvalError::DivisionByZero));
}

#[test]
fn const_expr_eval_op_overflow() {
    use riven_core::hir::types::{ConstEvalError, ConstExpr, ConstOp};
    use std::collections::HashMap;

    let empty: HashMap<String, u64> = HashMap::new();

    let add_overflow = ConstExpr::Op(
        Box::new(ConstExpr::Lit(u64::MAX)),
        ConstOp::Add,
        Box::new(ConstExpr::Lit(1)),
    );
    assert_eq!(add_overflow.eval(&empty), Err(ConstEvalError::Overflow));

    // u64 borrow: 0 - 1 surfaces as Overflow per spec (single E-CONST-OVERFLOW slot).
    let sub_underflow = ConstExpr::Op(
        Box::new(ConstExpr::Lit(0)),
        ConstOp::Sub,
        Box::new(ConstExpr::Lit(1)),
    );
    assert_eq!(sub_underflow.eval(&empty), Err(ConstEvalError::Overflow));

    let mul_overflow = ConstExpr::Op(
        Box::new(ConstExpr::Lit(u64::MAX)),
        ConstOp::Mul,
        Box::new(ConstExpr::Lit(2)),
    );
    assert_eq!(mul_overflow.eval(&empty), Err(ConstEvalError::Overflow));
}

#[test]
fn const_expr_eval_op_propagates_inner_errors() {
    use riven_core::hir::types::{ConstEvalError, ConstExpr, ConstOp};
    use std::collections::HashMap;

    let empty: HashMap<String, u64> = HashMap::new();

    // Unbound param on the right surfaces through Op.
    let unresolved = ConstExpr::Op(
        Box::new(ConstExpr::Lit(1)),
        ConstOp::Add,
        Box::new(ConstExpr::Param("M".to_string())),
    );
    assert_eq!(
        unresolved.eval(&empty),
        Err(ConstEvalError::Unresolved("M".to_string()))
    );

    // Parser-recovery `Error` on either side surfaces as Malformed.
    let malformed = ConstExpr::Op(
        Box::new(ConstExpr::Error),
        ConstOp::Mul,
        Box::new(ConstExpr::Lit(2)),
    );
    assert_eq!(malformed.eval(&empty), Err(ConstEvalError::Malformed));
}

#[test]
fn array_layout_evaluates_const_expr_arithmetic() {
    // [Int; 2 + 2] = 4 elements * 8 bytes = 32-byte layout.
    use riven_core::codegen::layout::layout_of;
    use riven_core::hir::types::{ConstExpr, ConstOp, Ty};
    use riven_core::resolve::symbols::SymbolTable;

    let symbols = SymbolTable::new();
    let arith = ConstExpr::Op(
        Box::new(ConstExpr::Lit(2)),
        ConstOp::Add,
        Box::new(ConstExpr::Lit(2)),
    );
    let layout = layout_of(&Ty::Array(Box::new(Ty::Int), arith), &symbols);
    assert_eq!(layout.size, 32);
    assert_eq!(layout.alignment, 8);
}

// ── Stage 8.S2: source-level arithmetic in `[T; expr]` array sizes ──
//
// The parser already accepts arbitrary expressions inside the
// array-size slot (it calls `parse_expression`); S8.S2 wires resolve
// to fold `+ - * /` into `ConstExpr::Op` trees rather than collapsing
// them to `ConstExpr::Error`.  These tests exercise the full
// parse-→-resolve pipeline so a regression on either side is caught.

fn resolve_first_struct_field_ty(
    src: &str,
    struct_name: &str,
) -> riven_core::hir::types::Ty {
    use riven_core::hir::nodes::HirItem;
    let mut lx = riven_core::lexer::Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = riven_core::parser::Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = riven_core::typeck::type_check(&prog);
    for item in &result.program.items {
        if let HirItem::Struct(s) = item {
            if s.name == struct_name {
                return s.fields[0].ty.clone();
            }
        }
    }
    panic!("no struct {} in source", struct_name);
}

#[test]
fn resolve_array_size_lowers_binary_add_to_const_op() {
    // Pure literal arithmetic is constant-folded by the S8.S4
    // normal-form rewriter, so `[Int; 2 + 3]` produces `Lit(5)`.
    // The pre-S8.S4 shape (`Op(Lit(2), Add, Lit(3))`) is no longer
    // observable through the resolver — see the S8.S4 isolated-
    // tree-shape pin for the unfolded form.
    use riven_core::hir::types::{ConstExpr, Ty};

    let src = r#"
struct Buf
  data: [Int; 2 + 3]
end
"#;
    let ty = resolve_first_struct_field_ty(src, "Buf");
    match ty {
        Ty::Array(elem, size) => {
            assert!(matches!(*elem, Ty::Int));
            assert_eq!(size, ConstExpr::Lit(5));
        }
        other => panic!("expected Ty::Array, got {:?}", other),
    }
}

#[test]
fn resolve_array_size_lowers_all_four_operators() {
    // Each pure-literal arithmetic case constant-folds to a single
    // `Lit` via the S8.S4 rewriter; we pin the eval result through
    // both the folded form (`ConstExpr::Lit(n)`) and the explicit
    // eval call so any future representation change is caught.
    use riven_core::hir::types::{ConstExpr, Ty};
    let cases = [
        ("2 + 3", 5u64),
        ("7 - 3", 4),
        ("4 * 5", 20),
        ("12 / 3", 4),
    ];
    for (expr_text, expected_eval) in cases {
        let src = format!(
            "struct Buf\n  data: [Int; {}]\nend\n",
            expr_text
        );
        let ty = resolve_first_struct_field_ty(&src, "Buf");
        match ty {
            Ty::Array(_, size) => {
                assert_eq!(
                    size, ConstExpr::Lit(expected_eval),
                    "wrong folded value for `{}`", expr_text
                );
                let bindings = std::collections::HashMap::new();
                assert_eq!(
                    size.eval(&bindings),
                    Ok(expected_eval),
                    "wrong eval for {}", expr_text
                );
            }
            other => panic!("expected Ty::Array for `{}`, got {:?}", expr_text, other),
        }
    }
}

#[test]
fn resolve_array_size_lowers_nested_arithmetic() {
    // Pure-literal nested arithmetic constant-folds end-to-end.
    // The user-written grouping `(2 + 3) * 4` evaluates to 20;
    // without parens `2 + 3 * 4` evaluates to 14 (precedence: `*`
    // binds tighter than `+`).  Both fold to a single `Lit`.
    use riven_core::hir::types::{ConstExpr, Ty};

    let src = r#"
struct WithParens
  data: [Int; (2 + 3) * 4]
end

struct NoParens
  data: [Int; 2 + 3 * 4]
end
"#;
    let with_parens = resolve_first_struct_field_ty(src, "WithParens");
    let no_parens = resolve_first_struct_field_ty(src, "NoParens");

    match with_parens {
        Ty::Array(_, size) => {
            assert_eq!(size, ConstExpr::Lit(20));
        }
        other => panic!("expected Ty::Array, got {:?}", other),
    }
    match no_parens {
        Ty::Array(_, size) => {
            assert_eq!(size, ConstExpr::Lit(14));
        }
        other => panic!("expected Ty::Array, got {:?}", other),
    }
}

#[test]
fn resolve_array_size_lowers_param_reference_in_arithmetic() {
    // `[Int; N + 1]` where N is an in-scope const param.  The S3
    // resolver already lowers a bare `Identifier(name)` to
    // `ConstExpr::Param(name)`; S8.S2 just preserves that under the
    // arithmetic recursion.
    use riven_core::hir::types::{ConstExpr, ConstOp, Ty};

    let src = r#"
struct Vector[T, const N: USize]
  data: [T; N + 1]
end
"#;
    let ty = resolve_first_struct_field_ty(src, "Vector");
    match ty {
        Ty::Array(_, size) => match size {
            ConstExpr::Op(a, ConstOp::Add, b) => {
                assert_eq!(*a, ConstExpr::Param("N".to_string()));
                assert_eq!(*b, ConstExpr::Lit(1));
            }
            other => panic!("expected Op(Param(N), Add, Lit(1)), got {:?}", other),
        },
        other => panic!("expected Ty::Array, got {:?}", other),
    }
}

// ── Stage 8.S3: arithmetic in const-arg position ────────────────────
//
// B8 (S8.S3): the parser also accepts `+ - * /` arithmetic in
// const-arg position (`Vector[Int, 2 + 3]`).  Triggered by an
// IntLiteral followed by an arithmetic op; the whole expression
// parses through `parse_expression` and emits
// `TypeExpr::ConstExprArg`, which resolve folds through the same
// `lower_const_expr_from_expr` helper used for array-size form.
// Bare literals (`Vector[Int, 4]`) continue to emit `ConstLit`.

#[test]
fn parse_const_arg_arithmetic_emits_const_expr_arg() {
    use riven_core::parser::ast::{Expr, ExprKind, BinOp, TopLevelItem, TypeExpr};
    let src = r#"
struct Holder
  x: Foo[Int, 2 + 3]
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
    assert_eq!(args.len(), 2);
    match &args[1] {
        TypeExpr::ConstExprArg { expr, .. } => {
            // The captured `Expr` should be a `BinaryOp(2, Add, 3)`.
            match &expr.as_ref().kind {
                ExprKind::BinaryOp { left, op, right } => {
                    assert_eq!(*op, BinOp::Add);
                    assert!(matches!(left.as_ref().kind, ExprKind::IntLiteral(2, _)));
                    assert!(matches!(right.as_ref().kind, ExprKind::IntLiteral(3, _)));
                    // Suppress unused-binding warnings if the const
                    // arms get refactored.
                    let _ = (left, right);
                    let _: &Expr = left.as_ref();
                }
                other => panic!("expected BinaryOp, got {:?}", other),
            }
        }
        other => panic!("expected ConstExprArg, got {:?}", other),
    }
}

#[test]
fn parse_const_arg_bare_literal_still_emits_const_lit() {
    // Backwards-compat: no arithmetic follow-up means the historic
    // `ConstLit` fast path is still used.
    use riven_core::parser::ast::{TopLevelItem, TypeExpr};
    let src = r#"
struct Holder
  x: Foo[Int, 4]
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
    assert_eq!(args.len(), 2);
    assert!(
        matches!(&args[1], TypeExpr::ConstLit { value: 4, .. }),
        "expected ConstLit(4), got {:?}",
        &args[1]
    );
}

#[test]
fn resolve_const_arg_arithmetic_lowers_to_const_expr_op() {
    // End-to-end: parse + resolve of `Vector[Int, 2 + 3]` in a
    // parameter-type annotation threads through to
    // `Ty::ConstArg(ConstExpr::Op(Lit(2), Add, Lit(3)))`.  The
    // kind-check accepts it (it's a const-kind arg landing in a
    // const-kind slot).  We pin the resolved param type directly
    // because constructor return-type inference doesn't fold the
    // const arg back in — that's a separate (S6 follow-up)
    // monomorphization concern.
    use riven_core::hir::nodes::HirItem;
    use riven_core::hir::types::{ConstExpr, ConstOp, Ty};

    let src = r#"
class Vector[T, const N: USize]
  data: USize

  def init(@data: USize)
  end
end

def take_vec(v: Vector[Int, 2 + 3])
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
        "expected no errors; got: {:?}",
        errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // The first parameter of `consume` should resolve to
    // `Vector` with generic_args = [Int, ConstArg(Op(Lit(2), Add, Lit(3)))].
    let func = result
        .program
        .items
        .iter()
        .find_map(|i| match i {
            HirItem::Function(f) if f.name == "take_vec" => Some(f),
            _ => None,
        })
        .expect("no take_vec function in HIR");
    let param_ty = &func.params[0].ty;
    let inner = match param_ty {
        // Walk through any Ref wrapper that comes from `&` or method
        // synthesis; the spec only commits to the unwrapped shape.
        Ty::Ref(inner) | Ty::RefMut(inner) => inner.as_ref(),
        Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => inner.as_ref(),
        other => other,
    };
    match inner {
        Ty::Class { name, generic_args, .. } => {
            assert_eq!(name, "Vector");
            assert_eq!(generic_args.len(), 2);
            // Pure-literal `2 + 3` constant-folds to `Lit(5)` via
            // the S8.S4 normal-form rewriter.  Param-mixed
            // arithmetic (`N + 1`) keeps the Op shape — see the
            // sibling array-size tests for that case.
            match &generic_args[1] {
                Ty::ConstArg(ce) => assert_eq!(*ce, ConstExpr::Lit(5)),
                other => panic!("expected ConstArg(Lit(5)), got {:?}", other),
            }
        }
        other => panic!("expected Ty::Class(Vector), got {:?}", other),
    }
    // Suppress unused-import warning if the const-arg path stops
    // using these tags.
    let _ = (ConstExpr::Lit(0), ConstOp::Add);
}

// ── Stage 8.S4: normal-form rewriter ────────────────────────────────
//
// B8 (S8.S4): identity-removal rewrites canonicalise `ConstExpr`
// trees so that `[T; N + 0]` and `[T; N]` produce the same
// `Ty::Array`.  Constant folding collapses `Lit(a) ⊙ Lit(b)` to a
// single `Lit(c)` when eval succeeds.  Spec §B8 documents the
// distributive / commutative cases the rewriter intentionally
// leaves as distinct forms in v1.

#[test]
fn const_expr_normal_form_identity_rewrites() {
    use riven_core::hir::types::{ConstExpr, ConstOp};

    let n = || ConstExpr::Param("N".to_string());

    // N + 0 = N (and 0 + N = N)
    assert_eq!(
        ConstExpr::Op(Box::new(n()), ConstOp::Add, Box::new(ConstExpr::Lit(0)))
            .normal_form(),
        n()
    );
    assert_eq!(
        ConstExpr::Op(Box::new(ConstExpr::Lit(0)), ConstOp::Add, Box::new(n()))
            .normal_form(),
        n()
    );

    // N - 0 = N (but 0 - N is left as-is: u64 has no negatives)
    assert_eq!(
        ConstExpr::Op(Box::new(n()), ConstOp::Sub, Box::new(ConstExpr::Lit(0)))
            .normal_form(),
        n()
    );
    let zero_minus_n =
        ConstExpr::Op(Box::new(ConstExpr::Lit(0)), ConstOp::Sub, Box::new(n()));
    assert_eq!(zero_minus_n.clone().normal_form(), zero_minus_n);

    // N * 1 = N, 1 * N = N
    assert_eq!(
        ConstExpr::Op(Box::new(n()), ConstOp::Mul, Box::new(ConstExpr::Lit(1)))
            .normal_form(),
        n()
    );
    assert_eq!(
        ConstExpr::Op(Box::new(ConstExpr::Lit(1)), ConstOp::Mul, Box::new(n()))
            .normal_form(),
        n()
    );

    // N * 0 = 0, 0 * N = 0
    assert_eq!(
        ConstExpr::Op(Box::new(n()), ConstOp::Mul, Box::new(ConstExpr::Lit(0)))
            .normal_form(),
        ConstExpr::Lit(0)
    );
    assert_eq!(
        ConstExpr::Op(Box::new(ConstExpr::Lit(0)), ConstOp::Mul, Box::new(n()))
            .normal_form(),
        ConstExpr::Lit(0)
    );

    // N / 1 = N (but 1 / N is left as-is)
    assert_eq!(
        ConstExpr::Op(Box::new(n()), ConstOp::Div, Box::new(ConstExpr::Lit(1)))
            .normal_form(),
        n()
    );
    let one_div_n =
        ConstExpr::Op(Box::new(ConstExpr::Lit(1)), ConstOp::Div, Box::new(n()));
    assert_eq!(one_div_n.clone().normal_form(), one_div_n);
}

#[test]
fn const_expr_normal_form_folds_pure_arithmetic() {
    // `Lit ⊙ Lit` collapses to a single Lit when eval succeeds.
    use riven_core::hir::types::{ConstExpr, ConstOp};

    let two_plus_three = ConstExpr::Op(
        Box::new(ConstExpr::Lit(2)),
        ConstOp::Add,
        Box::new(ConstExpr::Lit(3)),
    );
    assert_eq!(two_plus_three.normal_form(), ConstExpr::Lit(5));

    // Constant-fold nested: (2 + 3) * 4 → Lit(20).
    let nested = ConstExpr::Op(
        Box::new(ConstExpr::Op(
            Box::new(ConstExpr::Lit(2)),
            ConstOp::Add,
            Box::new(ConstExpr::Lit(3)),
        )),
        ConstOp::Mul,
        Box::new(ConstExpr::Lit(4)),
    );
    assert_eq!(nested.normal_form(), ConstExpr::Lit(20));
}

#[test]
fn const_expr_normal_form_preserves_op_on_overflow() {
    // Overflow / div-zero in `Lit ⊙ Lit` leaves the Op shape so a
    // later E0703 surfacing pass still has both spans.
    use riven_core::hir::types::{ConstExpr, ConstOp};

    let overflow = ConstExpr::Op(
        Box::new(ConstExpr::Lit(u64::MAX)),
        ConstOp::Add,
        Box::new(ConstExpr::Lit(1)),
    );
    // Should NOT collapse — the Op is preserved.
    assert!(matches!(overflow.clone().normal_form(), ConstExpr::Op(..)));

    let div_zero = ConstExpr::Op(
        Box::new(ConstExpr::Lit(7)),
        ConstOp::Div,
        Box::new(ConstExpr::Lit(0)),
    );
    assert!(matches!(div_zero.normal_form(), ConstExpr::Op(..)));
}

#[test]
fn const_expr_normal_form_recurses_into_children() {
    // `(N + 0) * 1 = N` — identities apply through nested Op.
    use riven_core::hir::types::{ConstExpr, ConstOp};

    let n = || ConstExpr::Param("N".to_string());
    let inner_add = ConstExpr::Op(Box::new(n()), ConstOp::Add, Box::new(ConstExpr::Lit(0)));
    let outer = ConstExpr::Op(Box::new(inner_add), ConstOp::Mul, Box::new(ConstExpr::Lit(1)));
    assert_eq!(outer.normal_form(), n());
}

#[test]
fn resolve_normalises_array_size_n_plus_zero_equals_n() {
    // End-to-end: `[T; N + 0]` and `[T; N]` produce equal Ty values
    // through the resolver, so any downstream code that compares
    // them with PartialEq (e.g. unification, the kind-check)
    // treats them as the same type.
    use riven_core::hir::nodes::HirItem;

    let src = r#"
struct Buf[T, const N: USize]
  data: [T; N + 0]
end

struct BufBare[T, const N: USize]
  data: [T; N]
end
"#;
    let mut lx = riven_core::lexer::Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = riven_core::parser::Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = riven_core::typeck::type_check(&prog);
    let find = |name: &str| {
        result
            .program
            .items
            .iter()
            .find_map(|i| match i {
                HirItem::Struct(s) if s.name == name => Some(s.fields[0].ty.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no struct {} in HIR", name))
    };
    let with_zero = find("Buf");
    let bare = find("BufBare");
    assert_eq!(with_zero, bare, "[T; N + 0] should normalise to [T; N]");
}

#[test]
fn resolve_normalises_const_arg_arithmetic_with_one_factor() {
    // `Vector[Int, N * 1]` and `Vector[Int, N]` produce equal Ty
    // through the parameter annotation path.
    use riven_core::hir::nodes::HirItem;
    use riven_core::hir::types::Ty;

    let src = r#"
class Vector[T, const N: USize]
  data: USize

  def init(@data: USize)
  end
end

def take_one(v: Vector[Int, 4 * 1])
end

def take_two(v: Vector[Int, 4])
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
        "expected no errors; got: {:?}",
        errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    let param_ty = |name: &str| {
        result
            .program
            .items
            .iter()
            .find_map(|i| match i {
                HirItem::Function(f) if f.name == name => Some(f.params[0].ty.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no fn {} in HIR", name))
    };
    let lhs = param_ty("take_one");
    let rhs = param_ty("take_two");
    let unwrap_class = |t: Ty| match t {
        Ty::Ref(inner) | Ty::RefMut(inner) => *inner,
        Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => *inner,
        other => other,
    };
    assert_eq!(unwrap_class(lhs), unwrap_class(rhs), "4 * 1 must normalise to 4");
}

// ── Stage 1 follow-up: E0705 const-param bad type ───────────────────
//
// Spec §B8 (E-CONST-BAD-TYPE / E0705): a `const N: TY` parameter's
// declared type must be an integer family or `Bool`.  Anything else
// (Float, String, user class, Vec, …) is rejected at resolve time.
// `Ty::Error` placeholders are intentionally allowed so the
// diagnostic doesn't stack on top of an upstream "unknown type"
// error.

#[test]
fn const_param_float_type_emits_e0705() {
    let src = r#"
struct Buf[T, const N: Float]
  data: [T; 4]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "E0705"),
        "expected E0705 for `const N: Float`; got: {:?}",
        codes
    );
}

#[test]
fn const_param_string_type_emits_e0705() {
    let src = r#"
struct Bag[T, const N: String]
  data: [T; 4]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "E0705"),
        "expected E0705 for `const N: String`; got: {:?}",
        codes
    );
}

#[test]
fn const_param_user_class_type_emits_e0705() {
    let src = r#"
class Wrapper
  x: Int
end

struct Bag[T, const N: Wrapper]
  data: [T; 4]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "E0705"),
        "expected E0705 for `const N: Wrapper`; got: {:?}",
        codes
    );
}

#[test]
fn const_param_integer_types_do_not_emit_e0705() {
    // Pin every integer family + Bool as accepted.  USize is the
    // canonical choice but the others must also stay accepted.
    let cases = ["Int", "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16",
                 "UInt32", "UInt64", "USize", "ISize", "Bool"];
    for ty in cases {
        let src = format!(
            "struct Buf[T, const N: {}]\n  data: [T; 4]\nend\n",
            ty
        );
        let codes = typecheck_diag_codes(&src);
        assert!(
            !codes.iter().any(|c| c == "E0705"),
            "did not expect E0705 for `const N: {}`; got: {:?}",
            ty,
            codes
        );
    }
}

#[test]
fn const_param_bad_type_does_not_stack_on_unresolved_type() {
    // `Bogus` doesn't resolve, so the type expression yields
    // `Ty::Error`.  E0705 must NOT fire on top of that — the user
    // already gets an "unknown type" diagnostic.
    let src = r#"
struct Bag[T, const N: Bogus]
  data: [T; 4]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        !codes.iter().any(|c| c == "E0705"),
        "did not expect E0705 stacked on Ty::Error; got: {:?}",
        codes
    );
}

// ── Stage 8.S4 follow-up: E0703 overflow / div-zero surfacing ───────
//
// After the S8.S4 normal-form pass, pure-literal sub-trees whose
// eval failed (overflow / division-by-zero) survive as `Op(Lit, _,
// Lit)` rather than folding to a single `Lit`.  The resolver runs
// `check_const_expr_eval_errors` on the normalised result; that
// helper calls `eval(empty)` and maps the evaluator's `Overflow` /
// `DivisionByZero` variants to E0703 against the source span.
// Param-bearing trees are deferred (`Err(Unresolved)` is silenced)
// because their overflow status depends on the instantiation.

fn typecheck_diag_codes(src: &str) -> Vec<String> {
    let mut lx = riven_core::lexer::Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = riven_core::parser::Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = riven_core::typeck::type_check(&prog);
    result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .filter_map(|d| d.code.clone())
        .collect()
}

#[test]
fn array_size_overflow_emits_e0703() {
    // i64::MAX (9_223_372_036_854_775_807) — the largest literal
    // the lexer accepts; `* 4` produces 36_893_488_147_419_103_228
    // which exceeds u64::MAX (18_446_744_073_709_551_615), so
    // the checked u64 multiplication in `eval` returns
    // `Err(Overflow)` and the resolver surfaces E0703.
    let src = r#"
struct Buf
  data: [Int; 9223372036854775807 * 4]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "E0703"),
        "expected E0703 for array-size overflow; got: {:?}",
        codes
    );
}

#[test]
fn array_size_division_by_zero_emits_e0703() {
    let src = r#"
struct Bad
  data: [Int; 10 / 0]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "E0703"),
        "expected E0703 for array-size div-zero; got: {:?}",
        codes
    );
}

#[test]
fn const_arg_position_overflow_emits_e0703() {
    let src = r#"
class Vector[T, const N: USize]
  data: USize
  def init(@data: USize)
  end
end

def take_vec(v: Vector[Int, 9223372036854775807 * 4])
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "E0703"),
        "expected E0703 for const-arg overflow; got: {:?}",
        codes
    );
}

#[test]
fn array_size_param_arithmetic_does_not_emit_e0703() {
    // Param-bearing trees (`N + 1`) defer the overflow check to
    // monomorphization; resolve must NOT emit E0703 here even
    // though some instantiations of N would overflow.
    let src = r#"
struct Buf[T, const N: USize]
  data: [T; N + 1]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        !codes.iter().any(|c| c == "E0703"),
        "did not expect E0703 for param-bearing arithmetic; got: {:?}",
        codes
    );
}

#[test]
fn array_size_bare_literal_does_not_emit_e0703() {
    // No-arithmetic baseline — the `Lit(4)` form has nothing to
    // evaluate.  Pin the absence so a future regression of the
    // helper doesn't start spamming E0703 on every array type.
    let src = r#"
struct Buf
  data: [Int; 4]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        !codes.iter().any(|c| c == "E0703"),
        "did not expect E0703 for bare literal; got: {:?}",
        codes
    );
}

#[test]
fn array_size_clean_arithmetic_does_not_emit_e0703() {
    // Non-overflowing literal arithmetic constant-folds to `Lit(5)`
    // pre-eval-check; the helper sees a `Lit`, `eval` returns
    // `Ok(5)`, no diagnostic.
    let src = r#"
struct Buf
  data: [Int; 2 + 3]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        !codes.iter().any(|c| c == "E0703"),
        "did not expect E0703 for non-overflowing arithmetic; got: {:?}",
        codes
    );
}

#[test]
fn const_arg_arithmetic_against_type_param_emits_e0704() {
    // Kind-check: arithmetic in a Type slot is still a kind
    // mismatch — same diagnostic the bare `ConstLit` version
    // produces.
    let src = r#"
class OnlyType[T]
  data: USize

  def init(@data: USize)
  end
end

def main
  let _x: OnlyType[2 + 3] = OnlyType.new(0)
end
"#;
    let mut lx = riven_core::lexer::Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = riven_core::parser::Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = riven_core::typeck::type_check(&prog);
    let codes: Vec<&str> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .filter_map(|d| d.code.as_deref())
        .collect();
    assert!(
        codes.contains(&"E0704"),
        "expected E0704 for arithmetic ConstExprArg on Type param; got: {:?}",
        codes
    );
}

#[test]
fn resolve_array_size_unsupported_op_becomes_error() {
    // `%` is not in the S8 arithmetic set (spec §B8 lists only
    // `+ - * /`).  The resolver folds it into `ConstExpr::Error`
    // rather than silently treating it as another op, so the
    // downstream eval path surfaces a Malformed error rather than
    // a wrong value.  The non-const-expr check (E0702 wiring)
    // additionally surfaces a user-facing diagnostic at the source
    // span; the Ty shape pinned here is unchanged.
    use riven_core::hir::types::{ConstEvalError, ConstExpr, Ty};

    let src = r#"
struct Buf
  data: [Int; 5 % 2]
end
"#;
    let ty = resolve_first_struct_field_ty(src, "Buf");
    match ty {
        Ty::Array(_, size) => {
            assert_eq!(size, ConstExpr::Error);
            let bindings = std::collections::HashMap::new();
            assert_eq!(size.eval(&bindings), Err(ConstEvalError::Malformed));
        }
        other => panic!("expected Ty::Array, got {:?}", other),
    }
}

// ── Stage 8 follow-up: E0702 non-const-expr surfacing ───────────────
//
// The lowerer produces `ConstExpr::Error` for AST shapes outside the
// v1 const language; resolve now surfaces those as **E0702** at the
// construction-site span.  One diagnostic per site — nested noise
// stays quiet.

#[test]
fn array_size_unsupported_op_emits_e0702() {
    let src = r#"
struct Buf
  data: [Int; 5 % 2]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "E0702"),
        "expected E0702 for `5 % 2` in array size; got: {:?}",
        codes
    );
}

#[test]
fn array_size_comparison_op_emits_e0702() {
    // `<` is also outside the v1 const language.
    let src = r#"
struct Buf
  data: [Int; 3 < 4]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        codes.iter().any(|c| c == "E0702"),
        "expected E0702 for comparison in array size; got: {:?}",
        codes
    );
}

#[test]
fn array_size_clean_arithmetic_does_not_emit_e0702() {
    // Pin the negative case so a future regression doesn't start
    // emitting E0702 on every supported `+ - * /` site.
    let src = r#"
struct Buf
  data: [Int; 2 + 3]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        !codes.iter().any(|c| c == "E0702"),
        "did not expect E0702 for clean arithmetic; got: {:?}",
        codes
    );
}

#[test]
fn array_size_param_reference_does_not_emit_e0702() {
    // `N + 1` against an in-scope `const N: USize` is a perfectly
    // valid v1 const expression — no E0702.
    let src = r#"
struct Buf[T, const N: USize]
  data: [T; N + 1]
end
"#;
    let codes = typecheck_diag_codes(src);
    assert!(
        !codes.iter().any(|c| c == "E0702"),
        "did not expect E0702 for param-reference arithmetic; got: {:?}",
        codes
    );
}
