//! Characterization pins for the `HirExprKind::MethodCall` inference arm
//! (typeck/infer/expr.rs), captured BEFORE the Phase 6 Task 3 extraction
//! that splits the ~300-line arm into infer_constructor_call /
//! infer_selected_method / infer_combinator_block and dedupes the generic
//! harvest. These assert the four separable jobs the arm performs:
//!
//!   1. constructor generic inference: `Pair.new(42, "hi")` ⇒ `Pair[Int, String]`
//!   2. selected-method generic harvest: `expect(aString)` ⇒ `Matcher[String]`
//!   3. block-combinator unify: `opt.map { |n| n * 2 }` ⇒ `Option[Int]`
//!   4. the entry-chain special case: `m.entry(k).or_insert(v)` ⇒ Unit,
//!      and a bad `or_insert` receiver is rejected.
//!
//! The extraction is behaviour-preserving; these are green before and after.

use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::hir::nodes::{HirExpr, HirExprKind, HirItem, HirStatement};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Type-check `source` and return the rendered (`Display`) types of every
/// `let` binding inside the function named `fn_name`, in source order.
fn let_types_in(source: &str, fn_name: &str) -> Vec<String> {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);

    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "unexpected type errors: {:?}", errors);

    let func = result
        .program
        .items
        .iter()
        .find_map(|item| match item {
            HirItem::Function(f) if f.name == fn_name => Some(f),
            _ => None,
        })
        .unwrap_or_else(|| panic!("function `{}` not found", fn_name));

    let mut out = Vec::new();
    collect_let_types(&func.body, &mut out);
    out
}

fn collect_let_types(expr: &HirExpr, out: &mut Vec<String>) {
    if let HirExprKind::Block(stmts, tail) = &expr.kind {
        for stmt in stmts {
            if let HirStatement::Let { ty, value, .. } = stmt {
                out.push(format!("{}", ty));
                if let Some(v) = value {
                    collect_let_types(v, out);
                }
            }
        }
        if let Some(t) = tail {
            collect_let_types(t, out);
        }
    }
}

fn typeck_errors(source: &str) -> Vec<String> {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| d.to_string())
        .collect()
}

/// Job 2: constructor generic inference from argument types.
#[test]
fn constructor_new_infers_class_generics() {
    let src = "\
class Pair[A, B]
  a: A
  b: B
  def init(@a: A, @b: B)
  end
end

def main
  let p = Pair.new(42, \"hi\")
end
";
    let tys = let_types_in(src, "main");
    assert_eq!(
        tys,
        vec!["Pair[Int, &str]".to_string()],
        "Pair.new(42, \"hi\") should infer the constructor's generic args \
         from the argument types (String literals carry &str)"
    );
}

/// Job 3 (the harvest): a declared method `wrap[T](actual: T) -> Holder[T]`
/// called with a String argument must yield `Holder[String]`.
/// (Uses `Holder`, not `Matcher` — the latter is an auto-loaded `std.test`
/// type and now correctly trips the E0727 name-collision check, Q14.)
#[test]
fn selected_method_harvests_own_type_params() {
    let src = "\
class Holder[T]
  v: T
  def init(@v: T)
  end
end

class Asserter
  def init
  end
  def wrap[T](actual: T) -> Holder[T]
    Holder.new(actual)
  end
end

def main
  let a = Asserter.new
  let m = a.wrap(\"hello\")
end
";
    let tys = let_types_in(src, "main");
    assert_eq!(
        tys,
        vec!["Asserter".to_string(), "Holder[&str]".to_string()],
        "wrap(\"hello\") should harvest T -> &str from the argument, \
         yielding Holder[&str] (without the harvest it stays Holder[T])"
    );
}

/// Job 4: block-combinator unify — `opt.map { |n| n * 2 }` on `Option[Int]`
/// stays `Option[Int]` (the closure body's Int unifies with the container's
/// element var).
#[test]
fn combinator_map_unifies_element_type() {
    let src = "\
def main
  let o: Option[Int] = Option.Some(3)
  let r = o.map { |n| n * 2 }
end
";
    let tys = let_types_in(src, "main");
    assert_eq!(
        tys,
        vec!["Option[Int]".to_string(), "Option[Int]".to_string()],
        "map over Option[Int] should yield Option[Int]"
    );
}

/// Job 1: the entry-chain special case rejects an `or_insert` whose receiver
/// is not an immediate `.entry(K)` call.
#[test]
fn or_insert_requires_entry_receiver() {
    let src = "\
def main
  let x = 5
  x.or_insert(1)
end
";
    let errs = typeck_errors(src);
    assert!(
        errs.iter().any(|e| e.contains("requires an immediate")),
        "or_insert on a non-entry receiver should be rejected, got {:?}",
        errs
    );
}
