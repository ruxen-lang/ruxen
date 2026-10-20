//! The MIR loan check (`mir::loan_check`, E1002 and E1009) through the
//! front end (`driver::check`): the loan matrix (`loan_matrix`, ignored:
//! slow), one case per loan family (`loan_check_families`), and a timing
//! probe (`loan_check_timing`).

use std::collections::BTreeSet;
use std::path::Path;

use ruxen_core::borrow_check::borrow_check;
use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::driver;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::typeck;

use crate::blocks_procs_lambdas::parse;

const LOAN_CODES: [&str; 2] = ["E1002", "E1009"];

/// The loan codes the front end reports for `source`, plus `lowering
/// failed` when MIR lowering fails. `None` when the program does not parse
/// or type-check.
fn loan_codes(source: &str) -> Option<BTreeSet<String>> {
    let frontend = driver::check(&parse(source).ok()?, &driver::Options::default());
    let typeck_failed = frontend
        .diagnostics()
        .iter()
        .any(|d| d.level == DiagnosticLevel::Error);
    if typeck_failed || !frontend.bootstrap.is_empty() {
        return None;
    }
    let mut codes: BTreeSet<String> = frontend
        .borrow_errors
        .iter()
        .map(|e| e.code.code_str())
        .filter(|code| LOAN_CODES.contains(code))
        .map(str::to_string)
        .collect();
    if frontend.lowering_error.is_some() {
        codes.insert("lowering failed".to_string());
    }
    Some(codes)
}

#[test]
#[ignore = "timing probe"]
fn loan_check_timing() {
    let Ok(path) = std::env::var("RUXEN_LOAN_TIMING") else {
        return;
    };
    let source = std::fs::read_to_string(path).expect("read");
    let t = std::time::Instant::now();
    let result = typeck::type_check(&parse(&source).expect("parse"));
    let typeck_ms = t.elapsed().as_millis();
    let t = std::time::Instant::now();
    let _ = borrow_check(&result.program, &result.symbols);
    let hir_ms = t.elapsed().as_millis();
    let t = std::time::Instant::now();
    let _ = Lowerer::new(&result.symbols).lower_program(&result.program);
    let lower_ms = t.elapsed().as_millis();
    let t = std::time::Instant::now();
    let _ = Lowerer::new(&result.symbols).lower_program_checked(&result.program);
    let checked_ms = t.elapsed().as_millis();
    println!("TIMING typeck={typeck_ms} hir={hir_ms} lower={lower_ms} lower+loans={checked_ms}");
}

/// Conflict rows of the loan matrix the loan check accepts, because the
/// write never reaches what the view points into. Each ran in the matrix's
/// memory run (`memrun.py`, Guard Malloc or ASan) with exit 0 and the expected
/// output; 18 of them run clean and the 4 marked
/// `leak` as leaking (Option, tuple and moved-temporary drops, the leak
/// sweep), not as a memory error.
const LOAN_MATRIX_ACCEPTED_CONFLICTS: &[&str] = &[
    // `x` views `u.name` (or an element of `u.items`); the write goes through
    // the alias `al = u.a` to `u.a.name`, a disjoint path.
    "array__alias_write__conflict",
    "arrlam__alias_write__conflict",
    "closure__alias_write__conflict",
    "copy__alias_write__conflict",
    "field__alias_write__conflict",
    "hash__alias_write__conflict",
    "index__alias_write__conflict",
    "lamarr__alias_write__conflict",
    "multi__alias_write__conflict",
    "some__alias_write__conflict",    // leak
    "somelam__alias_write__conflict", // leak
    "tuple__alias_write__conflict",   // leak
    "varbind__alias_write__conflict",
    // `x` views a field of a temporary (`mk(7).name`), which MIR keeps alive
    // for `x`; the write, move or `&var` is of the unrelated `u`.
    "temp__alias_write__conflict",
    "temp__assign_field__conflict",
    "temp__assign_root__conflict",
    "temp__move_call__conflict", // leak
    "temp__multi_target__conflict",
    "temp__opassign__conflict",
    "temp__var_auto__conflict",
    "temp__var_explicit__conflict",
    "temp__var_self__conflict",
];

/// The loan matrix (`tests/fixtures/loan_matrix/`, see its README): every
/// `__conflict` program is rejected, with a loan code or by the type check,
/// unless it is in [`LOAN_MATRIX_ACCEPTED_CONFLICTS`]; every `__dead`
/// program (a `__dead_conflict` row reads a Copy operand first)
/// type-checks with no loan code. Set
/// `RUXEN_LOAN_VERBOSE` to print each program's codes.
#[test]
#[ignore = "slow: checks every loan matrix program"]
fn loan_matrix() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/loan_matrix");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("read loan_matrix")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rx"))
        .collect();
    files.sort();
    let mut wrong = Vec::new();
    let mut accepted = BTreeSet::new();
    for path in &files {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(path).expect("read program");
        let conflict = name.ends_with("__conflict");
        let codes = match loan_codes(&source) {
            Some(codes) => codes,
            // The one conflict the type check rejects: a field borrow
            // returned as an owned `String` (E0744).
            None if name == "field__return__conflict" => continue,
            None => {
                wrong.push(format!("{name}: does not type-check"));
                continue;
            }
        };
        if std::env::var_os("RUXEN_LOAN_VERBOSE").is_some() {
            println!("{name} {codes:?}");
        }
        let listed = LOAN_MATRIX_ACCEPTED_CONFLICTS.contains(&name.as_str());
        match (conflict, codes.is_empty()) {
            (true, true) => {
                accepted.insert(name.clone());
                if !listed {
                    wrong.push(format!("{name}: conflict accepted"));
                }
            }
            (true, false) if listed => wrong.push(format!("{name}: listed but rejected {codes:?}")),
            (false, false) => wrong.push(format!("{name}: dead row rejected {codes:?}")),
            _ => {}
        }
    }
    println!(
        "{} programs, {} accepted conflicts",
        files.len(),
        accepted.len()
    );
    assert!(
        files.len() > 400,
        "loan matrix not found in {}",
        dir.display()
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Classes and free functions every `loan_check_families` case runs against;
/// each case is the body of `run`.
const FAMILY_PRELUDE: &str = r#"
class A
  name: String
  def init(@name: String)
  end
end

class U
  name: String
  a: A
  items: Array[String]
  def init(@name: String, @a: A, @items: Array[String])
  end

  def label -> &String
    @name
  end

  def var rename(n: String) -> nil
    @name = n
  end
end

def mk -> U
  U.new("old", A.new("a"), ["e0", "e1"])
end

class W
  f: Proc[(&String) -> Int]
  r: Array[&String]
  done: Array[Bool]
  c: Int
  def init(@f: Proc[(&String) -> Int], @r: Array[&String], @done: Array[Bool], @c: Int)
  end
end

def mkw -> W
  W.new(->(s: &String) { 1 }, Array.new, [true], 1)
end

def reset(u: &var U) -> Int
  u.name = "reset-a-long-enough-string-to-realloc"
  1
end

def resets(u: &var U) -> String
  u.name = "reset-a-long-enough-string-to-realloc"
  "r"
end

def replace(s: &var String) -> nil
  s = "replaced-value-long-enough-to-need-a-new-buffer"
end

def bump(n: &var Int) -> nil
  n += 1
end

def eat(s: String) -> nil
  puts s
end

def run_block(&block: Fn[() -> nil]) -> nil
  yield
end

def h(s: &var String, &block: Fn[() -> nil]) -> nil
  s = "h-wrote-a-long-enough-string-to-realloc"
  yield
end

def hs(s: &String, &block: Fn[() -> nil]) -> nil
  yield
  puts(s)
end

def pick(c: Bool, a: &A, b: &A) -> &A
  if c
    a
  else
    b
  end
end
"#;

/// Whole programs for the facts the checker reads instead of guessing
/// (name, source, loan codes). Each reject was accepted by the type-based
/// checker and crashed under Guard Malloc; each accept is a false reject a
/// missing fact caused.
const RECORDED_FACT_CASES: &[(&str, &str, &[&str])] = &[
    // A rebind free stays when the callee returns `self` on only some paths.
    (
        "rebind_partial_returns_self",
        "class R\n  n: Int\n  s: String\n  def init\n    self.n = 1\n    self.s = \"b\"\n  end\n  def var maybe(c: Int) -> R\n    if c > 3\n      return self\n    end\n    R.new\n  end\nend\n\ndef run -> R\n  var r: R = R.new\n  r = r.maybe(4)\n  r\nend\n\nputs run().s\n",
        &["E1009"],
    ),
    // ... and is elided when it returns `self` on every path.
    (
        "rebind_returns_self",
        "class R\n  n: Int\n  def init\n    self.n = 1\n  end\n  def var with(c: Int) -> R\n    self.n = c\n    self\n  end\nend\n\ndef run -> R\n  var r: R = R.new\n  r = r.with(4)\n  r\nend\n\nputs run().n\n",
        &[],
    ),
    // `MutexGuard#get` returns a copy, so a callee storing it keeps no
    // view of the guarded value and the later write is safe (it crashed
    // when `get` handed out the mutex's own String).
    (
        "copy_stored_by_callee",
        "use std.sync\n\nclass H\n  s: String\n  def init\n    self.s = \"\"\n  end\n  def var keep(v: String)\n    self.s = v\n  end\nend\n\ndef run\n  m = Mutex.new(\"hello\")\n  g = m.lock!\n  h = H.new\n  h.keep(g.get)\n  g.deref_var.push_str(\" world\")\n  puts h.s\nend\n\nrun()\n",
        &[],
    ),
    // The same copy moved into the object a callee returns.
    (
        "copy_kept_in_result",
        "use std.sync\n\nclass W\n  v: String\n  def init(v: String)\n    self.v = v\n  end\nend\n\ndef wrap(v: String) -> W\n  W.new(v)\nend\n\ndef run\n  m = Mutex.new(\"hello\")\n  g = m.lock!\n  w = wrap(g.get)\n  g.deref_var.push_str(\" world\")\n  puts w.v\nend\n\nrun()\n",
        &[],
    ),
    // The key-kind call before a lookup writes no entry a view points at.
    (
        "lookup_beside_element_view",
        "h = { \"a\" => 1, \"b\" => 2 }\nx = h.get(\"a\").unwrap_or(-1)\ny = h.get(\"b\").unwrap_or(-1)\nputs x + y\n",
        &[],
    ),
    // A dead view does not outlive its last use.
    (
        "guard_deref_after_dead_view",
        "use std.sync\n\ndef bang(s: &var String)\n  s.push_str(\"!\")\nend\n\ndef run\n  m = Mutex.new(\"hello\")\n  g = m.lock!\n  x = g.get\n  puts x\n  bang(g.deref_var)\n  puts g.get\nend\n\nrun()\n",
        &[],
    ),
];

/// One loan family per row, each a miss or a false reject of the MIR
/// checker before its fix, or a shape the HIR checker missed (I-30):
/// (name, body of `run`, loan codes).
#[test]
fn loan_check_families() {
    type Case = (&'static str, &'static str, &'static [&'static str]);
    let cases: &[Case] = &[
        // I-30: an index operand runs after the borrowed array literal (was a crash).
        (
            "index_after_literal",
            "puts([u.name][reset(u)])",
            &["E1002"],
        ),
        (
            "index_after_literal_2",
            "puts([u.name, \"x\"][reset(u)])",
            &["E1002"],
        ),
        // I-30: an index-assignment value runs after its borrowed key (was a crash).
        (
            "index_assign_key",
            "h = {\"k\" => 1}\n  h[u.name] = reset(u)",
            &["E1002"],
        ),
        // I-30: the borrow ends before the later operand runs.
        (
            "index_read_then_add",
            "h = {\"k\" => 1}\n  puts(h[u.name] + reset(u))",
            &[],
        ),
        ("op_assign_value_after", "u.name += resets(u)", &[]),
        // A view inside a call result.
        (
            "call_result_view",
            "x = u.label\n  u.name = \"n\"\n  puts x",
            &["E1009"],
        ),
        // An element view of an owned element type.
        (
            "element_view",
            "its = u.items\n  x = its[0]\n  u.items = [\"n\"]\n  puts x",
            &["E1009"],
        ),
        // An array literal holding a borrow.
        (
            "array_holds_borrow",
            "x = [u.name]\n  u.name = \"n\"\n  puts x[0]",
            &["E1009"],
        ),
        // A Some payload holding a borrow.
        (
            "some_payload",
            "x = Some(u.name)\n  u.name = \"n\"\n  puts x.unwrap",
            &["E1009"],
        ),
        // A closure stored in an array.
        (
            "closure_in_array",
            "y = u.name\n  x = [-> { puts(y) }]\n  u.name = \"n\"\n  x[0].()",
            &["E1009"],
        ),
        // A write through an alias deeper than the loan.
        (
            "alias_deeper_write",
            "x = u.label\n  al = u.a\n  al.name = \"z\"\n  puts x",
            &["E1009"],
        ),
        // A `&var` element slot.
        (
            "var_element_slot",
            "its = u.items\n  x = its[0]\n  replace(its[0])\n  puts x",
            &["E1002"],
        ),
        // `op=` is an assignment.
        (
            "op_assign_is_write",
            "x = u.name\n  u.name += \"-more\"\n  puts x",
            &["E1009"],
        ),
        // A block mutating a capture.
        (
            "block_mutates_capture",
            "y = u.name\n  run_block do\n    replace(&var u.name)\n  end\n  puts y",
            &["E1002"],
        ),
        // A block reading the field a `&var` argument borrows.
        (
            "block_reads_var_arg",
            "h(&var u.name) { puts(u.name) }",
            &["E1002"],
        ),
        // Guard: a field store frees the value it replaces; a callable's
        // `&` parameter is not a reference the field holds.
        (
            "guard_store_replaces_proc_field",
            "w = mkw\n  w.f = ->(s: &String) { 2 }\n  puts w.c",
            &[],
        ),
        // Guard: the replaced `Array[&String]` is moved out, not borrowed.
        (
            "guard_store_replaces_ref_array_field",
            "w = mkw\n  w.r = Array.new\n  puts w.c",
            &[],
        ),
        // Guard: `*v` of a `&Bool` view is a Copy value holding no loan.
        (
            "guard_copy_deref_holds_no_loan",
            "w = mkw\n  cur = match w.done.get(0)\n    Some(v) -> *v\n    nil -> false\n  end\n  nd = [false]\n  nd.push(cur)\n  w.done = [true]\n  puts nd.size",
            &[],
        ),
        // Guard: each element moved into a by-value call is not a loan conflict.
        (
            "guard_for_moves_element",
            "for s in [\"a\", \"b\"]\n    eat(s)\n  end",
            &[],
        ),
        // I-41: a view that is one of several places, after a join or from a
        // call pointing into two arguments; the write reaches each of them.
        (
            "join_alias_write",
            "w = mk\n  x = u.a.name\n  al = if u.items.size > 1\n    u.a\n  else\n    w.a\n  end\n  al.name = \"z\"\n  puts x",
            &["E1009"],
        ),
        (
            "join_alias_var",
            "w = mk\n  x = u.a.name\n  al = if u.items.size > 1\n    u.a\n  else\n    w.a\n  end\n  replace(&var al.name)\n  puts x",
            &["E1002"],
        ),
        (
            "conditional_rebind_write",
            "w = mk\n  x = u.a.name\n  al = w.a\n  if u.items.size > 1\n    al = u.a\n  end\n  al.name = \"z\"\n  puts x",
            &["E1009"],
        ),
        (
            "call_result_of_two_args_write",
            "w = mk\n  x = u.a.name\n  al = pick(true, u.a, w.a)\n  al.name = \"z\"\n  puts x",
            &["E1009"],
        ),
        // I-42: a closure call mutates through its `&var` parameter, or does
        // its body's write to a capture.
        (
            "closure_call_var_arg",
            "f = ->(q: &var U) { q.name = \"z\" }\n  x = u.name\n  f.(&var u)\n  puts x",
            &["E1002"],
        ),
        (
            "closure_call_var_arg_plain",
            "f = ->(q: &var U) { q.name = \"z\" }\n  x = u.name\n  f.(u)\n  puts x",
            &["E1002"],
        ),
        (
            "closure_call_writes_capture",
            "y = u.label\n  f = { u.name = \"z\" }\n  f.()\n  puts y",
            &["E1009"],
        ),
        // Guard: a closure call that only reads its argument.
        (
            "guard_closure_call_reads",
            "f = ->(q: &U) { puts(q.name) }\n  x = u.name\n  f.(u)\n  puts x",
            &[],
        ),
        // Guard: a `&var` param next to a field view of another object.
        (
            "guard_var_param_beside_view",
            "n = 0\n  x = u.name\n  for s in u.items\n    bump(&var n)\n  end\n  puts x",
            &[],
        ),
        // A join of two closures: the writer on either branch is called
        // (was a crash with the writer on the first branch).
        (
            "joined_closures",
            "y = u.label\n  f = if u.items.size > 1\n    ->() { u.name = \"a-long-enough-string-to-force-a-realloc-of-name\" }\n  else\n    ->() { puts \"a\" }\n  end\n  f.()\n  puts y",
            &["E1009"],
        ),
    ];
    let set =
        |codes: &[&str]| -> BTreeSet<String> { codes.iter().map(|c| c.to_string()).collect() };
    let mut wrong = Vec::new();
    for (name, body, expected) in cases {
        let source = format!("{FAMILY_PRELUDE}\ndef run -> nil\n  u = mk\n  {body}\nend\n\nrun\n");
        let codes = loan_codes(&source).unwrap_or_else(|| panic!("{name}: does not type-check"));
        if codes != set(expected) {
            wrong.push(format!("{name}: got {codes:?}, expected {expected:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    for (name, source, expected) in RECORDED_FACT_CASES {
        let codes = loan_codes(source).unwrap_or_else(|| panic!("{name}: does not type-check"));
        assert_eq!(codes, set(expected), "{name}");
    }
    // Guard: an iterator's element moved into a by-value call inside a for loop.
    let lines = include_str!("../fixtures/ruxen/stdin_lines_yields_each_line.rx");
    assert_eq!(loan_codes(lines).expect("fixture type-checks"), set(&[]));
}
