//! Block Tier-2 negative pins: a `&` block argument that is not last, and
//! forwarding `&block` from a method that takes none, are E1124; `next`
//! inside an MIR-inlined `each` is rejected rather than miscompiled.
//!
//! The positive cases (numbered params / `it`, `&:symbol` to-proc, `next`
//! as block value, `&` forwarding) are the release-e2e fixtures 943-946
//! and 1780.

use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Type-check `source`; return error-level diagnostics as (code, message).
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

/// Run the pipeline through MIR lowering; return `Err(msg)` if MIR lowering
/// rejects the program (used to pin the inlined-`next` refusal).
fn mir_lower_result(source: &str) -> Result<(), String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| d.message.clone())
        .collect();
    if !errors.is_empty() {
        return Err(format!("typecheck: {}", errors.join(" | ")));
    }
    let mut lowerer = Lowerer::new(&result.symbols);
    lowerer.lower_program(&result.program).map(|_| ())
}

#[test]
fn block_pass_not_last_is_e1124() {
    // A `&`-block argument anywhere but last is E1124.
    let source = r##"
def main
  let xs = [1, 2, 3]
  let _ = xs.foo(&:to_s, 9)
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1124"),
        "expected E1124, got {errors:?}"
    );
}

#[test]
fn next_in_inlined_each_is_rejected() {
    // `each` is MIR-inlined; a `next` there would be a non-local return, so it
    // is rejected (not silently miscompiled).
    let source = r##"
def main
  let xs = [1, 2, 3]
  xs.each { |x|
    if x == 2
      next
    end
    puts "#{x}"
  }
end
"##;
    let err = mir_lower_result(source).expect_err("inlined-each `next` must be rejected");
    assert!(
        err.contains("next") && err.contains("inlined"),
        "expected an inlined-`next` rejection, got: {err}"
    );
}

#[test]
fn forward_with_no_block_is_e1124() {
    let source = r##"
def inner(&block: Fn[() -> nil])
  yield
end

def outer
  inner(&block)
end

def main
  outer
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1124"),
        "expected E1124 (no block to forward), got {errors:?}"
    );
}
