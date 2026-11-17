//! A by-value pattern binding of a non-Copy payload moves the scrutinee
//! variable (P-3). Two `if let Some(v) = o` on an `Option[String]` handed the
//! same String to two bindings, and each binding is dropped: a double free.
//! Moves now recorded at more by-value positions (`return`, patterns), so a
//! branch that leaves (`return`, `break`, `next`) must not carry its moves
//! into the code after the branch. Every pin runs the real lexer, parser,
//! type checker and borrow checker.

use crate::blocks_procs_lambdas::borrow_codes;

const DEFS: &str = "def mk(k: Int) -> Option[String]\n  Some(\"opt-#{k}\")\nend\n\n\
                    def mk_int(k: Int) -> Option[Int]\n  Some(k)\nend\n";

fn program(body: &str) -> String {
    format!("{DEFS}\n{body}")
}

/// A second `if let`, `match` or `case ... in` on a variable whose payload was
/// bound by value is a use after move (o1, o4).
#[test]
fn semantics_binding_a_payload_moves_the_scrutinee_variable() {
    for body in [
        // o1: two `if let` on the same Option[String].
        "o = mk(7)\nif let Some(v) = o\n  puts v\nend\nif let Some(v) = o\n  puts v\nend\n",
        // o4: a `match` arm binding, then an `if let`.
        "o = mk(7)\nheld = Array[String].new\nmatch o\n  Some(v) -> held.push(v)\n  nil -> nil\nend\nputs held.size\nif let Some(v) = o\n  puts v\nend\n",
        // `case ... in`, then an `if let`.
        "o = mk(7)\ncase o\nin Some(v)\n  puts v\nelse\n  puts \"none\"\nend\nif let Some(v) = o\n  puts v\nend\n",
        // a whole-value binding arm, then a read.
        "k = 3\ns = \"s-#{k}\"\nmatch s\n  x -> puts x\nend\nputs s\n",
    ] {
        assert_eq!(borrow_codes(&program(body)), vec!["E1001"], "{body}");
    }
}

/// One binding use compiles, and so do patterns that move nothing: a Copy
/// payload, a `_` or literal pattern, and a temporary scrutinee.
#[test]
fn semantics_patterns_that_move_nothing_leave_the_scrutinee_usable() {
    for body in [
        "o = mk(7)\nif let Some(v) = o\n  puts v\nend\n",
        "o = mk_int(7)\nif let Some(v) = o\n  puts v\nend\nif let Some(v) = o\n  puts v\nend\n",
        "if let Some(v) = mk(1)\n  puts v\nend\nif let Some(v) = mk(2)\n  puts v\nend\n",
        "k = 3\ns = \"s-#{k}\"\nmatch s\n  \"a\" -> puts \"a\"\n  _ -> puts \"other\"\nend\nputs s\n",
    ] {
        assert_eq!(borrow_codes(&program(body)), Vec::<String>::new(), "{body}");
    }
}

/// A branch that returns its String leaves the function, so the code after
/// the `if` (or `match`) still owns it; a branch that falls through moves it.
#[test]
fn semantics_a_branch_that_leaves_does_not_move_for_the_code_after_it() {
    let shape = |branch: &str| {
        format!(
            "def eat(s: String) -> nil\n  puts s\nend\n\n\
             def pick(k: Int) -> String\n  s = \"s-#{{k}}\"\n  i = 0\n  \
             while i < 3\n    {branch}\n    puts s\n    i += 1\n  end\n  s\nend\n\n\
             puts pick(1)\n"
        )
    };
    for branch in [
        "if i == k\n      return s\n    end",
        "match i\n      1 -> return s\n      _ -> nil\n    end",
    ] {
        assert_eq!(
            borrow_codes(&shape(branch)),
            Vec::<String>::new(),
            "{branch}"
        );
    }
    assert_eq!(
        borrow_codes(&shape("if i == k\n      eat(s)\n    end")),
        vec!["E1001", "E1001"]
    );
}

/// `break` and `next` leave the branch but not the function: the state they
/// carry reaches the code after the loop (I-16; a1, a1b, a1h). Moving `s`
/// before a `break` or `next`, then reading it after the loop, is E1001, in
/// an `if` and in a `match` arm. Moving it before a `return` still compiles.
#[test]
fn semantics_a_move_before_break_or_next_reaches_the_code_after_the_loop() {
    let shape = |branch: &str| {
        format!(
            "def eat(s: String) -> nil\n  puts s\nend\n\n\
             k = 3\ns = \"hi-#{{k}}\"\ni = 0\nwhile i < 5\n  i = i + 1\n  {branch}\nend\nputs s\n"
        )
    };
    for branch in [
        // a1: a move into a by-value call, then `break`.
        "if i == 2\n    eat(s)\n    break\n  end",
        // a move then `continue`.
        "if i == 2\n    eat(s)\n    continue\n  end",
        // the match-arm path.
        "match i\n    2 -> do\n      eat(s)\n      break\n    end\n    _ -> nil\n  end",
    ] {
        assert_eq!(borrow_codes(&shape(branch)), vec!["E1001"], "{branch}");
    }
    // a1b: the move stores `s` in an object that frees it.
    let a1b = "class Box\n  s: String\n\n  def init(@s: String)\n  end\nend\n\n\
               k = 3\ns = \"hi-#{k}\"\ni = 0\nwhile i < 5\n  i = i + 1\n  \
               if i == 2\n    b = Box.new(s)\n    puts b.s\n    break\n  end\nend\nputs s\n";
    assert_eq!(borrow_codes(a1b), vec!["E1001"]);
    // Moving before `return` leaves the function: still fine.
    let returns = "def eat(s: String) -> nil\n  puts s\nend\n\n\
                   def run(k: Int) -> nil\n  s = \"hi-#{k}\"\n  i = 0\n  while i < 5\n    \
                   i = i + 1\n    if i == 2\n      eat(s)\n      return nil\n    end\n  end\n  puts s\nend\n\nrun(3)\n";
    assert_eq!(borrow_codes(returns), Vec::<String>::new());
}
