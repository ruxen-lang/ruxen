//! F-2 (no silent stubs): shapes that used to compile to a runtime no-op
//! are coded errors now.
//!
//! - `super(...)` outside a subclass `init` used to lower to `ruxen_noop`
//!   and silently skip the call; it is E1127.
//! - Indexing a receiver with no index lowering used to fall through the
//!   MIR catch-all and yield an unset value; it is E1128.

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Type-check `source`; return the codes of the error-level diagnostics.
fn error_codes(source: &str) -> Vec<String> {
    let tokens = Lexer::new(source).tokenize().expect("lex");
    let program = Parser::new(tokens).parse().expect("parse");
    typeck::type_check(&program)
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| d.code.clone().unwrap_or_default())
        .collect()
}

#[test]
fn super_in_a_free_function_is_e1127() {
    let src = "def helper\n  super()\nend\n\ndef main\n  helper\nend\n";
    assert_eq!(error_codes(src), vec!["E1127".to_string()]);
}

#[test]
fn super_in_init_of_a_class_without_parent_is_e1127() {
    let src = "class Point\n  def init(x: Int)\n    super(x)\n  end\nend\n\ndef main\n  let p = Point.new(1)\nend\n";
    assert_eq!(error_codes(src), vec!["E1127".to_string()]);
}

#[test]
fn super_in_a_non_init_method_of_a_subclass_is_e1127() {
    let src = "class Base\n  def init\n  end\nend\n\nclass Child < Base\n  def init\n    super()\n  end\n\n  def again -> Int\n    super()\n    1\n  end\nend\n\ndef main\n  let c = Child.new()\nend\n";
    assert_eq!(error_codes(src), vec!["E1127".to_string()]);
}

#[test]
fn indexing_a_tuple_is_e1128() {
    let src = "def main\n  let t = (1, 2)\n  let i = 1\n  puts t[i]\nend\n";
    assert!(
        error_codes(src).contains(&"E1128".to_string()),
        "{:?}",
        error_codes(src)
    );
}
