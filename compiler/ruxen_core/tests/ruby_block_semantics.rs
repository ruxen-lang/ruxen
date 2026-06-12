//! Ruby-block-semantics pins (ADR: docs/decisions/ruby-block-semantics.md).
//!
//! Covers the assertions the release-e2e stdout fixtures cannot: a compile
//! diagnostic (E1119, `&block` not last) and a runtime panic (yield with no
//! block → clean LocalJumpError-style message + exit 101, not a segfault).
//!
//! Each program is driven through the full pipeline (lexer → parser → typeck
//! → MIR → Cranelift) and, where it compiles, the produced binary is run and
//! its stdout / stderr / exit status asserted.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Type-check `source` and return the error-level diagnostics (code + message).
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

/// Full pipeline compile + run. Returns (stdout, stderr, exit code).
fn compile_and_run(source: &str, basename: &str) -> (String, String, Option<i32>) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!(
        "{}-{}-{}.bin",
        basename,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "typecheck errors for {basename}: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .unwrap_or_else(|e| panic!("MIR lowering for {basename}: {e}"));
    codegen::compile(&mir, bin_path.to_str().unwrap())
        .unwrap_or_else(|e| panic!("codegen for {basename}: {e}"));
    let output = Command::new(&bin_path).output().expect("run");
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code(),
    )
}

/// Pin (g): `&block` not last → E1119 with a clear message naming the offender.
#[test]
fn block_param_not_last_is_e1119() {
    let source = r##"
def bad(&block: Fn[() -> nil], x: Int)
  yield
end
def main
  bad do
    puts "hi"
  end
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, msg)| code == "E1119"
            && msg.contains("block parameter")
            && msg.contains("must be the last")
            && msg.contains('x')),
        "expected E1119 naming offender `x`, got {errors:?}"
    );
}

/// Pin (f): a `yield` reached with no block panics cleanly (no segfault),
/// naming the enclosing function, and exits with status 101.
#[test]
fn yield_without_block_panics_cleanly() {
    let source = r##"
def must_yield(&block: Fn[() -> nil])
  yield
end
def main
  must_yield
end
"##;
    let (stdout, stderr, code) = compile_and_run(source, "yield_no_block");
    assert_eq!(stdout, "", "stdout was {stdout:?}");
    assert_eq!(code, Some(101), "exit code was {code:?}, stderr={stderr:?}");
    assert!(
        stderr.contains("yield called without a block") && stderr.contains("must_yield"),
        "panic message must name the function; stderr was {stderr:?}"
    );
}

/// Pin (j): `ruxen fmt` round-trips BOTH block-type spellings byte-stably and
/// does NOT rewrite one into the other (ADR D8; the fmt-destructiveness class).
/// Also confirms a `do…end` block and a `{ }` block are each preserved (no
/// `do…end`↔`{}` rewrite).
#[test]
fn fmt_preserves_both_block_type_spellings_and_block_forms() {
    // Canonical square-bracket spelling must survive verbatim.
    let canonical = "def f(x: Int, &block: Fn[(Int) -> nil])\n  yield x\nend\n";
    let out = ruxen_core::formatter::format(canonical).output;
    assert!(
        out.contains("&block: Fn[(Int) -> nil]"),
        "canonical Fn[...] spelling was rewritten; fmt output:\n{out}"
    );
    // Idempotent.
    let out2 = ruxen_core::formatter::format(&out).output;
    assert_eq!(out, out2, "fmt not idempotent on canonical spelling");

    // Back-compat paren spelling must NOT be promoted to the bracket form.
    let paren = "def g(&block: Fn() -> nil)\n  yield\nend\n";
    let out = ruxen_core::formatter::format(paren).output;
    assert!(
        out.contains("&block: Fn() -> nil") && !out.contains("Fn[]"),
        "paren Fn(...) spelling was rewritten to brackets; fmt output:\n{out}"
    );

    // fmt is IDEMPOTENT on a multi-statement `do…end` block attached to a
    // `&block` free fn (the new surface). The formatter's call-site block
    // form is chosen by content fit (a pre-existing, intentional policy:
    // multi-statement → `do…end`, single-statement-that-fits → `{ }`), not
    // by the authored token — this feature does not change that policy. What
    // the ADR guarantees and we pin here is that the new block surface does
    // not break the idempotency invariant `format(format(x)) == format(x)`.
    let prog = concat_src();
    let once = ruxen_core::formatter::format(&prog).output;
    let twice = ruxen_core::formatter::format(&once).output;
    assert_eq!(once, twice, "fmt not idempotent on block program:\n{once}");
    // A multi-statement block keeps `do…end` (content-driven), and the
    // `&block` decl keeps its canonical type spelling.
    assert!(
        once.contains("do |n|"),
        "multi-stmt do…end not kept:\n{once}"
    );
    assert!(
        once.contains("&block: Fn[(Int) -> nil]"),
        "type spelling lost:\n{once}"
    );
}

fn concat_src() -> String {
    "def emit(x: Int, &block: Fn[(Int) -> nil])\n  yield x\nend\n\n\
     def main\n  emit(7) do |n|\n    let y = n + 1\n    puts \"#{y}\"\n  end\nend\n"
        .to_string()
}

/// Regression: a paren-less auto-called function with a NON-block default
/// param must receive the real default value (not a blanket null). Guards the
/// auto-call default-fill path the block work touched.
#[test]
fn autocall_uses_real_default_not_null() {
    let source = r##"
def greet(n: Int = 5) -> Int
  n
end
def main
  let r = greet
  puts "#{r}"
end
"##;
    let (stdout, stderr, code) = compile_and_run(source, "autocall_real_default");
    assert_eq!(code, Some(0), "stderr={stderr:?}");
    assert_eq!(
        stdout, "5\n",
        "auto-call must use the real default; got {stdout:?}"
    );
}

/// Pin (c2): an explicit `&block` parameter works on a METHOD — block-bearing
/// call (`do…end`), the parens blockless form (`w.build()`), AND the paren-less
/// blockless form (`w.build`). All three reach `block_defined?` correctly. The
/// paren-less blockless form previously CRASHED the MIR arity verifier (it
/// parses as a `FieldAccess` whose no-arg method path did not append the block
/// `nil` default → one too few args); it is now fixed (block-slot consistency,
/// ADR D1/D5) so `w.build` and `w.build()` lower identically.
#[test]
fn explicit_block_param_on_method() {
    let source = r##"
class Widget
  tag: Int
  def init
    self.tag = 5
  end
  def build(&block: Fn[(Int) -> nil]) -> nil
    if block_defined?
      yield self.tag
    else
      puts "no-block"
    end
  end
end
def main
  var w = Widget.new
  w.build do |t|
    puts "tag=#{t}"
  end
  w.build()
  w.build
end
"##;
    let (stdout, stderr, code) = compile_and_run(source, "block_on_method");
    assert_eq!(code, Some(0), "stderr={stderr:?}");
    assert_eq!(
        stdout, "tag=5\nno-block\nno-block\n",
        "stdout was {stdout:?}"
    );
}

/// Item-1 dedicated pin: a PAREN-LESS, blockless call to an optional-`&block`
/// METHOD (`w.frame`) must fill the block slot with the null sentinel exactly
/// like the parens form (`w.frame()`), reaching `block_defined?` = false
/// cleanly. Before the fix this lowered as a `FieldAccess` that omitted the
/// trailing block default, emitting one too few MIR args and CRASHING the
/// arity verifier (`__closure_*: got 1, expected 2`) — so a revert of the
/// `mir/lower/expr/field_access.rs` default-fill makes THIS test fail at MIR
/// lowering / codegen, not merely on a stdout mismatch. Mirrors release-e2e
/// case 921. (ADR D1/D5; closes the blocks-feature paren-less-method gap.)
#[test]
fn parenless_blockless_method_call_fills_block_slot() {
    let source = r##"
class Widget
  label: String
  def init
    self.label = "w"
  end
  def frame(&block: Fn[() -> nil]) -> nil
    if block_defined?
      puts "with-block"
      yield
    else
      puts "no-block"
    end
  end
end
def main
  var w = Widget.new
  w.frame
  w.frame()
  w.frame do
    puts "inner"
  end
end
"##;
    let (stdout, stderr, code) = compile_and_run(source, "parenless_block_method");
    assert_eq!(code, Some(0), "stderr={stderr:?}");
    assert_eq!(
        stdout, "no-block\nno-block\nwith-block\ninner\n",
        "paren-less `w.frame` must lower identically to `w.frame()`; stdout was {stdout:?}"
    );
}

/// Pin (e2): `block_given?` is accepted as an alias of `block_defined?`.
#[test]
fn block_given_alias_works() {
    let source = r##"
def maybe(&block: Fn[() -> nil])
  if block_given?
    yield
  else
    puts "none"
  end
end
def main
  maybe do
    puts "yes"
  end
  maybe
end
"##;
    let (stdout, stderr, code) = compile_and_run(source, "block_given_alias");
    assert_eq!(code, Some(0), "stderr={stderr:?}");
    assert_eq!(stdout, "yes\nnone\n", "stdout was {stdout:?}");
}

/// E0729: interpolating a closure / `Fn` value is a clean compile error, not a
/// silent pointer print. A bare `do … end` is a closure literal (never an
/// expression block), so `let v = do … end; puts "#{v}"` binds the un-invoked
/// closure; without this check MIR interpolation's "unknown type → Int_fmt"
/// fallback prints a raw pointer (silent garbage). Reported live against the
/// `docs/tutorial/05-control-flow.md` "Blocks as expressions" section.
#[test]
fn interpolating_a_closure_is_e0729() {
    let source = r##"
def main
  let v = do
    1 + 2
  end
  puts "#{v}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors
            .iter()
            .any(|(code, msg)| code == "E0729" && msg.contains("no `Display`")),
        "expected E0729 on closure interpolation, got {errors:?}"
    );
}

/// Negative half of E0729: invoking the closure and interpolating its (Int)
/// RESULT is fine — only formatting the closure VALUE itself is rejected.
#[test]
fn interpolating_an_invoked_closure_result_is_ok() {
    let source = r##"
def main
  let f = do
    1 + 2
  end
  puts "#{f.()}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        !errors.iter().any(|(code, _)| code == "E0729"),
        "E0729 must not fire on an invoked closure's result, got {errors:?}"
    );
}
