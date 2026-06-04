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
fn array_vec_resolve_to_ty_array() {
    let tys = let_types("def main\n  let a: Array[Int] = []\n  let v: Vec[Int] = []\nend");
    assert_eq!(
        tys,
        vec!["Array[Int]".to_string(), "Array[Int]".to_string()]
    );
}

#[test]
fn map_hashmap_resolve_to_ty_map() {
    let tys = let_types(
        "def main\n  let m: Map[Int, Int] = { 1 => 2 }\n  let h: HashMap[Int, Int] = { 1 => 2 }\nend",
    );
    assert_eq!(
        tys,
        vec!["Map[Int, Int]".to_string(), "Map[Int, Int]".to_string()]
    );
}

#[test]
fn set_hashset_resolve_to_ty_set() {
    let tys =
        let_types("def main\n  let s: Set[Int] = Set.new\n  let hs: HashSet[Int] = Set.new\nend");
    assert_eq!(tys, vec!["Set[Int]".to_string(), "Set[Int]".to_string()]);
}
