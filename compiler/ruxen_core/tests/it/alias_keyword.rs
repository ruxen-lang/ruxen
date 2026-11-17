//! Ruby `alias new_name old_name` pins (ADR: docs/decisions/alias-keyword.md).
//!
//! Covers the assertions the release-e2e stdout fixtures cannot: the alias
//! diagnostics (E1120 unknown target, E1122 collision/self-alias, E1123 staged
//! unsupported operator alias) and the fmt byte-stability of the alias item.
//! The runtime synonym behaviour is the release-e2e cases 470-472 and
//! 913-918.

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Type-check `source`; return the error-level diagnostics (code + message).
fn typecheck_errors(source: &str) -> Vec<(String, String)> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| (d.code.clone().unwrap_or_default(), d.message.clone()))
        .collect()
}

/// Pin (g): aliasing an unknown method target → E1120.
#[test]
fn unknown_method_alias_target_is_e1120() {
    let source = r##"
class C
  def a -> Int
    1
  end
  alias b nonexistent
end
def main
  let c = C.new
  puts "#{c.a}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1120"),
        "expected E1120 for unknown alias target, got {errors:?}"
    );
}

/// Pin (g2): aliasing an unknown free-function target → E1120.
#[test]
fn unknown_free_fn_alias_target_is_e1120() {
    let source = r##"
def f -> Int
  1
end
alias g does_not_exist
def main
  puts "#{f()}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1120"),
        "expected E1120 for unknown free-fn alias target, got {errors:?}"
    );
}

/// Pin (h): a self-alias (`alias x x`) is the degenerate collision → E1122.
#[test]
fn self_alias_is_e1122() {
    let source = r##"
class C
  def a -> Int
    1
  end
  alias a a
end
def main
  let c = C.new
  puts "#{c.a}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1122"),
        "expected E1122 for self-alias, got {errors:?}"
    );
}

/// Pin (h2): an alias whose NEW name collides with an existing method → E1122.
#[test]
fn collision_with_existing_method_is_e1122() {
    let source = r##"
class C
  def a -> Int
    1
  end
  def b -> Int
    2
  end
  alias a b
end
def main
  let c = C.new
  puts "#{c.a}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1122"),
        "expected E1122 for alias colliding with existing method, got {errors:?}"
    );
}

/// `+@` has no surface operator expression and remains staged with E1123.
#[test]
fn unsupported_unary_plus_alias_is_e1123() {
    let source = r##"
class V
  def value -> Int
    1
  end
  alias +@ value
end
def main
  let v = V.new
  puts "#{v.value}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1123"),
        "expected E1123 for unsupported +@ alias, got {errors:?}"
    );
}

/// Pin (i): `ruxen fmt` round-trips the alias item byte-stably (ADR D10) — the
/// literal `alias new old` line survives and `format(format(x)) == format(x)`.
#[test]
fn fmt_round_trips_alias_byte_stably() {
    let src = "class Bag\n  count: Int\n  def size -> Int\n    self.count\n  end\n  alias length size\nend\n";
    let out = ruxen_core::formatter::format(src).output;
    assert!(
        out.contains("alias length size"),
        "alias item not preserved verbatim; fmt output:\n{out}"
    );
    let out2 = ruxen_core::formatter::format(&out).output;
    assert_eq!(out, out2, "fmt not idempotent on alias item");

    // Free-fn alias at top level round-trips too.
    let free = "def greet -> Int\n  1\nend\n\nalias hail greet\n";
    let fout = ruxen_core::formatter::format(free).output;
    assert!(
        fout.contains("alias hail greet"),
        "free-fn alias not preserved; fmt output:\n{fout}"
    );
    assert_eq!(
        fout,
        ruxen_core::formatter::format(&fout).output,
        "fmt not idempotent on free-fn alias"
    );
}
