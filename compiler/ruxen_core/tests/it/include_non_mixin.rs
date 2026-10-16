//! `include X` where `X` names a class, struct or enum rather than a mixin
//! is E1017. Ruby raises `TypeError (wrong argument type Class (expected
//! Module))`; Ruxen used to accept it silently, so `include Hash` (the
//! retired spelling of `include Hashable`) compiled and gave the type
//! nothing.

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Type-check `source`; return its error-level diagnostics.
fn errors(source: &str) -> Vec<Diagnostic> {
    let tokens = Lexer::new(source).tokenize().expect("lex");
    let program = Parser::new(tokens).parse().expect("parse");
    typeck::type_check(&program)
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

fn codes(diags: &[Diagnostic]) -> Vec<String> {
    diags
        .iter()
        .map(|d| d.code.clone().unwrap_or_default())
        .collect()
}

#[test]
fn including_a_user_class_is_e1017() {
    let diags = errors("class Bar\nend\n\nclass Foo\n  include Bar\nend\n\ndef main\nend\n");
    assert_eq!(codes(&diags), vec!["E1017".to_string()]);
    assert!(
        diags[0].message.contains("`Bar` is a class, not a mixin"),
        "{}",
        diags[0].message
    );
}

#[test]
fn including_array_in_a_struct_is_e1017() {
    let diags = errors("struct P\n  x: Int\n  include Array\nend\n\ndef main\nend\n");
    assert_eq!(codes(&diags), vec!["E1017".to_string()]);
}

#[test]
fn including_hash_is_e1017_with_a_hashable_hint() {
    let diags = errors("class Foo\n  include Hash\nend\n\ndef main\nend\n");
    assert_eq!(codes(&diags), vec!["E1017".to_string()]);
    assert!(
        diags[0].message.contains("include Hashable"),
        "{}",
        diags[0].message
    );
}

#[test]
fn including_real_mixins_still_compiles() {
    let src = "mixin Greeting\n  def greeting -> Int\n    1\n  end\nend\n\n\
               class Rank\n  value: Int\n\n  include Comparable\n  include Greeting\n\n  \
               def init(@value: Int)\n  end\n\n  \
               def <=>(other: &Rank) -> Int\n    self.value <=> other.value\n  end\nend\n\n\
               struct Key\n  id: Int\n  include Hashable\n  include Eq\n  include PartialEq\nend\n\n\
               def main\nend\n";
    assert_eq!(codes(&errors(src)), Vec::<String>::new());
}

#[test]
fn a_module_mixin_wins_over_a_top_level_class_of_the_same_name() {
    let src = "class Reader\nend\n\n\
               module Buf\n  mixin Reader\n    def tag -> Int\n      1\n    end\n  end\n\n  \
               class File\n    include Reader\n  end\nend\n\n\
               def main\nend\n";
    assert_eq!(codes(&errors(src)), Vec::<String>::new());
}
