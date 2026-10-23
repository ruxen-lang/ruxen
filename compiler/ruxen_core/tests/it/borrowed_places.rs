//! A borrow of a place keeps the place's root borrowed while the borrow is
//! live (I-15; TODO.md Decisions "Borrow of a temporary"). A place is a
//! variable, a field chain, an index chain, or the result of a method
//! returning `&T`, rooted at a variable or at a temporary. Reading a non-Copy
//! place into a binding borrows its root; assigning to the root or to any
//! field or element of it, `op=`, a parallel-assignment target, a `var self`
//! method on it, and moving it are rejected while the binding is still used
//! later (E1009, E1002 for a `var self` call). After the binding's last use
//! the root is free again (NLL). Every pin runs the real lexer, parser, type
//! checker and borrow checker; the run-time shapes (temporary roots) are
//! pinned in `drop_fixtures` and e2e 2219.

use crate::blocks_procs_lambdas::borrow_codes;

const DEFS: &str = "\
class U
  name: String
  count: Int

  def init(@name: String)
    @count = 0
  end

  def label -> &String
    @name
  end

  def var rename(n: String) -> nil
    @name = n
  end
end

class A
  name: String

  def init(@name: String)
  end
end

class B
  a: A

  def init(@a: A)
  end
end

def pick(u: &U) -> &String
  u.name
end

def eat(u: U) -> nil
  puts u.name
end
";

fn codes(body: &str) -> Vec<String> {
    borrow_codes(&format!("{DEFS}\nk = 3\n{body}"))
}

/// Each review probe that read freed memory is now a compile-time E1009:
/// a3, g4, g5, g6, g9, and the same root reached through a ref-returning
/// free function and through a copy of the borrow.
#[test]
fn semantics_writing_a_borrowed_root_while_the_borrow_is_live_is_e1009() {
    for body in [
        // a3: assign the field the binding reads.
        "u = U.new(\"old-#{k}\")\nx = u.name\nu.name = \"new-#{k}\"\nputs x\n",
        // g9: compound-assign it.
        "u = U.new(\"old-#{k}\")\nx = u.name\nu.name += \"-more\"\nputs x\n",
        // g4: an element's field, then reassign the array.
        "arr = [U.new(\"e-#{k}\")]\nx = arr[0].name\narr = [U.new(\"f-#{k}\")]\nputs x\n",
        // g5: a nested field, then assign the middle field.
        "b = B.new(A.new(\"n-#{k}\"))\nx = b.a.name\nb.a = A.new(\"m-#{k}\")\nputs x\n",
        // g6: a method returning `&String`, then reassign the receiver.
        "u = U.new(\"a-#{k}\")\nx = u.label\nu = U.new(\"b-#{k}\")\nputs x\n",
        // a free function returning a borrow of its `&U` argument.
        "u = U.new(\"p-#{k}\")\nx = pick(u)\nu = U.new(\"q-#{k}\")\nputs x\n",
        // a copy of the borrow keeps the root borrowed too.
        "u = U.new(\"c-#{k}\")\nx = u.name\ny = x\nu.name = \"d-#{k}\"\nputs y\n",
        // a parallel-assignment target.
        "def run(k: Int) -> nil\n  u = U.new(\"m-#{k}\")\n  n = 0\n  x = u.name\n  u, n = U.new(\"z-#{k}\"), 1\n  puts x\n  puts n\nend\nrun(k)\n",
        // moving the root.
        "u = U.new(\"v-#{k}\")\nx = u.name\neat(u)\nputs x\n",
        // a use before the write does not end the borrow if one follows it.
        "u = U.new(\"w-#{k}\")\nx = u.name\nputs x\nu.name = \"y-#{k}\"\nputs x\n",
        // the binding is made in a branch and used after it.
        "u = U.new(\"i-#{k}\")\nw = U.new(\"none\")\nx = w.name\nif k > 1\n  x = u.name\nend\nu.name = \"j-#{k}\"\nputs x\n",
    ] {
        assert_eq!(codes(body), vec!["E1009"], "{body}");
    }
}

/// A `var self` method on a borrowed root is a mutable borrow while a
/// shared one is live (E1002). `push_str` on a borrowed field chain
/// assigns the field (E1009).
#[test]
fn semantics_a_var_self_call_on_a_borrowed_root_is_e1002() {
    let rename = "u = U.new(\"r-#{k}\")\nx = u.name\nu.rename(\"s-#{k}\")\nputs x\n";
    assert_eq!(codes(rename), vec!["E1002"]);
    let push_str = "b = B.new(A.new(\"n-#{k}\"))\nx = b.a.name\nb.a.name.push_str(\"!\")\nputs x\n";
    assert_eq!(codes(push_str), vec!["E1009"]);
}

/// What stays legal: a write after the binding's last use (NLL, also inside
/// a loop body), a `.clone` read, a scalar read, a temporary root, a borrow
/// read into an argument only, and rebinding the borrowing variable itself.
#[test]
fn semantics_writes_after_the_last_use_and_owned_reads_compile() {
    for body in [
        "u = U.new(\"a-#{k}\")\nx = u.name\nputs x\nu.name = \"b-#{k}\"\nputs u.name\n",
        "u = U.new(\"a-#{k}\")\nx = u.name.clone\nu.name = \"b-#{k}\"\nputs x\n",
        "u = U.new(\"a-#{k}\")\nn = u.count\nu.name = \"b-#{k}\"\nputs n\n",
        "u = U.new(\"a-#{k}\")\nputs u.name\nu.name = \"b-#{k}\"\nputs u.name\n",
        "n = U.new(\"t-#{k}\").name\nputs n\n",
        "x = pick(U.new(\"p-#{k}\"))\nputs x\n",
        "u = U.new(\"a-#{k}\")\nw = U.new(\"w-#{k}\")\nx = u.name\nputs x\nx = w.name\nu.name = \"b-#{k}\"\nputs x\n",
        "u = U.new(\"a-#{k}\")\ni = 0\nwhile i < 2\n  x = u.name\n  puts x\n  u.name = \"n-#{i}\"\n  i += 1\nend\n",
        "u = U.new(\"a-#{k}\")\nx = u.label\nputs x\nu = U.new(\"b-#{k}\")\nputs u.name\n",
    ] {
        assert_eq!(codes(body), Vec::<String>::new(), "{body}");
    }
}

/// A binding used again in a later iteration keeps its root borrowed across
/// the loop's back edge: the write in the loop body is before that use.
#[test]
fn semantics_a_borrow_used_in_a_later_iteration_spans_the_loop() {
    let body = "u = U.new(\"a-#{k}\")\nx = u.name\ni = 0\nwhile i < 2\n  puts x\n  u.name = \"n-#{i}\"\n  i += 1\nend\n";
    assert_eq!(codes(body), vec!["E1009"]);
}

/// One provenance rule (I-15, F-1): a binding borrows from every place its
/// value may come from, whatever the shape that yields it: an `if`, `match`
/// or `case` arm, a block tail, a cast, a parallel assignment's value, a
/// closure that captures a borrowing binding (the loan lives as long as the
/// closure), a pattern binding out of a borrowed place, and a `for` loop
/// over one. Review probes q8, q13, q1h and c1g.
#[test]
fn semantics_every_value_shape_that_yields_a_place_borrows_its_root() {
    for body in [
        // q8: an `if` whose branches read the place.
        "u = U.new(\"old-#{k}\")\nx = if k > 1\n  u.name\nelse\n  u.label\nend\nu.name = \"n\"\nputs x\n",
        // q13: a `match` whose arms read it.
        "u = U.new(\"old-#{k}\")\nx = match k\n  3 -> u.name\n  _ -> u.label\nend\nu.name = \"n\"\nputs x\n",
        // a `case/when`.
        "u = U.new(\"old-#{k}\")\nx = case k\nwhen 3 then u.name\nelse u.label\nend\nu.name = \"n\"\nputs x\n",
        // a block tail after statements, and a cast.
        "u = U.new(\"old-#{k}\")\nx = if k > 1\n  puts \"in\"\n  u.name\nelse\n  u.label as &String\nend\nu.name = \"n\"\nputs x\n",
        // q1h: a lambda capturing the borrow runs after the write.
        "def run(k: Int) -> nil\n  u = U.new(\"old-#{k}\")\n  x = u.name\n  f = -> { puts(x) }\n  u.name = \"n-#{k}\"\n  f.()\nend\nrun(k)\n",
        // c1g: a parallel assignment's value.
        "def run(k: Int) -> nil\n  u = U.new(\"old-#{k}\")\n  v = U.new(\"v-#{k}\")\n  x = v.name\n  y = 0\n  x, y = u.name, 1\n  u.name = \"n-#{k}\"\n  puts(x)\n  puts(y)\nend\nrun(k)\n",
        // an element read through an alias whose last use is that read.
        "class L\n  names: Array[String]\n  def init(@names: Array[String])\n  end\nend\nl = L.new([\"a-#{k}\"])\nns = l.names\nx = ns[0]\nl.names = [\"c\"]\nputs x\n",
        // a `for` loop over a borrowed collection field.
        "class L\n  names: Array[String]\n  def init(@names: Array[String])\n  end\nend\nl = L.new([\"a-#{k}\", \"b\"])\nfor n in l.names\n  l.names = [\"c\"]\n  puts n\nend\n",
    ] {
        assert_eq!(codes(body), vec!["E1009"], "{body}");
    }
}

/// Every conflicting use goes through the same roots: a `&var` argument,
/// explicit or auto-borrowed, of a borrowed root (c1a, c1b, c1f) is a
/// mutable borrow while a shared one is live (E1002); a write through an
/// alias of a sub-object writes the root the alias borrows (c1c, E1009).
#[test]
fn semantics_var_arguments_and_alias_writes_check_the_borrowed_root() {
    let replace = "def replace(s: &var String) -> nil\n  s = \"new-value\"\nend\n";
    for body in [
        format!("{replace}u = U.new(\"old-#{{k}}\")\nx = u.name\nreplace(u.name)\nputs x\n"),
        format!("{replace}u = U.new(\"old-#{{k}}\")\nx = u.name\nreplace(&var u.name)\nputs x\n"),
        format!("{replace}def run(k: Int) -> nil\n  u = U.new(\"old-#{{k}}\")\n  x = u.name\n  replace(&var u.name)\n  puts(x)\nend\nrun(k)\n"),
    ] {
        assert_eq!(codes(&body), vec!["E1002"], "{body}");
    }
    let alias =
        "u = B.new(A.new(\"old-#{k}\"))\nx = u.a.name\nb = u.a\nb.name = \"n-#{k}\"\nputs x\n";
    assert_eq!(codes(alias), vec!["E1009"]);
}

/// A borrowing binding that is never used holds nothing (I-25, probe q9c),
/// and the matching "borrow dead before the write" shapes compile.
#[test]
fn semantics_dead_borrows_never_block_a_write() {
    for body in [
        "u = U.new(\"old-#{k}\")\nx = u.name\nu.name = \"n\"\nputs u.name\n",
        "u = U.new(\"old-#{k}\")\nx = if k > 1\n  u.name\nelse\n  u.label\nend\nputs x\nu.name = \"n\"\nputs u.name\n",
        "def run(k: Int) -> nil\n  u = U.new(\"old-#{k}\")\n  x = u.name\n  f = -> { puts(x) }\n  f.()\n  u.name = \"n-#{k}\"\n  puts(u.name)\nend\nrun(k)\n",
        "u = B.new(A.new(\"old-#{k}\"))\nb = u.a\nb.name = \"n-#{k}\"\nputs b.name\nputs u.a.name\n",
    ] {
        assert_eq!(codes(body), Vec::<String>::new(), "{body}");
    }
}

/// A `&var` place argument is a mutable loan of its place for the call
/// (I-24): the receiver or any other argument viewing that place is E1002.
/// Review probes v1, v2b, v8, v9 and v11.
#[test]
fn semantics_a_var_place_argument_excludes_its_root_from_the_rest_of_the_call() {
    let h = "def h(s: &var String, w: &var U)\n  w.name = \"via-u\"\n  s = \"via-s\"\nend\n\
             def hr(s: &var String, w: &U)\n  s = \"via-s\"\n  puts w.name\nend\n\
             def hv(s: &var String, w: &String)\n  s = \"via-s\"\n  puts w\nend\n\
             def g(s: &var String, a: &var Array[String])\n  a.push(\"pad\")\n  s = \"new-value\"\nend\n\
             class V\n  name: String\n  def init(@name: String)\n  end\n  def bump(s: &var String) -> nil\n    s = \"new-#{@name.size}\"\n    puts(@name)\n  end\nend\n";
    for body in [
        format!("{h}a = [\"old-#{{k}}\"]\ng(&var a[0], &var a)\nputs a[0]\n"),
        format!("{h}u = U.new(\"old-#{{k}}\")\nh(&var u.name, &var u)\nputs u.name\n"),
        format!("{h}u = U.new(\"old-#{{k}}\")\nhv(&var u.name, u.name)\n"),
    ] {
        assert_eq!(codes(&body), vec!["E1002"], "{body}");
    }
    // Distinct roots stay legal, and so does the object's handle (an
    // argument or the receiver) beside `&var` of its field: the callee reads
    // the field after the write and sees the new value, never a view of the
    // old one (Guard Malloc clean). `self.bump(&var @name)` is v9.
    let v9 = "class W\n  name: String\n  def init(@name: String)\n  end\n  def bump(s: &var String) -> nil\n    s = \"new\"\n    puts(@name)\n  end\n  def go -> nil\n    self.bump(&var @name)\n  end\nend\nW.new(\"old-#{k}\").go\n";
    for ok in [
        v9.to_string(),
        format!("{h}v = V.new(\"old-#{{k}}\")\nv.bump(&var v.name)\nputs v.name\n"),
        format!("{h}v = V.new(\"old-#{{k}}\")\nv.bump(v.name)\nputs v.name\n"),
        format!(
            "{h}u = U.new(\"old-#{{k}}\")\nw = U.new(\"w\")\nh(&var u.name, &var w)\nputs u.name\n"
        ),
        format!("{h}u = U.new(\"old-#{{k}}\")\nhr(&var u.name, &u)\n"),
    ] {
        assert_eq!(codes(&ok), Vec::<String>::new(), "{ok}");
    }
}

/// A `&var` argument for a String or scalar parameter is passed by address:
/// an element of anything but an Array (a Hash value) has no address to
/// pass, so it is E1021, explicit or auto-borrowed (I-27; it used to be an
/// uncoded MIR error, and before that the write was silently lost).
#[test]
fn semantics_a_hash_value_cannot_be_a_var_argument() {
    let replace = "def replace(s: &var String) -> nil\n  s = \"new-value\"\nend\n";
    for call in ["replace(&var h[\"a\"])", "replace(h[\"a\"])"] {
        let body = format!("{replace}h = {{\"a\" => \"x-#{{k}}\"}}\n{call}\nputs h[\"a\"]\n");
        assert_eq!(codes(&body), vec!["E1021"], "{body}");
    }
}

/// One provenance query for every value that may hold a reference (I-29):
/// an aggregate built from a borrow keeps its root borrowed while the
/// aggregate is live, whatever the shape: an Array, a tuple, a Hash, an
/// enum constructor argument, and a closure (a lambda capturing a borrowing
/// binding reaches what that binding borrows) put into one. Review probes
/// b14e, c2, b19, c6, c7, a14, b14 and b14c each read freed memory.
#[test]
fn semantics_an_aggregate_holding_a_borrow_keeps_its_root_borrowed() {
    let run = |body: &str| {
        format!("def run(k: Int) -> nil\n  u = U.new(\"old-#{{k}}\")\n{body}end\nrun(k)\n")
    };
    for body in [
        "  x = u.name\n  arr = [x]\n  u.name = \"n\"\n  puts(arr[0])\n",
        "  arr = [u.name]\n  u.name = \"n\"\n  puts(arr[0])\n",
        "  t = (u.name, 1)\n  u.name = \"n\"\n  puts(t.0)\n",
        "  o = Some(u.name)\n  u.name = \"n\"\n  puts(o.unwrap)\n",
        "  h = {\"a\" => u.name}\n  u.name = \"n\"\n  puts(h[\"a\"])\n",
        "  x = u.name\n  arr = [-> { puts(x) }]\n  u.name = \"n\"\n  arr[0].()\n",
        "  x = u.name\n  o = Some(-> { puts(x) })\n  u.name = \"n\"\n  o.unwrap.()\n",
        "  x = u.name\n  f = -> { puts(x) }\n  arr = [f]\n  u.name = \"n\"\n  arr[0].()\n",
    ] {
        assert_eq!(codes(&run(body)), vec!["E1009"], "{body}");
    }
    // The same aggregates with the write after their last use compile
    // (c1, c3, c5), and so does a Copy element read from a place.
    for body in [
        "  x = u.name\n  arr = [x]\n  puts(arr[0])\n  puts(u.name)\n  u.name = \"n\"\n",
        "  arr = [u.name]\n  puts(arr[0])\n  u.name = \"n\"\n  puts(u.name)\n",
        "  t = (u.name, 1)\n  puts(t.0)\n  u.name = \"n\"\n  puts(u.name)\n",
        "  n = [u.name.size]\n  u.name = \"n\"\n  puts(n[0])\n",
    ] {
        assert_eq!(codes(&run(body)), Vec::<String>::new(), "{body}");
    }
}

const OPERANDS: &str = "\
def reset(u: &var U) -> Int
  u.name = \"reset-a-long-enough-string-to-realloc\"
  1
end
def resets(u: &var U) -> String
  u.name = \"reset-a-long-enough-string-to-realloc\"
  \"r\"
end
def replace(s: &var String, n: Int) -> nil
  s = \"replaced-#{n}\"
end
def show(s: &String, n: Int) -> nil
  puts(s)
end
def h(s: &var String, &block: Fn[() -> nil]) -> nil
  s = \"new-value-padding-padding-padding\"
  yield
end
class V
  def init
  end
  def var show(s: &String, n: Int) -> nil
    puts(s)
  end
end
class P
  name: String
  a: A
  def init(@name: String, @a: A)
  end
end
def both(s: &var String, t: &var String) -> nil
  s = \"s\"
  t = \"t\"
end
";

/// An operand already evaluated is protected while the later operands of
/// the same call run (I-30): each operand that may point into a place holds
/// a temporary loan of it until the call, so a later operand that writes the
/// place is E1002 (k1, k1b, k2, k3, k9: a free function, a method and a
/// binary operator). A free function's trailing block reaches each capture
/// it borrows, like a method's block (I-31: e5, d3).
#[test]
fn semantics_an_evaluated_operand_is_protected_while_later_operands_run() {
    let run = |body: &str| {
        format!(
            "{OPERANDS}def run(k: Int) -> nil\n  u = U.new(\"old-#{{k}}\")\n{body}end\nrun(k)\n"
        )
    };
    for body in [
        "  replace(&var u.name, reset(&var u))\n  puts(u.name)\n",
        "  replace(&var u.name, reset(u))\n  puts(u.name)\n",
        "  show(u.name, reset(u))\n  puts(u.name)\n",
        "  v = V.new\n  v.show(u.name, reset(u))\n  puts(u.name)\n",
        "  s = u.name + resets(u)\n  puts(s)\n",
        "  s = \"old-#{k}\"\n  h(&var s) { puts(s) }\n  puts(s)\n",
        "  h(&var u.name) { puts(u.name) }\n  puts(u.name)\n",
    ] {
        assert_eq!(codes(&run(body)), vec!["E1002"], "{body}");
    }
    // What stays legal: a Copy operand read before the write (k10), an
    // interpolation (each part is formatted when it is evaluated, k8),
    // disjoint fields (d2 and `both`), and a block that reaches nothing the
    // `&var` argument names.
    for body in [
        "  n = [u.name.size, reset(u)]\n  puts(n[0])\n",
        "  s = \"#{u.name}-#{resets(u)}\"\n  puts(s)\n",
        "  w = U.new(\"w\")\n  h(&var u.name) { puts(w.name) }\n  puts(u.name)\n",
        "  p = P.new(\"p-#{k}\", A.new(\"a\"))\n  both(&var p.name, &var p.a.name)\n  puts(p.name)\n",
        "  show(u.name, u.count)\n  replace(&var u.name, u.count)\n  puts(u.name)\n",
    ] {
        assert_eq!(codes(&run(body)), Vec::<String>::new(), "{body}");
    }
}

/// A `r = &var place` binding holds the address of the place (I-24): a
/// write through it (`r = v`) is a use of the binding, not a rebinding, so
/// the place stays borrowed until the last write through `r`; replacing
/// the object that owns the place before then is E1009. After the last
/// use the place is free again.
#[test]
fn semantics_a_var_binding_keeps_its_place_borrowed_through_writes() {
    let run = |body: &str| {
        format!("def run(k: Int) -> nil\n  u = U.new(\"old-#{{k}}\")\n{body}end\nrun(k)\n")
    };
    let conflict = "  r = &var u.name\n  u = U.new(\"z\")\n  r = \"w\"\n  puts(u.name)\n";
    assert_eq!(codes(&run(conflict)), vec!["E1009"], "{conflict}");
    for body in [
        "  r = &var u.name\n  r = \"w\"\n  u = U.new(\"z\")\n  puts(u.name)\n",
        "  c = &var u.count\n  c = 9\n  u.count = 3\n  puts(u.count)\n",
    ] {
        assert_eq!(codes(&run(body)), Vec::<String>::new(), "{body}");
    }
}

/// A `var self` receiver is a mutable loan of its place for the call, like
/// a `&var` argument (I-24 sibling): another operand that points into the
/// same storage (`u.rename_with(u.name)`, where the method replaces
/// `@name` and then reads its argument) is E1002, and so is any other field
/// of the receiver (the method may replace any of them). A field receiver
/// and an argument on a disjoint field, and owned copies, stay legal.
#[test]
fn semantics_a_var_self_receiver_excludes_its_place_from_the_arguments() {
    let defs = "class W\n  name: String\n  tag: String\n  def init(@name: String, @tag: String)\n  end\n  def var rename_with(n: &String) -> nil\n    @name = \"replaced\"\n    puts(n)\n  end\nend\n";
    let run = |body: &str| {
        format!(
            "{defs}def run(k: Int) -> nil\n  w = W.new(\"old-#{{k}}\", \"t\")\n{body}end\nrun(k)\n"
        )
    };
    for body in ["  w.rename_with(w.name)\n", "  w.rename_with(w.tag)\n"] {
        assert_eq!(codes(&run(body)), vec!["E1002"], "{body}");
    }
    for body in [
        "  w.name.push_str(w.tag)\n  puts(w.name)\n",
        "  w.rename_with(w.name.clone)\n",
        "  v = W.new(\"v\", \"t\")\n  w.rename_with(v.name)\n",
    ] {
        assert_eq!(codes(&run(body)), Vec::<String>::new(), "{body}");
    }
}
