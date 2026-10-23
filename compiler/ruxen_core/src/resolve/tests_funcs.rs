//! Resolve diagnostics raised by `funcs.rs`: E0737 (`where` predicates),
//! E0738 (`use` group misses) and E0739 (parameters on `main`).

use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::resolve::Resolver;

fn resolve_codes(source: &str) -> Vec<String> {
    let tokens = Lexer::new(source).tokenize().expect("lexer failed");
    let program = Parser::new(tokens).parse().expect("parser failed");
    Resolver::new()
        .resolve(&program)
        .diagnostics
        .into_iter()
        .filter_map(|d| d.code)
        .collect()
}

#[test]
fn where_predicate_on_a_non_generic_subject_is_e0737() {
    let codes = resolve_codes(
        "mixin Add\nend\ndef total[T](items: &Array[T]) -> Int where Int: Add\n  0\nend\n",
    );
    assert_eq!(codes, vec!["E0737"]);
}

#[test]
fn where_predicate_on_an_own_generic_is_accepted() {
    let codes = resolve_codes(
        "mixin Add\nend\ndef total[T](items: &Array[T]) -> Int where T: Add\n  0\nend\n",
    );
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn use_group_miss_after_flat_merge_fallback_is_e0738() {
    let codes =
        resolve_codes("class Box\nend\nclass Holder\nend\nuse pkg.Holder.Box.{Box, Nope}\n");
    assert!(codes.contains(&"E0738".to_string()), "{codes:?}");
}

#[test]
fn main_with_parameters_is_e0739() {
    let codes = resolve_codes("def main(x: Int)\n  x\nend\n");
    assert_eq!(codes, vec!["E0739"]);
}

#[test]
fn main_without_parameters_and_methods_named_main_are_accepted() {
    for src in [
        "def main\nend\n",
        "def main()\nend\n",
        "class App\n  def main(x: Int) -> Int\n    x\n  end\nend\ndef main\nend\n",
    ] {
        let codes = resolve_codes(src);
        assert!(codes.is_empty(), "{src:?}: {codes:?}");
    }
}
