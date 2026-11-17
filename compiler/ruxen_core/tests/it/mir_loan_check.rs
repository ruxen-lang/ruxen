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

def pick[l](c: Bool, a: &l A, b: &l A) -> &l A
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
        // I-77: a fresh runtime result holding references (`keys`, `to_a`)
        // borrows its table; `clear` / `remove` free the keys it points at.
        (
            "table_keys_across_clear",
            "h = {u.name.clone => 1}\n  ks = h.keys\n  h.clear\n  puts(ks[0])",
            &["E1002"],
        ),
        (
            "table_entries_across_remove",
            "h = {u.name.clone => 1}\n  es = h.to_a\n  h.remove(&u.name)\n  puts(es.size)",
            &["E1002"],
        ),
        (
            "set_view_across_clear",
            "s = [u.name.clone].to_set\n  vs = s.to_a\n  s.clear\n  puts(vs[0])",
            &["E1002"],
        ),
        // Guard: the view's last use is before the table is changed.
        (
            "guard_table_keys_then_clear",
            "h = {u.name.clone => 1}\n  ks = h.keys\n  puts(ks[0])\n  h.clear",
            &[],
        ),
        (
            "guard_set_view_then_remove",
            "s = [u.name.clone].to_set\n  vs = s.to_a\n  puts(vs.size)\n  s.remove(&u.name)\n  puts(s.size)",
            &[],
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

/// A call result's borrow comes from the callee's declared signature by
/// the one elision rule (`hir::types::result_source`): a result holding a
/// reference borrows from the receiver, else the reference parameters, and
/// the loan is on what the argument denotes, owned or reached through a
/// reference. A declared result with no reference (`remove -> Option[V]`)
/// borrows nothing and keeps only the loans its arguments held.
#[test]
fn elided_result_borrows() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "view_through_ref_param",
            "def wipe(h: &var Hash[String, Int]) -> Int\n  ks = h.keys\n  h.clear\n  puts(ks[0])\n  0\nend\n\nh = {\"alpha\".clone => 1}\nwipe(&var h)\n",
            &["E1002"],
        ),
        (
            "view_through_field",
            "class Reg\n  t: Hash[String, Int]\n\n  def init\n    @t = {\"alpha\".clone => 1}\n  end\n\n  def var wipe -> Int\n    ks = @t.keys\n    @t.clear\n    puts(ks[0])\n    0\n  end\nend\n\nr = Reg.new\nr.wipe\n",
            &["E1002"],
        ),
        (
            "view_through_ref_local",
            "h = {\"alpha\".clone => 1}\nr = &var h\nks = r.keys\nr.clear\nputs(ks[0])\n",
            &["E1002"],
        ),
        (
            "view_from_a_wrapper",
            "def key_view(h: &Hash[String, Int]) -> Array[&String]\n  h.keys\nend\n\nh = {\"alpha\".clone => 1}\nks = key_view(&h)\nh.clear\nputs(ks[0])\n",
            &["E1002"],
        ),
        (
            "view_from_a_wrapper_of_a_wrapper",
            "def key_view(h: &Hash[String, Int]) -> Array[&String]\n  h.keys\nend\n\ndef outer(h: &Hash[String, Int]) -> Array[&String]\n  key_view(h)\nend\n\nh = {\"alpha\".clone => 1}\nks = outer(&h)\nh.clear\nputs(ks[0])\n",
            &["E1002"],
        ),
        (
            "guard_view_used_before_clear_through_ref",
            "def wipe(h: &var Hash[String, Int]) -> Int\n  ks = h.keys\n  puts(ks[0])\n  h.clear\n  0\nend\n\nh = {\"alpha\".clone => 1}\nwipe(&var h)\n",
            &[],
        ),
        (
            "removed_value_outlives_clear",
            "s = \"kept\"\nh = {1 => &s}\nr = h.remove(&1)\nh.clear\nputs(r.unwrap)\n",
            &[],
        ),
        // A named lifetime (`def longest[l](x: &l String, y: &l String)`)
        // borrows every parameter that carries it.
        (
            "named_lifetime_borrows_each_named_param",
            "def longest[l](x: &l String, y: &l String) -> &l String\n  if x.size > y.size\n    x\n  else\n    y\n  end\nend\n\na = \"hello\".clone\nb = \"hi\".clone\nr = longest(&a, &b)\nb.push_str(\"!\")\nputs(r)\n",
            &["E1002"],
        ),
        (
            "removed_value_keeps_its_loan",
            "s = \"kept\".clone\nh = {1 => &s}\nr = h.remove(&1)\ns.push_str(\"!\")\nputs(r.unwrap)\n",
            &["E1002"],
        ),
        // The F-2 provenance rows (review round 3): what a call result
        // points into (`borrows`) and whose references it carries
        // (`carries`), each rejected form next to its form accepted after
        // the last use. The same programs run under Guard Malloc in
        // `tmp/f2-rethink/check-probes.sh`.
        (
            "t1_take_rebind",
            r#"a = ["alpha".clone, "beta".clone]
t = a.take(1)
a = ["x".clone]
puts(t[0])
puts(a[0])
"#,
            &["E1009"],
        ),
        (
            "t_clone",
            r#"a = ["alpha".clone, "beta".clone]
t = a.clone
a = ["x".clone]
puts(t[0])
puts(a[0])
"#,
            &["E1009"],
        ),
        (
            "t_reverse",
            r#"a = ["alpha".clone, "beta".clone]
t = a.reverse
a = ["x".clone]
puts(t[0])
puts(a[0])
"#,
            &["E1009"],
        ),
        // `j` is a view of the parse result's temporary (`unwrap` is a C
        // value result that is not provably new, so it points into its
        // receiver) and `c` borrows through `j`: rebinding the view `j`
        // writes no storage (as rebinding a `&Json` in Rust), and the
        // temporary lives while `c` is used.
        (
            "json_get",
            r#"j = JSON.parse(&"[1, 2]".clone).unwrap
c = JSON.array_get(&j, 0).unwrap
j = JSON.parse(&"[3]".clone).unwrap
puts(JSON.as_int(&c).unwrap)
"#,
            &[],
        ),
        (
            "c3_method_view",
            r#"class Viewer
  n: Int

  def init(@n: Int)
  end

  def key_view(h: &Hash[String, Int]) -> Array[&String]
    h.keys
  end
end

v = Viewer.new(1)
h = {"alpha".clone => 1}
ks = v.key_view(&h)
h.clear
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "method_to_method",
            r#"class Viewer
  n: Int

  def init(@n: Int)
  end

  def key_view(h: &Hash[String, Int]) -> Array[&String]
    h.keys
  end

  def outer(h: &Hash[String, Int]) -> Array[&String]
    self.key_view(h)
  end
end

v = Viewer.new(1)
h = {"alpha".clone => 1}
ks = v.outer(&h)
h.clear
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "method_to_free_fn",
            r#"def key_view(h: &Hash[String, Int]) -> Array[&String]
  h.keys
end

class Viewer
  n: Int

  def init(@n: Int)
  end

  def outer(h: &Hash[String, Int]) -> Array[&String]
    key_view(h)
  end
end

v = Viewer.new(1)
h = {"alpha".clone => 1}
ks = v.outer(&h)
h.clear
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "mixin_method_view",
            r#"mixin Keyed
  def key_view(h: &Hash[String, Int]) -> Array[&String]
    h.keys
  end
end

class Viewer
  include Keyed
  n: Int

  def init(@n: Int)
  end
end

v = Viewer.new(1)
h = {"alpha".clone => 1}
ks = v.key_view(&h)
h.clear
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "c1_remove_key",
            r#"value = "payload".clone
key = "key".clone
h = {"key".clone => &value}
result = h.remove(&key)
key = "changed".clone
puts(result.unwrap)
puts(key)
"#,
            &[],
        ),
        (
            "fp1_get_key",
            r#"h = {"a".clone => "val".clone}
key = "a".clone
v = h.get(&key)
key = "zzz".clone
puts(v.unwrap)
puts(key)
"#,
            &[],
        ),
        (
            "user_key_get",
            r#"class Key
  id: Int

  def init(@id: Int)
  end
end

h = {Key.new(1) => "val".clone}
k = Key.new(1)
v = h.get(&k)
k = Key.new(2)
puts(v.unwrap)
r = h.remove(&k)
puts(r.nil?)
"#,
            &[],
        ),
        (
            "h1_remove_heldloan",
            r#"s = "kept".clone
h = {1 => &s}
r = h.remove(&1)
s = "other".clone
puts(r.unwrap)
"#,
            &["E1009"],
        ),
        (
            "h2_remove_drop_owner",
            r#"def make(h: &var Hash[Int, &String]) -> Option[&String]
  h.remove(&1)
end

s = "kept".clone
h = {1 => &s}
r = make(&var h)
s = "other".clone
puts(r.unwrap)
"#,
            &["E1009"],
        ),
        (
            "carries_only_wrapper",
            r#"def take_one(h: Hash[Int, &String]) -> Option[&String]
  h.remove(&1)
end

s = "kept".clone
t = {1 => &s}
r = take_one(t)
s = "other".clone
puts(r.unwrap)
"#,
            &["E1009"],
        ),
        (
            "sf_tmp",
            r#"class Bag
  ks: Array[&String]

  def init(@ks: Array[&String])
  end

  def first -> &String
    @ks[0]
  end
end

b = Bag.new({"alpha".clone => 1}.keys)
puts(b.first)
"#,
            &[],
        ),
        (
            "field_ref_method",
            r#"class Holder
  r: &String

  def init(@r: &String)
  end

  def get -> &String
    @r
  end
end

s = "kept".clone
hd = Holder.new(&s)
g = hd.get
s = "other".clone
puts(g)
"#,
            &["E1009"],
        ),
        // A `->` closure moves `s` into its captures, so reassigning `s`
        // is sound.
        (
            "closure_returns_class",
            r#"class Holder
  r: &String

  def init(@r: &String)
  end

  def get -> &String
    @r
  end
end

s = "kept".clone
f = -> { Holder.new(&s) }
hd = f.call
s = "other".clone
puts(hd.get)
"#,
            &[],
        ),
        (
            "fn_payload_view",
            r#"def first_val(h: &Hash[String, String]) -> &String
  h.get(&"a".clone).unwrap
end

h = {"a".clone => "val".clone}
v = first_val(&h)
h.clear
puts(v)
"#,
            &["E1002"],
        ),
        (
            "fn_param_alias",
            r#"def key_view(h: &Hash[String, Int]) -> Array[&String]
  x = h
  x.keys
end

h = {"alpha".clone => 1}
ks = key_view(&h)
h.clear
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "fn_aggregate_store",
            r#"class Bag
  ks: Array[&String]

  def init(@ks: Array[&String])
  end

  def first -> &String
    @ks[0]
  end
end

def bag(h: &Hash[String, Int]) -> Bag
  Bag.new(h.keys)
end

h = {"alpha".clone => 1}
b = bag(&h)
h.clear
puts(b.first)
"#,
            &["E1002"],
        ),
        (
            "fn_recursive",
            r#"def kv(h: &Hash[String, Int], n: Int) -> Array[&String]
  if n > 0
    kv(h, n - 1)
  else
    h.keys
  end
end

h = {"alpha".clone => 1}
ks = kv(&h, 2)
h.clear
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "tmp_payload",
            r#"v = {"a".clone => "val".clone}.get(&"a".clone).unwrap
puts(v)
"#,
            &[],
        ),
        (
            "tmp_struct",
            r#"class Bag
  ks: Array[&String]

  def init(@ks: Array[&String])
  end

  def first -> &String
    @ks[0]
  end
end

def bag(h: &Hash[String, Int]) -> Bag
  Bag.new(h.keys)
end

b = bag(&{"alpha".clone => 1})
puts(b.first)
"#,
            &[],
        ),
        (
            "tmp_closure_call",
            r#"f = ->(h: &Hash[String, Int]) { h.keys }
ks = f.call(&{"alpha".clone => 1})
puts(ks[0])
"#,
            &[],
        ),
        (
            "tmp_copied_param",
            r#"def key_view(h: &Hash[String, Int]) -> Array[&String]
  x = h
  x.keys
end

ks = key_view(&{"alpha".clone => 1})
puts(ks[0])
"#,
            &[],
        ),
        (
            "c4_wrapper_view",
            r#"def key_view(h: &Hash[String, Int]) -> Array[&String]
  h.keys
end

h = {"alpha".clone => 1}
ks = key_view(&h)
h.clear
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "s1_get_clear",
            r#"h = {"alpha".clone => "one".clone}
v = h.get(&"alpha".clone)
h.clear
puts(v.unwrap)
"#,
            &["E1002"],
        ),
        (
            "s2_values_remove",
            r#"h = {"alpha".clone => "one".clone}
vs = h.values
h.remove(&"alpha".clone)
puts(vs[0])
"#,
            &["E1002"],
        ),
        (
            "s3_set_remove",
            r#"s = Set.new
s.insert("alpha".clone)
xs = s.to_a
s.remove(&"alpha".clone)
puts(xs[0])
"#,
            &["E1002"],
        ),
        (
            "s5_method_chain",
            r#"class Reg
  t: Hash[String, Int]

  def init
    @t = {"alpha".clone => 1}
  end

  def table -> &Hash[String, Int]
    @t
  end

  def var wipe
    @t.clear
  end
end

r = Reg.new
ks = r.table.keys
r.wipe
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "s6_generic",
            r#"def wipe[K](h: &var Hash[K, Int]) -> Int
  ks = h.keys
  h.clear
  puts(ks.size)
  0
end

h = {"alpha".clone => 1}
wipe(&var h)
"#,
            &["E1002"],
        ),
        (
            "s9_store_view",
            r#"class Holder
  ks: Array[&String]

  def init(@ks: Array[&String])
  end

  def show -> Int
    puts(@ks[0])
    0
  end
end

h = {"alpha".clone => 1}
hd = Holder.new(h.keys)
h.clear
hd.show
"#,
            &["E1002"],
        ),
        (
            "s11_first_key",
            r#"def first_key(h: &Hash[String, Int]) -> &String
  ks = h.keys
  ks[0]
end

h = {"alpha".clone => 1}
k = first_key(&h)
h.clear
puts(k)
"#,
            &["E1002"],
        ),
        (
            "s12_closure_view",
            r#"h = {"alpha".clone => 1}
ks = h.keys
f = -> { puts(ks[0]) }
h.clear
f.call
"#,
            &["E1002"],
        ),
        (
            "s13_struct_field",
            r#"struct Reg
  t: Hash[String, Int]
end

r = Reg.new({"alpha".clone => 1})
ks = r.t.keys
r.t.clear
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "s14_entries_clear",
            r#"h = {"alpha".clone => "one".clone}
es = h.to_a
h.clear
puts(es[0].0)
"#,
            &["E1002"],
        ),
        (
            "s15_two_args",
            r#"def keys_of(n: Int, h: &Hash[String, Int]) -> Array[&String]
  h.keys
end

h = {"alpha".clone => 1}
ks = keys_of(1, &h)
h.clear
puts(ks[0])
"#,
            &["E1002"],
        ),
        (
            "s16_owned_arg_elems",
            r#"def pick(xs: Array[&String]) -> &String
  xs[0]
end

h = {"alpha".clone => 1}
k = pick(h.keys)
h.clear
puts(k)
"#,
            &["E1002"],
        ),
        (
            "s17_mixed",
            r#"def pick(s: &String, xs: Array[&String]) -> &String
  xs[0]
end

t = "x".clone
h = {"alpha".clone => 1}
k = pick(&t, h.keys)
h.clear
puts(k)
"#,
            &["E1002"],
        ),
        (
            "s18_valid_after",
            r#"h = {"alpha".clone => 1}
ks = h.keys
puts(ks[0])
h.clear
h.insert("beta".clone, 2)
vs = h.values
puts(vs.size)
r = h.remove(&"beta".clone)
puts(r.unwrap)
"#,
            &[],
        ),
        (
            "c3_method_view_after",
            r#"class Viewer
  n: Int

  def init(@n: Int)
  end

  def key_view(h: &Hash[String, Int]) -> Array[&String]
    h.keys
  end
end

v = Viewer.new(1)
h = {"alpha".clone => 1}
ks = v.key_view(&h)
puts(ks[0])
h.clear
puts(h.size)
"#,
            &[],
        ),
        (
            "carries_only_wrapper_after",
            r#"def take_one(h: Hash[Int, &String]) -> Option[&String]
  h.remove(&1)
end

s = "kept".clone
t = {1 => &s}
r = take_one(t)
puts(r.unwrap)
s = "other".clone
puts(s)
"#,
            &[],
        ),
        (
            "clone_after",
            r#"a = ["alpha".clone, "beta".clone]
t = a.clone
puts(t[0])
a = ["x".clone]
puts(a[0])
"#,
            &[],
        ),
        (
            "closure_returns_class_after",
            r#"class Holder
  r: &String

  def init(@r: &String)
  end

  def get -> &String
    @r
  end
end

s = "kept".clone
f = -> { Holder.new(&s) }
hd = f.call
puts(hd.get)
s = "other".clone
puts(s)
"#,
            &[],
        ),
        (
            "field_ref_method_after",
            r#"class Holder
  r: &String

  def init(@r: &String)
  end

  def get -> &String
    @r
  end
end

s = "kept".clone
hd = Holder.new(&s)
g = hd.get
puts(g)
s = "other".clone
puts(s)
"#,
            &[],
        ),
        (
            "fn_aggregate_store_after",
            r#"class Bag
  ks: Array[&String]

  def init(@ks: Array[&String])
  end

  def first -> &String
    @ks[0]
  end
end

def bag(h: &Hash[String, Int]) -> Bag
  Bag.new(h.keys)
end

h = {"alpha".clone => 1}
b = bag(&h)
puts(b.first)
h.clear
puts(h.size)
"#,
            &[],
        ),
        (
            "fn_param_alias_after",
            r#"def key_view(h: &Hash[String, Int]) -> Array[&String]
  x = h
  x.keys
end

h = {"alpha".clone => 1}
ks = key_view(&h)
puts(ks[0])
h.clear
puts(h.size)
"#,
            &[],
        ),
        (
            "fn_payload_view_after",
            r#"def first_val(h: &Hash[String, String]) -> &String
  h.get(&"a".clone).unwrap
end

h = {"a".clone => "val".clone}
v = first_val(&h)
puts(v)
h.clear
puts(h.size)
"#,
            &[],
        ),
        (
            "fn_recursive_after",
            r#"def kv(h: &Hash[String, Int], n: Int) -> Array[&String]
  if n > 0
    kv(h, n - 1)
  else
    h.keys
  end
end

h = {"alpha".clone => 1}
ks = kv(&h, 2)
puts(ks[0])
h.clear
puts(h.size)
"#,
            &[],
        ),
        (
            "json_get_after",
            r#"j = JSON.parse(&"[1, 2]".clone).unwrap
c = JSON.array_get(&j, 0).unwrap
puts(JSON.as_int(&c).unwrap)
j = JSON.parse(&"[3]".clone).unwrap
puts(JSON.array_len(&j).unwrap)
"#,
            &[],
        ),
        (
            "method_to_free_fn_after",
            r#"def key_view(h: &Hash[String, Int]) -> Array[&String]
  h.keys
end

class Viewer
  n: Int

  def init(@n: Int)
  end

  def outer(h: &Hash[String, Int]) -> Array[&String]
    key_view(h)
  end
end

v = Viewer.new(1)
h = {"alpha".clone => 1}
ks = v.outer(&h)
puts(ks[0])
h.clear
puts(h.size)
"#,
            &[],
        ),
        (
            "method_to_method_after",
            r#"class Viewer
  n: Int

  def init(@n: Int)
  end

  def key_view(h: &Hash[String, Int]) -> Array[&String]
    h.keys
  end

  def outer(h: &Hash[String, Int]) -> Array[&String]
    self.key_view(h)
  end
end

v = Viewer.new(1)
h = {"alpha".clone => 1}
ks = v.outer(&h)
puts(ks[0])
h.clear
puts(h.size)
"#,
            &[],
        ),
        (
            "mixin_method_view_after",
            r#"mixin Keyed
  def key_view(h: &Hash[String, Int]) -> Array[&String]
    h.keys
  end
end

class Viewer
  include Keyed
  n: Int

  def init(@n: Int)
  end
end

v = Viewer.new(1)
h = {"alpha".clone => 1}
ks = v.key_view(&h)
puts(ks[0])
h.clear
puts(h.size)
"#,
            &[],
        ),
        (
            "reverse_after",
            r#"a = ["alpha".clone, "beta".clone]
t = a.reverse
puts(t[0])
a = ["x".clone]
puts(a[0])
"#,
            &[],
        ),
        (
            "take_after",
            r#"a = ["alpha".clone, "beta".clone]
t = a.take(1)
puts(t[0])
a = ["x".clone]
puts(a[0])
"#,
            &[],
        ),
        // I-77: a by-reference parameter stored in or returned from a
        // result is borrowed by it (the caller's `&x` loan stays), while
        // what the parameter only stores is carried (`rm` keeps the
        // value's loan, not the key's).
        (
            "q9_holder_wrap",
            r#"class Holder
  s: &String

  def init(@s: &String)
  end

  def get -> &String
    @s
  end
end

def wrap(s: &String) -> Holder
  Holder.new(s)
end

x = "alpha".clone
w = wrap(&x)
x = "beta".clone
puts(w.get)
puts(x)
"#,
            &["E1009"],
        ),
        (
            "q9_holder_wrap_after",
            r#"class Holder
  s: &String

  def init(@s: &String)
  end

  def get -> &String
    @s
  end
end

def wrap(s: &String) -> Holder
  Holder.new(s)
end

x = "alpha".clone
w = wrap(&x)
puts(w.get)
x = "beta".clone
puts(x)
"#,
            &[],
        ),
        (
            "q11_method_wrap",
            r#"class Holder
  s: &String

  def init(@s: &String)
  end

  def get -> &String
    @s
  end
end

class Maker
  n: Int

  def init(@n: Int)
  end

  def make(s: &String) -> Holder
    Holder.new(s)
  end
end

m = Maker.new(1)
x = "alpha".clone
w = m.make(&x)
x = "beta".clone
puts(w.get)
puts(x)
"#,
            &["E1009"],
        ),
        (
            "q11_method_wrap_after",
            r#"class Holder
  s: &String

  def init(@s: &String)
  end

  def get -> &String
    @s
  end
end

class Maker
  n: Int

  def init(@n: Int)
  end

  def make(s: &String) -> Holder
    Holder.new(s)
  end
end

m = Maker.new(1)
x = "alpha".clone
w = m.make(&x)
puts(w.get)
x = "beta".clone
puts(x)
"#,
            &[],
        ),
        (
            "rm_wrapper_key_change",
            r#"def rm(h: &var Hash[String, &String], k: String) -> Option[&String]
  h.remove(k)
end

value = "payload".clone
key = "key".clone
h = {"key".clone => &value}
r = rm(&var h, key.clone)
key = "changed".clone
puts(r.unwrap)
puts(key)
"#,
            &[],
        ),
        (
            "rm_wrapper_value_change",
            r#"def rm(h: &var Hash[String, &String], k: String) -> Option[&String]
  h.remove(k)
end

value = "payload".clone
key = "key".clone
h = {"key".clone => &value}
r = rm(&var h, key.clone)
value = "changed".clone
puts(r.unwrap)
puts(value)
"#,
            &["E1009"],
        ),
    ];
    let set =
        |codes: &[&str]| -> BTreeSet<String> { codes.iter().map(|c| c.to_string()).collect() };
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(name, source, expected)| {
            let codes = loan_codes(source).unwrap_or_else(|| panic!("{name}: does not type-check"));
            (codes != set(expected))
                .then(|| format!("{name}: got {codes:?}, expected {expected:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The body summary fixpoint ends in at most chain depth plus two rounds,
/// on a forwarding chain of three functions and on a recursive pair, and
/// the chain's caller still borrows the table (E1002).
#[test]
fn summary_rounds_are_bounded() {
    let chain = "def c3(h: &Hash[String, Int]) -> Array[&String]\n  h.keys\nend\n\ndef c2(h: &Hash[String, Int]) -> Array[&String]\n  c3(h)\nend\n\ndef c1(h: &Hash[String, Int]) -> Array[&String]\n  c2(h)\nend\n\nh = {\"a\".clone => 1}\nks = c1(&h)\nh.clear\nputs(ks[0])\n";
    let pair = "def f(h: &Hash[String, Int], n: Int) -> Array[&String]\n  if n > 0\n    g(h, n - 1)\n  else\n    h.keys\n  end\nend\n\ndef g(h: &Hash[String, Int], n: Int) -> Array[&String]\n  f(h, n)\nend\n\nh = {\"a\".clone => 1}\nks = g(&h, 3)\nh.clear\nputs(ks[0])\n";
    for (name, source) in [("chain", chain), ("pair", pair)] {
        let codes = loan_codes(source).expect("type-checks");
        assert_eq!(codes, BTreeSet::from(["E1002".to_string()]), "{name}");
        // The accepted form lowers to MIR, which records the round count.
        let accepted = source.replace("h.clear\nputs(ks[0])", "puts(ks[0])\nh.clear");
        let frontend = driver::check(
            &parse(&accepted).expect("parse"),
            &driver::Options::default(),
        );
        let rounds = frontend.mir.as_ref().expect("lowered").summary_rounds;
        assert!((1..=5).contains(&rounds), "{name}: {rounds} rounds");
    }
}

/// E0748: a free function returning a reference from several reference
/// parameters has no single source to borrow from (Rust's E0106); Ruby's
/// surface has no lifetime to name one, so it is an error with a help line.
#[test]
fn ambiguous_result_source_is_e0748() {
    let source = "class A\n  name: String\n  def init(@name: String)\n  end\nend\n\ndef pick(c: Bool, a: &A, b: &A) -> &A\n  if c\n    a\n  else\n    b\n  end\nend\n\nx = A.new(\"x\")\ny = A.new(\"y\")\nputs(pick(true, &x, &y).name)\n";
    let frontend = driver::check(&parse(source).expect("parse"), &driver::Options::default());
    let e0748: Vec<_> = frontend
        .diagnostics()
        .iter()
        .filter(|d| d.code.as_deref() == Some("E0748"))
        .collect();
    assert_eq!(e0748.len(), 1, "{:?}", frontend.diagnostics());
    assert!(
        e0748[0].message.contains("(`a`, `b`)"),
        "{}",
        e0748[0].message
    );
    assert!(e0748[0].help[0].contains("method of the value it borrows from"));
    assert!(e0748[0].help[0].contains("def f[a](x: &a T, y: &a T) -> &a T"));
}
