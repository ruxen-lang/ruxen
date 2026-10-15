//! Phase 6 Task 7 characterization: the collection built-in names
//! (`COLLECTION_BUILTINS`) resolve through `resolve_type_expr`'s three
//! hardcoded arms to the dedicated `Ty::Array` / `Ty::Map` / `Ty::Set`
//! shapes — NOT to a generic `Ty::Class`. This pins the contract the shared
//! `COLLECTION_BUILTINS` const guards: the ffi anchor-only check
//! (ffi_registration.rs) and these resolve arms must list exactly the same
//! names, or `let x: Array[Int]` would resolve as a class and break the
//! collection ABI.

use ruxen_core::hir::nodes::{HirExpr, HirExprKind, HirItem, HirStatement};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Rendered (`Display`) types of every `let` binding in `main`, in order.
fn let_types(source: &str) -> Vec<String> {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);

    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors: {:?}", errors);

    let func = result
        .program
        .items
        .iter()
        .find_map(|item| match item {
            HirItem::Function(f) if f.name == "main" => Some(f),
            _ => None,
        })
        .expect("main not found");

    let mut out = Vec::new();
    collect(&func.body, &mut out);
    out
}

fn collect(expr: &HirExpr, out: &mut Vec<String>) {
    if let HirExprKind::Block(stmts, tail) = &expr.kind {
        for stmt in stmts {
            if let HirStatement::Let { ty, .. } = stmt {
                out.push(format!("{}", ty));
            }
        }
        if let Some(t) = tail {
            collect(t, out);
        }
    }
}

#[test]
fn array_resolves_to_ty_array() {
    // `Array[T]` is the Ruby spelling; the Rust `Vec` is gone
    // (ruby-naming.spec.md).
    let tys = let_types("def main\n  let a: Array[Int] = []\nend");
    assert_eq!(tys, vec!["Array[Int]".to_string()]);
}

#[test]
fn vec_is_an_undefined_type() {
    let tokens = Lexer::new("def main\n  let v: Vec[Int] = []\nend")
        .tokenize()
        .expect("lex");
    let prog = Parser::new(tokens).parse().expect("parse");
    let result = typeck::type_check(&prog);
    assert!(
        result.diagnostics.iter().any(|d| d.level
            == ruxen_core::diagnostics::DiagnosticLevel::Error
            && d.message.contains("undefined type `Vec`")),
        "{:?}",
        result.diagnostics
    );
}

#[test]
fn hash_resolves_to_ty_map() {
    // The Ruby spelling `Hash[K, V]` resolves to `Ty::Map`; the Rust
    // spellings `Map` / `HashMap` are no longer accepted.
    let tys = let_types("def main\n  let h: Hash[Int, Int] = { 1 => 2 }\nend");
    assert_eq!(tys, vec!["Hash[Int, Int]".to_string()]);
}

#[test]
fn set_resolves_to_ty_set() {
    // `Set[T]` is the Ruby spelling; the Rust `HashSet` is gone.
    let tys = let_types("def main\n  let s: Set[Int] = Set.new\nend");
    assert_eq!(tys, vec!["Set[Int]".to_string()]);
}
