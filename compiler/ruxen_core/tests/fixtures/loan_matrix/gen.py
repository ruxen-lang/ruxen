#!/usr/bin/env python3
"""Generate the loan matrix (see README.md). Every row is a program
`<shape>__<op>__<variant>.rx`: `conflict` uses a view after a write, move
or `&var` of a place it may denote; `dead` does not (its last read comes
first, or it denotes a different place). Some rows also get the output a
clean run prints in `<name>.expect`.

usage: gen.py [DIR]          write every row into DIR (default: this dir)
       gen.py --check [DIR]  exit 1 if DIR's rows differ from the generator

Sections: shape x op rows (round 1), operand order, function blocks,
captures and `var self` receivers (round 3), and round 5's branch merges,
multi-argument call results and direct closure calls."""
import os, sys

# ---------------------------------------------------------------------------
# Round 1: every value shape x every conflicting operation.
# ---------------------------------------------------------------------------


SHAPE_PRELUDE = '''class A
  name: String
  def init(@name: String)
  end
end

class U
  name: String
  count: Int
  a: A
  items: Array[String]
  def init(@name: String, @a: A, @items: Array[String])
    @count = 0
  end

  def label -> &String
    @name
  end

  def var rename(n: String) -> nil
    @name = n
  end
end

def replace(s: &var String) -> nil
  s = "replaced-value-long-enough-to-need-a-new-buffer"
end

def eat(u: U) -> nil
  puts u.name
end

def mk(k: Int) -> U
  U.new("old-#{k}", A.new("a-#{k}"), ["e0-#{k}", "e1-#{k}"])
end
'''

# shape: (setup lines binding `x`, how to read the borrow, path kind)
# path kind decides the same-path write: name / aname / item
SHAPES = {
    "field":   (["x = u.name"], "puts x", "name"),
    "nested":  (["x = u.a.name"], "puts x", "aname"),
    "index":   (["its = u.items", "x = its[0]"], "puts x", "item"),
    "method":  (["x = u.label"], "puts x", "name"),
    "if":      (["x = if k > 1", "  u.name", "else", "  u.label", "end"], "puts x", "name"),
    "case":    (["x = case k", "when 3 then u.name", "else u.label", "end"], "puts x", "name"),
    "tail":    (["x = if k > 1", "  puts \"in\"", "  u.a.name", "else", "  u.a.name", "end"], "puts x", "aname"),
    "cast":    (["x = u.label as &String"], "puts x", "name"),
    "multi":   (["w = mk(9)", "x = w.name", "n = 0", "x, n = u.name, 1"], "puts x", "name"),
    "copy":    (["w = u.name", "x = w"], "puts x", "name"),
    "closure": (["y = u.name", "x = -> { puts(y) }"], "x.()", "name"),
    "temp":    (["x = mk(7).name"], "puts x", "name"),
    # round 3 (I-29): aggregates holding a borrow, and a `&var` binding.
    "array":   (["x = [u.name]"], "puts x[0]", "name"),
    "tuple":   (["x = (u.name, 1)"], "puts x.0", "name"),
    "hash":    (['x = {"a" => u.name}'], 'puts x["a"]', "name"),
    "some":    (["x = Some(u.name)"], "puts x.unwrap", "name"),
    "arrlam":  (["y = u.name", "x = [-> { puts(y) }]"], "x[0].()", "name"),
    "somelam": (["y = u.name", "x = Some(-> { puts(y) })"], "x.unwrap.()", "name"),
    "lamarr":  (["y = u.name", "f = -> { puts(y) }", "x = [f]"], "x[0].()", "name"),
    "varbind": (["x = &var u.name"], "puts x", "name"),
}

def same_path(kind, what):
    """The write `what` on the borrowed path of `kind`."""
    place = {"name": "u.name", "aname": "u.a.name", "item": "its[0]"}[kind]
    if kind == "item" and what == "assign":
        return ['u.items = ["n-#{k}"]']
    if kind == "item" and what == "multi":
        return ["m = 0", 'u.items, m = ["n-#{k}"], 2']
    if what == "assign":
        return [f'{place} = "n-#{{k}}"']
    if what == "opassign":
        return [f'{place} += "-more"']
    if what == "multi":
        return ["m = 0", f'{place}, m = "n-#{{k}}", 2']
    if what == "varx":
        return [f"replace(&var {place})"]
    if what == "vara":
        return [f"replace({place})"]

OPS = {
    "assign_root":  lambda kind: ['u = mk(5)'],
    "assign_field": lambda kind: same_path(kind, "assign"),
    "opassign":     lambda kind: same_path(kind, "opassign"),
    "multi_target": lambda kind: same_path(kind, "multi"),
    "alias_write":  lambda kind: ["al = u.a", 'al.name = "via-alias-#{k}"'],
    "var_self":     lambda kind: ['u.rename("r-#{k}")'],
    "var_explicit": lambda kind: same_path(kind, "varx"),
    "var_auto":     lambda kind: same_path(kind, "vara"),
    "move_call":    lambda kind: ["eat(u)"],
    "for_write":    lambda kind: ["for it in u.items", '  u.items = ["z-#{k}"]', "  puts(it)", "end"],
}

def program(shape, op, variant):
    setup, read, kind = SHAPES[shape]
    body = ["u = mk(k)"] + setup
    if variant == "dead":
        body.append(read)
    if op == "for_write" and variant == "dead":
        # The loop always borrows the collection it walks: its dead form
        # writes a disjoint field instead.
        body += ["for it in u.items", "  u.count += 1", "  puts(it)", "end"]
    else:
        body += OPS[op](kind)
    if variant == "conflict":
        body.append(read)
    if op != "move_call":
        body.append("puts u.name")
        body.append("puts u.a.name")
        body.append("puts(u.items.size)")
    fn = "def run(k: Int) -> nil\n" + "\n".join("  " + l for l in body) + "\nend\n\nrun(3)\n"
    return SHAPE_PRELUDE + "\n" + fn

def returns(variant):
    # the borrow leaves the function whose local root dies
    tail = "x" if variant == "conflict" else "x.clone"
    return SHAPE_PRELUDE + f'''
def leak_out(k: Int) -> String
  u = mk(k)
  x = u.name
  {tail}
end

puts leak_out(3)
'''

def shape_rows():
    for shape in SHAPES:
        for op in OPS:
            for variant in ("conflict", "dead"):
                yield f"{shape}__{op}__{variant}", program(shape, op, variant), None
    for variant in ("conflict", "dead"):
        yield f"field__return__{variant}", returns(variant), None

# ---------------------------------------------------------------------------
# Round 3 (I-29, I-30, I-31, I-24, P-11): operand ordering in calls, binary
# operators and literals; free-function trailing blocks; `&var` and
# `def var` access to captured variables in blocks, lambdas and procs. Each
# row is `conflict` (a later operand or the block reaches what an earlier
# operand points into) or `dead` (it reaches something else, or the earlier
# operand is Copy: `__dead_conflict`). Capture rows carry their output.
# ---------------------------------------------------------------------------


OPERAND_PRELUDE = '''class A
  name: String
  def init(@name: String)
  end
  def show_with(n: Int) -> nil
    puts(@name)
  end
end

class U
  name: String
  count: Int
  a: A
  def init(@name: String, @a: A)
    @count = 0
  end
  def var rename_n(n: String) -> Int
    @name = n
    1
  end
  def var rename_ref(n: &String) -> nil
    @name = "renamed-a-long-enough-string-to-realloc"
    puts(n)
  end
end

class V
  def init
  end
  def show(s: &String, n: Int) -> nil
    puts(s)
  end
end

def mk(k: Int) -> U
  U.new("old-#{k}", A.new("a-#{k}"))
end

def reset(u: &var U) -> Int
  u.name = "reset-a-long-enough-string-to-realloc"
  u.a = A.new("reset-a")
  1
end

def resets(u: &var U) -> String
  u.name = "reset-a-long-enough-string-to-realloc"
  "r"
end

def show(s: &String, n: Int) -> nil
  puts(s)
end

def setn(s: &var String, n: Int) -> nil
  s = "set-#{n}-long-enough-to-need-a-new-buffer"
end

def replace(s: &var String) -> nil
  s = "replaced-value-long-enough-to-need-a-new-buffer"
end

def bump(n: &var Int) -> nil
  n += 4
end

def h(s: &var String, &block: Fn[() -> nil]) -> nil
  s = "h-wrote-a-long-enough-string-to-realloc"
  yield
end

def hs(s: &String, &block: Fn[() -> nil]) -> nil
  yield
  puts(s)
end

def run_block(&block: Fn[() -> nil]) -> nil
  yield
end
'''

# operand ordering: FIRST forms take `L` (the later operand, an Int or a
# String expression) and evaluate an operand pointing into `u.name` (or
# `u.a`) before it.
FIRST = {
    "arg_shared": ("show(u.name, {L})", "int"),
    "arg_var": ("setn(&var u.name, {L})", "int"),
    "arg_auto": ("setn(u.name, {L})", "int"),
    "recv_arg": ("v.show(u.name, {L})", "int"),
    "recv_obj": ("u.a.show_with({L})", "int"),
    "binop": ("s2 = u.name + {L}\nputs(s2)", "str"),
    "tuple": ("t = (u.name, {L})\nputs(t.0)", "int"),
    "copy_first": ("show2 = u.count + {L}\nputs(show2)", "int"),
}
LATER = {
    "reset_auto": {"int": "reset({o})", "str": "resets({o})"},
    "reset_var": {"int": "reset(&var {o})", "str": "resets(&var {o})"},
    "var_self": {"int": '{o}.rename_n("r")', "str": None},
}

def operand_rows():
    for first, (tmpl, kind) in FIRST.items():
        for later, forms in LATER.items():
            form = forms[kind]
            if form is None:
                continue
            for variant in ("conflict", "dead"):
                target = "u" if variant == "conflict" else "w"
                call = tmpl.replace("{L}", form.replace("{o}", target))
                body = ["u = mk(k)", "w = mk(9)", "v = V.new"] + call.split("\n") + [
                    "puts u.name", "puts w.name"]
                # the Copy first operand never points into u: both forms are dead
                v = "dead" if first == "copy_first" else variant
                yield f"operand__{first}__{later}__{v}{'' if first != 'copy_first' else '_' + variant}", body, None

def block_rows():
    places = {"local": "s", "field": "u.name", "nested": "u.a.name"}
    for name, place in places.items():
        for variant in ("conflict", "dead"):
            read = place if variant == "conflict" else "w.name"
            body = ["u = mk(k)", "w = mk(9)", 's = "loc-#{k}"',
                    f"h(&var {place}) {{ puts({read}) }}", f"puts({place})"]
            yield f"fnblock__{name}__read__{variant}", body, None
        # a shared first operand, the block writes the place it points into
        if name != "local":
            for variant in ("conflict", "dead"):
                obj = "u" if variant == "conflict" else "w"
                body = ["u = mk(k)", "w = mk(9)", f"hs({place}) {{ reset({obj}) }}", "puts(u.name)"]
                yield f"fnblock__{name}__write__{variant}", body, None

# captures: closure kind x access. Each prints the captured value after the
# access; the expected output is the final value.
LONG = "replaced-value-long-enough-to-need-a-new-buffer"
MORE = "-more-more-more-more-more-more-more"
ACCESS = {
    "explicit": ("replace(&var s)", LONG),
    "auto": ("replace(s)", LONG),
    "varself": (f's.push_str("{MORE}")', "cap-3" + MORE),
    "assign": ('s = "assigned-#{k}"', "assigned-3"),
    "opassign": (f's += "{MORE}"', "cap-3" + MORE),
}

def capture_rows():
    for kind in ("block", "lambda", "proc"):
        for access, (stmt, expect) in ACCESS.items():
            if kind == "block":
                body = ['s = "cap-#{k}"', "run_block do", f"  {stmt}", "end", "puts(s)"]
            else:
                open_ = "-> {" if kind == "lambda" else "proc {"
                body = ['s = "cap-#{k}"', f"f = {open_}", f"  {stmt}", "  puts(s)", "}", "f.()"]
            yield f"capture__{kind}__{access}__dead", body, expect + "\n"
        # an Int capture written through `&var`
        if kind == "block":
            body = ["n = 1", "run_block do", "  bump(&var n)", "end", "puts(n)"]
        else:
            open_ = "-> {" if kind == "lambda" else "proc {"
            body = ["n = 1", f"f = {open_}", "  bump(&var n)", "  puts(n)", "}", "f.()"]
        yield f"capture__{kind}__intvar__dead", body, "5\n"
    # conflict: a live borrow of a field, a block that replaces the field
    for variant in ("conflict", "dead"):
        read = "puts y" if variant == "conflict" else "puts u.name"
        pre = ["u = mk(k)", "y = u.name"] + (["puts y"] if variant == "dead" else [])
        body = pre + ["run_block do", "  replace(&var u.name)", "end", read]
        yield f"capture__block__fieldvar__{variant}", body, None
    # a block nested in a lambda writes the lambda's own capture
    body = ['s = "cap-#{k}"', "f = -> {", "  run_block do", "    replace(&var s)", "  end",
            "  puts(s)", "}", "f.()"]
    yield "capture__nested__explicit__dead", body, LONG + "\n"

def receiver_rows():
    # a `var self` receiver is a mutable loan for its call
    for arg, label in (("u.name", "field"), ("u.a.name", "nested"), ("x", "binding")):
        for variant in ("conflict", "dead"):
            obj = "u" if variant == "conflict" else "w"
            pre = ["u = mk(k)", "w = mk(9)"] + (["x = " + obj + ".name"] if label == "binding" else [])
            a = arg if label == "binding" else arg.replace("u.", obj + ".", 1)
            yield f"receiver__{label}__varself__{variant}", pre + [f"u.rename_ref({a})", "puts(u.name)"], None

def run_fn(body):
    """`body` as the body of `run(k)`, called with k = 3."""
    return "def run(k: Int) -> nil\n" + "\n".join("  " + l for l in body) + "\nend\n\nrun(3)\n"

def operand_section():
    for gen in (operand_rows, block_rows, capture_rows, receiver_rows):
        for name, body, expect in gen():
            yield name, OPERAND_PRELUDE + "\n" + run_fn(body), expect

# ---------------------------------------------------------------------------
# Round 5 (I-41, I-42): views and write targets whose place is not one
# syntactic path. Every row runs with k = 3, so the condition `k > 1` picks
# `u`, the borrowed object: an accepted conflict frees the buffer the view
# still reads, which the memory run catches. A `dead` row whose view stays
# live (read after the write) denotes only other places and must stay
# accepted.
# ---------------------------------------------------------------------------

MERGE_PRELUDE = SHAPE_PRELUDE + '''
def pick(a: &A, b: &A, c: Bool) -> &A
  if c
    a
  else
    b
  end
end

def pick_s(a: &String, b: &String, c: Bool) -> &String
  if c
    a
  else
    b
  end
end

class W
  def init
  end

  def pick(a: &A, b: &A, c: Bool) -> &A
    if c
      a
    else
      b
    end
  end
end
'''

BIG = "written-a-long-enough-string-to-realloc-the-buffer"
SETUP = ["u = mk(k)", "w = mk(9)"]
TAIL = ["puts u.name", "puts u.a.name", "puts w.a.name"]

# Merges of two places {A} and {B} on condition {C}: `&&`/`||` take only Bool
# operands (E0731), so the expression merges are if, case and the ternary;
# the statement merges are a conditional rebind and a loop-carried rebind.
MERGES = {
    "if": ["{v} = if {C}", "  {A}", "else", "  {B}", "end"],
    "case": ["{v} = case k", "when 3 then {A}", "else {B}", "end"],
    "ternary": ["{v} = {C} ? {A} : {B}"],
    "rebind": ["{v} = {B}", "if {C}", "  {v} = {A}", "end"],
    "loop": ["{v} = {B}", "i = 0", "while i < k", "  {v} = {A}", "  i += 1", "end"],
}

def merge(form, v, a, b, cond="k > 1"):
    return [l.replace("{v}", v).replace("{A}", a).replace("{B}", b).replace("{C}", cond)
            for l in MERGES[form]]

# Writes through an `A` view `al` (the merge or call result).
AL_OPS = {
    "alias_write": [f'al.name = "{BIG}"'],
    "var_explicit": ["replace(&var al.name)"],
    "var_auto": ["replace(al.name)"],
}
# Writes to `u.a.name` or `u.name`, the place a merged view may denote.
def place_ops(place):
    ops = {
        "assign_field": [f'{place} = "{BIG}"'],
        "var_explicit": [f"replace(&var {place})"],
        "assign_root": ["u = mk(5)"],
    }
    if place == "u.a.name":
        ops["alias_write"] = ["al = u.a", f'al.name = "{BIG}"']
    else:
        ops["var_self"] = [f'u.rename("{BIG}")']
    return ops

def ordered(view, op, variant, read="puts x"):
    """`conflict` reads the view after the write, `dead` before it."""
    if variant == "conflict":
        return view + op + [read]
    return view + [read] + op

def merge_rows():
    # The write target is a merge of u.a and w.a; the view is u.a.name.
    for form in MERGES:
        for op, lines in AL_OPS.items():
            for variant in ("conflict", "dead"):
                view = ["x = u.a.name"] + merge(form, "al", "u.a", "w.a")
                yield f"merge_{form}__{op}__{variant}", SETUP + ordered(view, lines, variant) + TAIL
    # Both arms one place: u.a (conflict) or w.a (dead, view live).
    # Other arm: the view is w.a.name, which `k < 2` picks at run time
    # (conflict); a disjoint field u.name (dead, view live).
    for op, lines in AL_OPS.items():
        for shape, cond, conflict, dead in (
            ("merge_samearm", "k > 1", ("u.a.name", "u.a", "u.a"), ("u.a.name", "w.a", "w.a")),
            ("merge_field", "k < 2", ("w.a.name", "u.a", "w.a"), ("u.name", "u.a", "w.a")),
        ):
            for variant, (place, a, b) in (("conflict", conflict), ("dead", dead)):
                view = [f"x = {place}"] + merge("if", "al", a, b, cond)
                yield f"{shape}__{op}__{variant}", SETUP + view + lines + ["puts x"] + TAIL
    # The view is a merge of u.a.name and w.a.name; the write is to u.
    src_forms = ("if", "case", "ternary", "rebind")
    for form in src_forms:
        for op, lines in place_ops("u.a.name").items():
            for variant in ("conflict", "dead"):
                view = merge(form, "x", "u.a.name", "w.a.name")
                yield f"mergesrc_{form}__{op}__{variant}", SETUP + ordered(view, lines, variant) + TAIL
    for op, lines in place_ops("u.a.name").items():
        for variant, place in (("conflict", "u.a.name"), ("dead", "w.a.name")):
            view = merge("if", "x", place, place)
            yield f"mergesrc_samearm__{op}__{variant}", SETUP + view + lines + ["puts x"] + TAIL

def call_rows():
    # A write through a call result that may be either argument.
    calls = {
        "pick_fn": "al = pick(u.a, w.a, k > 1)",
        "pick_second": "al = pick(w.a, u.a, k < 2)",
        "pick_method": "al = v.pick(u.a, w.a, k > 1)",
    }
    for shape, call in calls.items():
        for op, lines in AL_OPS.items():
            for variant in ("conflict", "dead"):
                view = ["v = W.new", "x = u.a.name", call]
                yield f"{shape}__{op}__{variant}", SETUP + ordered(view, lines, variant) + TAIL
    for op, lines in AL_OPS.items():
        for variant, obj in (("conflict", "u"), ("dead", "w")):
            view = ["x = u.a.name", f"al = pick({obj}.a, {obj}.a, k > 1)"]
            yield f"pick_same__{op}__{variant}", SETUP + view + lines + ["puts x"] + TAIL
    # A view that is a call result into either argument; the write is to u.
    views = {
        "pickview_fn": "x = pick_s(u.name, w.name, k > 1)",
        "pickview_second": "x = pick_s(w.name, u.name, k < 2)",
    }
    for shape, call in views.items():
        for op, lines in place_ops("u.name").items():
            for variant in ("conflict", "dead"):
                yield f"{shape}__{op}__{variant}", SETUP + ordered([call], lines, variant) + TAIL
    for op, lines in place_ops("u.name").items():
        for variant, obj in (("conflict", "u"), ("dead", "w")):
            view = [f"x = pick_s({obj}.name, {obj}.name, k > 1)"]
            yield f"pickview_same__{op}__{variant}", SETUP + view + lines + ["puts x"] + TAIL

def closure_rows():
    # A closure taking `&var U`, called directly while a view into u is live.
    kinds = {
        "lambda": f'f = ->(q: &var U) {{ q.name = "{BIG}" }}',
        "proc": f'f = proc {{ |q: &var U| q.name = "{BIG}" }}',
        "block": f'f = {{ |q: &var U| q.name = "{BIG}" }}',
    }
    calls = {"call_var": "f.(&var u)", "call_auto": "f.(u)", "call_method": "f.call(&var u)"}
    for kind, closure in kinds.items():
        for op, call in calls.items():
            for variant in ("conflict", "dead"):
                view = [closure, "x = u.name"]
                yield f"clocall_{kind}__{op}__{variant}", SETUP + ordered(view, [call], variant) + TAIL
    # The closure writes the nested q.a.name: the view u.a.name (conflict)
    # or u.name with the call taking w (dead, view live).
    nested = f'f = ->(q: &var U) {{ q.a.name = "{BIG}" }}'
    for op in ("call_var", "call_auto"):
        for variant, view, call in (("conflict", "x = u.a.name", calls[op]),
                                    ("dead", "x = u.name", calls[op].replace("u", "w"))):
            body = [nested, view, call, "puts x"]
            yield f"clocall_target__{op}__{variant}", SETUP + body + TAIL
    # Read-only closures over a `&U` parameter: the view stays live.
    readers = {
        "lambda": "f = ->(q: &U) { puts(q.name) }",
        "proc": "f = proc { |q: &U| puts(q.name) }",
        "block": "f = { |q: &U| puts(q.name) }",
    }
    for kind, closure in readers.items():
        for op, call in (("call", "f.(u)"), ("call_method", "f.call(u)")):
            body = [closure, "x = u.name", call, "puts x"]
            yield f"clocall_read_{kind}__{op}__dead", SETUP + body + TAIL
    # A closure that writes a captured field, called directly. Closures
    # capture by move, so u is not read after the closure is made.
    opens = {"lambda": ("-> {", "}"), "proc": ("proc {", "}"), "block": ("{", "}")}
    writes = {
        "assign_field": f'u.name = "{BIG}"',
        "var_explicit": "replace(&var u.name)",
        "var_self": f'u.rename("{BIG}")',
    }
    for kind, (open_, close) in opens.items():
        for op, stmt in writes.items():
            for variant in ("conflict", "dead"):
                view = ["x = u.name"]
                call = [f"f = {open_} {stmt} {close}", "f.()"]
                yield f"clocap_{kind}__{op}__{variant}", SETUP + ordered(view, call, variant) + ["puts w.name"]

def round5_section():
    for gen in (merge_rows, call_rows, closure_rows):
        for name, body in gen():
            yield name, MERGE_PRELUDE + "\n" + run_fn(body), None

def rows():
    yield from shape_rows()
    yield from operand_section()
    yield from round5_section()

def main(argv):
    check = "--check" in argv
    args = [a for a in argv if a != "--check"]
    out = args[0] if args else os.path.dirname(os.path.abspath(__file__))
    os.makedirs(out, exist_ok=True)
    files = {}
    for name, text, expect in rows():
        assert name + ".rx" not in files, f"duplicate row {name}"
        files[name + ".rx"] = text
        if expect is not None:
            files[name + ".expect"] = expect
    if check:
        on_disk = {f for f in os.listdir(out) if f.endswith((".rx", ".expect"))}
        stale = sorted(f for f, t in files.items()
                       if f not in on_disk or open(os.path.join(out, f)).read() != t)
        extra = sorted(on_disk - files.keys())
        for f in stale:
            print(f"differs or missing: {f}")
        for f in extra:
            print(f"not generated: {f}")
        print(f"{len(files)} generated files, {len(stale)} differ, {len(extra)} extra")
        return 1 if stale or extra else 0
    for f, t in files.items():
        with open(os.path.join(out, f), "w") as fh:
            fh.write(t)
    print(sum(f.endswith(".rx") for f in files), "programs")
    return 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
