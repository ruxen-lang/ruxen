//! Regression guard for ROADMAP P0.7 — string literals must be wrapped
//! at MIR lowering via an implicit `ruxen_string_from` call so that any
//! later `String::drop` -> `free()` cannot double-free a pointer into
//! `.rodata`. The wrap lives in `mir/lower.rs::emit_owned_string_literal`.

use ruxen_core::hir::types::Ty;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::mir::nodes::MirInst;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// The MIR-local type of the `let s = …` binding named `s` in `main`.
fn s_local_ty(source: &str) -> Ty {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer.lower_program(&result.program).expect("lower");
    let main = mir
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main");
    main.locals
        .iter()
        .find(|l| l.name == "s")
        .map(|l| l.ty.clone())
        .expect("local s")
}

/// An UN-annotated `let s = "x"` binds an OWNED `String` (so it drops at scope
/// exit, no leak — ledger Q38), identical to `let s = String.from("x")`. An
/// explicit `let s: &str = "x"` keeps the borrow type untouched.
#[test]
fn bare_string_literal_let_binds_owned_string() {
    assert_eq!(
        s_local_ty("def main\n  let s = \"hello\"\n  let _l = s.size\nend\n"),
        Ty::String,
        "an un-annotated `let s = \"...\"` must bind an owned String (else it leaks)"
    );
    assert_eq!(
        s_local_ty("def main\n  let s = String.from(\"hello\")\n  let _l = s.size\nend\n"),
        Ty::String,
        "String.from binding stays String (control)"
    );
    // Explicit &str annotation is left as a borrow (not promoted).
    assert!(
        !matches!(
            s_local_ty("def main\n  let s: &str = \"hello\"\n  let _l = s.size\nend\n"),
            Ty::String
        ),
        "an explicit `let s: &str` annotation must NOT be promoted to String"
    );
}

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Lowering `let s = "hello"` must emit a `Call { callee: "ruxen_string_from", ... }`
/// so the local holds an owned heap-allocated `String`, not a pointer into `.rodata`.
#[test]
fn string_literal_lowers_through_string_from_wrapper() {
    let source = rx("string_literal_lowers_through_string_from_wrapper");

    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    assert!(
        result
            .diagnostics
            .iter()
            .all(|d| d.level != ruxen_core::diagnostics::DiagnosticLevel::Error),
        "unexpected typeck errors: {:?}",
        result.diagnostics
    );

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer.lower_program(&result.program).expect("lower");

    let main_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main fn in MIR");

    let mut saw_literal = false;
    let mut saw_string_from = false;

    for block in &main_fn.blocks {
        for inst in &block.instructions {
            match inst {
                MirInst::StringLiteral { value, .. } if value == "hello" => {
                    saw_literal = true;
                }
                MirInst::Call { callee, .. } if callee == "ruxen_string_from" => {
                    saw_string_from = true;
                }
                _ => {}
            }
        }
    }

    assert!(
        saw_literal,
        "expected MIR to contain a StringLiteral instruction for \"hello\""
    );
    assert!(
        saw_string_from,
        "P0.7 regression: string literal was NOT wrapped in ruxen_string_from. \
         Without the wrap, String::drop -> free() on the literal pointer would double-free."
    );
}

/// Coercion guard (typeck half of release-e2e case 922): a BARE string literal
/// coerces to the right string type in every position we rely on after dropping
/// `String.from("literal")` everywhere. OWNED direction — function param,
/// struct field, static-method arg, `Err(...)` into `Result[T, String]`,
/// `Array[String].push`, var reassignment + interpolation, return. BORROW
/// direction — a bare literal into a `&String` param, into a `&str` param, and
/// as the borrowed `&String` needle of `include?` (`""` is owned `String`; at a
/// call site it also satisfies `&String`/`&str` — the two are distinct types
/// the unifier bridges as equivalent). Zero typeck errors is the invariant; a
/// regression would break the whole swept corpus across the four repos (docs +
/// stdlib). The run+stdout half is pinned by release-e2e
/// `922_string_literal_coercion_all_positions`.
#[test]
fn bare_string_literal_coerces_to_string_in_all_positions() {
    let source = r##"
class Person
  name: String
  def init(n: String)
    self.name = n
  end
  def greeting -> String
    "hi #{self.name}"
  end
end
def label(s: String) -> String
  s
end
def borrow_string(s: &String) -> USize
  s.size
end
def borrow_str(s: &str) -> USize
  s.size
end
def check(a: Int, b: Int) -> Result[Int, String]
  if b == 0
    Err("nope")
  else
    Ok(a / b)
  end
end
def main
  let p = Person.new("Ada")
  puts p.greeting
  puts label("badge")
  var xs: Array[String] = []
  xs.push("one")
  var msg = "start"
  msg = "now #{label("end")}"
  puts msg
  let _ = check(1, 0)
  # BORROW direction: a bare literal also coerces into &String and &str params,
  # and to the borrowed `&String` needle of `include?` (no leading `&` needed).
  let _b1 = borrow_string("abcd")
  let _b2 = borrow_str("abc")
  let _b3 = "haystack".include?("st")
end
"##;
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
    assert!(
        errors.is_empty(),
        "bare string literal must coerce to String in every position; \
         got typeck errors: {errors:?}"
    );
}
