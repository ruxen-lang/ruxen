//! Block Tier-2 pins (block ADR, staged): numbered params / `it`, `&:symbol`
//! to-proc, `next` as block-value, and `&` block-forwarding.
//!
//! Each program is driven through the full pipeline (lexer → parser → typeck →
//! MIR → Cranelift); where it compiles, the binary runs and its stdout / exit
//! is asserted. Complements the release-e2e stdout fixtures with focused
//! compile-diagnostic and behavioral assertions.

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

// ─── Feature 1: numbered params / `it` ──────────────────────────────────

#[test]
fn numbered_param_single() {
    let source = r##"
def main
  let xs = [1, 2, 3]
  let ys = xs.map { _1 * 2 }
  puts "#{ys[0]} #{ys[1]} #{ys[2]}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "num_single");
    assert_eq!(stdout, "2 4 6\n");
    assert_eq!(code, Some(0));
}

#[test]
fn implicit_it_param() {
    let source = r##"
def main
  let xs = [1, 2, 3]
  let ys = xs.map { it + 10 }
  puts "#{ys[0]} #{ys[1]} #{ys[2]}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "it_param");
    assert_eq!(stdout, "11 12 13\n");
    assert_eq!(code, Some(0));
}

#[test]
fn numbered_param_arity_two_via_reduce() {
    let source = r##"
def main
  let xs = [1, 2, 3]
  let total = xs.reduce(0) { _1 + _2 }
  puts "#{total}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "num_arity2");
    assert_eq!(stdout, "6\n");
    assert_eq!(code, Some(0));
}

#[test]
fn it_as_method_call_receiver() {
    let source = r##"
def main
  let names = ["ab", "cde"]
  let lens = names.map { it.size }
  puts "#{lens[0]} #{lens[1]}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "it_receiver");
    assert_eq!(stdout, "2 3\n");
    assert_eq!(code, Some(0));
}

// ─── Feature 2: `&:symbol` to-proc ──────────────────────────────────────

#[test]
fn to_proc_map() {
    let source = r##"
def main
  let names = ["ab", "cde", "f"]
  let lens = names.map(&:size)
  puts "#{lens[0]} #{lens[1]} #{lens[2]}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "to_proc_map");
    assert_eq!(stdout, "2 3 1\n");
    assert_eq!(code, Some(0));
}

#[test]
fn to_proc_equivalent_to_explicit_block() {
    // `&:size` must behave identically to `{ |x| x.size }`.
    let via_sugar = r##"
def main
  let xs = ["aa", "b"]
  let ys = xs.map(&:size)
  puts "#{ys[0]}#{ys[1]}"
end
"##;
    let via_block = r##"
def main
  let xs = ["aa", "b"]
  let ys = xs.map { |s| s.size }
  puts "#{ys[0]}#{ys[1]}"
end
"##;
    let (a, _, ca) = compile_and_run(via_sugar, "to_proc_eq_a");
    let (b, _, cb) = compile_and_run(via_block, "to_proc_eq_b");
    assert_eq!(a, "21\n");
    assert_eq!(a, b);
    assert_eq!(ca, Some(0));
    assert_eq!(cb, Some(0));
}

#[test]
fn to_proc_bool_predicate_in_select() {
    let source = r##"
def main
  let mixed = ["", "x", "", "yz"]
  let blanks = mixed.select(&:empty?)
  puts "#{blanks.size}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "to_proc_select");
    assert_eq!(stdout, "2\n");
    assert_eq!(code, Some(0));
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

// ─── Feature 3: `next` as block-value ───────────────────────────────────

#[test]
fn next_value_in_map_real_closure() {
    // `map` on a real Array routes to a real closure; `next expr` early-returns
    // this iteration's mapped value.
    let source = r##"
def main
  let xs = [1, 2, 3, 4]
  let ys = xs.map { |x|
    if x % 2 == 0
      next 0
    end
    x * 10
  }
  puts "#{ys[0]} #{ys[1]} #{ys[2]} #{ys[3]}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "next_map");
    assert_eq!(stdout, "10 0 30 0\n");
    assert_eq!(code, Some(0));
}

#[test]
fn next_value_in_select_predicate() {
    let source = r##"
def main
  let xs = [1, 2, 3, 4]
  let keep = xs.select { |x|
    if x == 3
      next false
    end
    x > 1
  }
  puts "#{keep.size}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "next_select");
    assert_eq!(stdout, "2\n");
    assert_eq!(code, Some(0));
}

#[test]
fn next_value_in_yield_block() {
    // The block passed to a `&block`/`yield` method is a real closure; `next`
    // early-returns its value as `yield`'s value.
    let source = r##"
def transform(n: Int, &block: Fn[(Int) -> Int]) -> Int
  yield(n)
end

def main
  let a = transform(5) { |x|
    if x > 3
      next 100
    end
    x
  }
  let b = transform(2) { |x|
    if x > 3
      next 100
    end
    x
  }
  puts "#{a} #{b}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "next_yield");
    assert_eq!(stdout, "100 2\n");
    assert_eq!(code, Some(0));
}

#[test]
fn next_as_sole_tail_expression() {
    // Degenerate: the block body is JUST `next expr` (tail is `Never`). The
    // closure's result type must still be inferred from the `next` value.
    let source = r##"
def main
  let xs = [1, 2, 3]
  let ys = xs.map { |x| next x * 100 }
  puts "#{ys[0]} #{ys[1]} #{ys[2]}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "next_sole");
    assert_eq!(stdout, "100 200 300\n");
    assert_eq!(code, Some(0));
}

#[test]
fn next_and_numbered_params_in_do_end_block() {
    // `do … end` blocks must recognize `next` and implicit `_1` too (not only
    // brace blocks).
    let source = r##"
def main
  let xs = [1, 2, 3, 4]
  let ys = xs.map do |x|
    if x == 3
      next 0
    end
    x
  end
  let zs = xs.map do
    _1 * 5
  end
  puts "#{ys[2]} #{zs[0]} #{zs[3]}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "do_end_next");
    assert_eq!(stdout, "0 5 20\n");
    assert_eq!(code, Some(0));
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
fn dot_next_field_access_still_works() {
    // `next` as a field name (postfix `.next`) must be unaffected by the
    // block-value keyword — it is only special at expression-start in a block.
    let source = r##"
class Node
  next: Int
  def init(@next: Int)
  end
end

def main
  let n = Node.new(7)
  puts "#{n.next}"
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "dot_next");
    assert_eq!(stdout, "7\n");
    assert_eq!(code, Some(0));
}

// ─── Feature 4: `&` block-forwarding ────────────────────────────────────

#[test]
fn forward_named_block_to_free_fn() {
    let source = r##"
def inner(&block: Fn[(Int) -> nil])
  yield(1)
  yield(2)
end

def outer(&block: Fn[(Int) -> nil])
  inner(&block)
end

def main
  outer do |x|
    puts "got #{x}"
  end
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "fwd_named");
    assert_eq!(stdout, "got 1\ngot 2\n");
    assert_eq!(code, Some(0));
}

#[test]
fn forward_anonymous_ampersand() {
    let source = r##"
def inner(&block: Fn[(Int) -> nil])
  yield(7)
end

def outer(&block: Fn[(Int) -> nil])
  inner(&)
end

def main
  outer do |x|
    puts "x=#{x}"
  end
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "fwd_anon");
    assert_eq!(stdout, "x=7\n");
    assert_eq!(code, Some(0));
}

#[test]
fn forward_block_to_method_target() {
    let source = r##"
class Emitter
  tag: Int
  def init(@tag: Int)
  end
  def emit(&block: Fn[(Int) -> nil]) -> nil
    yield(self.tag)
    yield(self.tag + 1)
  end
end

def relay(e: Emitter, &block: Fn[(Int) -> nil]) -> nil
  e.emit(&block)
end

def main
  let e = Emitter.new(10)
  relay(e) do |n|
    puts "n=#{n}"
  end
end
"##;
    let (stdout, _stderr, code) = compile_and_run(source, "fwd_method");
    assert_eq!(stdout, "n=10\nn=11\n");
    assert_eq!(code, Some(0));
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
