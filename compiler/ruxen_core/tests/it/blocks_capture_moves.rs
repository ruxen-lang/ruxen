//! Blocks, procs and lambdas: moving a borrowed capture out of a closure
//! body (E1019) and closure arity (fewer block parameters, E0743), split
//! from `blocks_procs_lambdas` (plan: blocks-procs-lambdas). Every pin runs
//! the real lexer, parser, type checker and borrow checker.

use crate::blocks_procs_lambdas::{borrow_codes, parse, typeck_codes};
use ruxen_core::typeck;

/// Definitions shared by the move-out-of-a-block pins.
const MOVE_TARGETS: &str = r#"def run(f: Fn() -> nil) -> nil
  f.()
end

def give(f: Fn() -> String) -> String
  f.()
end

def eat(s: String) -> nil
  puts s
end

def peek(s: &String) -> nil
  puts s
end

def mk(k: Int) -> Option[String]
  Some("opt-#{k}")
end

class Holder
  fns: Array[Proc[() -> nil]]
  def init
    @fns = Array.new
  end
  def var add(f: Proc[() -> nil]) -> nil
    @fns.push(f)
  end
end
"#;

/// A block borrows its captures, so its body may not move one out: the
/// enclosing scope frees it too (use after free / double free). Every by-value
/// use is E1019: returning it as the block's value, binding it, passing it to
/// a by-value parameter, pushing it, putting it in an array literal, or
/// capturing it in an owning lambda (escaping or not). A closure nested in a
/// lambda borrows the lambda's capture the same way.
#[test]
fn semantics_moving_a_borrowed_capture_out_of_a_block_is_e1019() {
    let shape = |body: &str| {
        format!(
            "{MOVE_TARGETS}
k = 7
s = \"x-#{{k}}\"
{body}
puts s
"
        )
    };
    for body in [
        // p10: the block's value is the capture, stored by the caller.
        "arr = Array[String].new\narr.push(give do\n  s\nend)",
        // p1: the block's value is the capture.
        "out = give do\n  s\nend",
        // p2: a consuming argument.
        "run do\n  eat(s)\nend",
        // p3: a by-value binding.
        "run do\n  q = s\n  puts q\nend",
        // p8: pushed into a block-local array.
        "run do\n  arr = Array[String].new\n  arr.push(s)\nend",
        // c1: an array literal element.
        "run do\n  xs = [s]\n  puts xs.size\nend",
        // p7: captured by an owning lambda stored outside the block.
        "held = Array[Proc[() -> nil]].new\nrun do\n  held.push(-> { puts s })\nend",
        // p6: captured by an owning lambda handed to a `Proc[...]` param.
        "b = Holder.new\nrun do\n  b.add(-> { puts s })\nend",
        // x1: a cast is the value itself.
        "arr = Array[String].new\narr.push(give do\n  s as String\nend)",
        // x3: a by-value match binding moves the scrutinee.
        "arr = Array[String].new\narr.push(give do\n  match s\n    x -> x\n  end\nend)",
        // x2: a multi-assign right-hand side.
        "run do\n  q = \"a\"\n  n = 0\n  q, n = s, 1\n  puts q\nend",
        // n9: a tuple literal element.
        "run do\n  let (a, b) = (s, 1)\n  puts a\nend",
        // n4: an enum variant payload.
        "held = Array[Option[String]].new\nrun do\n  held.push(Some(s))\nend",
    ] {
        assert_eq!(borrow_codes(&shape(body)), vec!["E1019"], "`{body}`");
    }
    // c1b: a lambda nested in a lambda borrows the outer lambda's capture:
    // capturing it in the inner owning lambda moves it out of the outer body,
    // and so does pushing it inside the inner body.
    let nested = format!(
        "{MOVE_TARGETS}
k = 4
s = \"x-#{{k}}\"
outer = -> do
  inner = -> do
    xs = Array[String].new
    xs.push(s)
  end
  inner.()
end
outer.()
"
    );
    assert_eq!(borrow_codes(&nested), vec!["E1019", "E1019"]);
}

/// One rule for every closure kind: the body borrows its non-Copy captures
/// (the closure owns a proc's or lambda's captures, the enclosing scope owns a
/// block's). An owning lambda or proc whose body moves its own capture would
/// hand the same String out on every call and free it again on closure drop:
/// E1019 at the move, whatever the position.
#[test]
fn semantics_an_owning_closure_moving_its_own_capture_is_e1019() {
    let shape = |body: &str| {
        format!(
            "{MOVE_TARGETS}
k = 7
s = \"x-#{{k}}\"
o = mk(k)
{body}
f.()
f.()
"
        )
    };
    for body in [
        // n2: pushed into a body-local array.
        "f = -> do\n  xs = Array[String].new\n  xs.push(s)\nend",
        // h2: the same through a brace closure stored in a binding.
        "f = { ||\n  xs = Array[String].new\n  xs.push(s)\n}",
        // n2d: a proc binding its capture by value.
        "f = proc do\n  t = s\n  puts t\nend",
        // n1: the lambda's value is its capture.
        "f = -> { s }",
        // n2c: a consuming argument.
        "f = -> { eat(s) }",
        // a cast.
        "f = -> { s as String }",
        // o5: an `if let` binding moves the captured Option.
        "f = -> do\n  if let Some(v) = o\n    puts v\n  end\nend",
    ] {
        assert_eq!(borrow_codes(&shape(body)), vec!["E1019"], "`{body}`");
    }
}

/// Reads, auto-borrowed `&T` arguments and `.clone` stay allowed in an owning
/// lambda, and matching a temporary (`mk(k)`) moves nothing.
#[test]
fn semantics_reading_or_cloning_an_owned_capture_in_a_lambda_is_allowed() {
    let source = format!(
        "{MOVE_TARGETS}
k = 7
s = \"x-#{{k}}\"
f = -> do
  puts s.size
  puts \"#{{s}}!\"
  peek(s)
  peek(s)
  eat(s.clone)
  xs = Array[String].new
  xs.push(s.clone)
  t = s.clone
  puts t
  if let Some(v) = mk(k)
    puts v
  end
  s.clone as String
end
f.()
f.()
"
    );
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
}

/// Reads of a borrowed capture stay allowed in a block: `&self` method calls,
/// interpolation, auto-borrowed `&T` arguments, passing it on to a `Fn[...]`
/// parameter, and `.clone` for every shape E1019 rejects. A builtin type
/// constructor (`Array.new`) read in a lambda inside a block is no capture.
#[test]
fn semantics_reading_or_cloning_a_borrowed_capture_in_a_block_is_allowed() {
    let source = format!(
        "{MOVE_TARGETS}
k = 7
s = \"x-#{{k}}\"
run do
  puts s.size
  puts \"#{{s}}!\"
  peek(s)
  peek(s)
  eat(s.clone)
  q = s.clone
  xs = [s.clone]
  arr = Array[String].new
  arr.push(s.clone)
  t = s.clone
  run do
    puts t
  end
  fresh = -> do
    ys = Array[String].new
    ys.push(\"y\")
    puts ys.size
  end
  fresh.()
end
out = give do
  s.clone
end
cast = give do
  s.clone as String
end
bound = give do
  match s.clone
    x -> x
  end
end
run do
  q = \"a\"
  n = 0
  q, n = s.clone, 1
  puts q
  let (a, b) = (s.clone, 1)
  puts a
  match s
    \"lit\" -> puts \"lit\"
    _ -> puts \"other\"
  end
end
b = Holder.new
run do
  t = s.clone
  b.add(-> {{ puts t }})
end
outer = -> do
  mine = s.clone
  inner = -> do
    xs = Array[String].new
    xs.push(mine.clone)
  end
  inner.()
end
outer.()
"
    );
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
}

/// Ruby blocks and procs may take fewer parameters than the callable they are
/// bound to (the extra arguments are dropped); a lambda keeps exact arity, and
/// a non-callable stays E0743.
#[test]
fn semantics_blocks_and_procs_may_take_fewer_params() {
    let defs = "def run(f: Fn(Int) -> nil) -> nil\n  f.(1)\nend\n\n\
                def keep(p: Proc[(Int, String) -> nil]) -> nil\n  p.(1, \"s\")\nend\n";
    let accepted = format!(
        "{defs}
run do
  puts \"y\"
end
run() {{ puts \"z\" }}
keep {{ |n| puts n }}
keep(proc {{ puts \"p\" }})
[1, 2].each {{ puts \"x\" }}
"
    );
    assert_eq!(typeck_codes(&accepted), Vec::<String>::new());
    for body in [
        "run(-> { puts \"y\" })",
        "run(lambda { puts \"y\" })",
        "run(5)",
        "run { |a: Int, b: Int| puts a }",
    ] {
        let source = format!("{defs}\n{body}\n");
        assert_eq!(typeck_codes(&source), vec!["E0743"], "`{body}`");
    }
}

/// E0743 spells callable types the way they are written: a unit return is
/// `nil`, never `()`, and the message names the declared function.
#[test]
fn semantics_e0743_message_spells_nil() {
    let source = "def run(f: Fn(Int) -> nil)\n  f.(1)\nend\n\n\
                  def keep(p: Proc[(Int) -> nil])\n  p.(1)\nend\n\n\
                  run(->(s: String) { puts s })\nkeep(5)\n";
    let messages: Vec<String> = typeck::type_check(&parse(source).expect("parse"))
        .diagnostics
        .iter()
        .filter(|d| d.code.as_deref() == Some("E0743"))
        .map(|d| d.message.clone())
        .collect();
    assert_eq!(
        messages,
        vec![
            "`run` expects a closure `Fn(Int) -> nil` here, found `Fn(String) -> nil`",
            "`keep` expects a closure `Proc[(Int) -> nil]` here, found `Int`",
        ]
    );
}
