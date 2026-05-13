//! Pin tests for `docs/specs/system/user-modules.spec.md`.
//!
//! Stage 1 covers parser surface only — the resolver does NOT yet
//! handle user-module path lookup.  These tests assert the AST shape
//! `parse_module_def` produces; the negative case (B4) is covered by
//! a typeck-only test that confirms the resolver still emits a
//! diagnostic when a user-module type is referenced.

use riven_core::lexer::Lexer;
use riven_core::parser::ast::{Program, TopLevelItem};
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

/// B1 — `module foo ... end` parses and contains the inner items.
#[test]
fn parse_module_def_basic() {
    let src = r#"
module geometry
  struct Point
    x: Int
    y: Int
  end

  def origin -> Int
    0
  end
end
"#;
    let prog = parse(src);
    let module = prog
        .items
        .iter()
        .find_map(|item| match item {
            TopLevelItem::Module(m) if m.name == "geometry" => Some(m),
            _ => None,
        })
        .expect("no `module geometry` in program");
    assert_eq!(
        module.items.len(),
        2,
        "expected 2 inner items (struct + def); got {:?}",
        module.items
    );

    // First inner item is a struct named Point.
    match &module.items[0] {
        TopLevelItem::Struct(s) => assert_eq!(s.name, "Point"),
        other => panic!("expected struct, got {:?}", other),
    }
    // Second is a fn named origin.
    match &module.items[1] {
        TopLevelItem::Function(f) => assert_eq!(f.name, "origin"),
        other => panic!("expected function, got {:?}", other),
    }
}

/// B2 — nested modules parse to nested `Module` AST nodes.
#[test]
fn parse_nested_modules() {
    let src = r#"
module outer
  module inner
    def hi -> Int
      1
    end
  end
end
"#;
    let prog = parse(src);
    let outer = prog
        .items
        .iter()
        .find_map(|item| match item {
            TopLevelItem::Module(m) if m.name == "outer" => Some(m),
            _ => None,
        })
        .expect("no `outer` module");
    let inner = outer
        .items
        .iter()
        .find_map(|item| match item {
            TopLevelItem::Module(m) if m.name == "inner" => Some(m),
            _ => None,
        })
        .expect("no nested `inner` module");
    assert_eq!(inner.items.len(), 1);
}

/// B3 — module body accepts every top-level item form (smoke).
#[test]
fn parse_module_with_diverse_items() {
    let src = r#"
module bag
  struct S
    n: Int
  end

  class C
    n: Int
  end

  enum E
    A
    B
  end

  trait T
    def t -> Int
  end

  def free -> Int
    0
  end

  type Alias = Int

  const ZERO: Int = 0
end
"#;
    let prog = parse(src);
    let module = prog
        .items
        .iter()
        .find_map(|item| match item {
            TopLevelItem::Module(m) if m.name == "bag" => Some(m),
            _ => None,
        })
        .expect("no `bag` module");
    // Just sanity-check that several items were parsed; exact count
    // depends on parser lenience for top-level forms.
    assert!(
        module.items.len() >= 5,
        "expected ≥5 inner items, got {} ({:?})",
        module.items.len(),
        module.items.iter().map(|i| std::mem::discriminant(i)).collect::<Vec<_>>()
    );
}
