# B33 design spec: the field-walk codegen substrate

Design document for batch **B33** in [`../QUEUE.md`](../QUEUE.md), enabler **E6**.
This is a design, not an implementation: no Rust, no `.rx` implementation.

**Why it deserves a design.** QUEUE.md marks B33 the highest-leverage compiler
batch after per-`T` mono, and the dependency graph agrees: B55 (folio's `Model`
synthesis), B44 (`to_json`/`from_json`), B50 (`Struct`, `Data`, `Forwardable`,
`Singleton`), B63 (`FromParams` strong params), and most of B34 all sit behind
it. Getting the substrate shape wrong means rewriting the ORM after it exists.

**The binding constraint.** Ruxen has no derive macros and will not get them.
The compile-time surface is mixins (`include`) plus `extension`. The north star
(`docs/specs/system/generic-compiler.spec.md`) is stricter still: *grep for any
stdlib class name across `compiler/ruxen_core/src/` must be empty*, and a
per-feature special-case in `typeck`/`mir` is named as the anti-pattern. So the
substrate must be **one generic mechanism** with **zero knowledge of `Model`,
`Serializable`, or `FromParams`.**

Everything marked **(probed)** was verified on 2026-07-29 by compiling with
`./target/release/ruxen compile`. The probes are what shaped this design: two of
them ruled out the obvious approach, and two more (P9, P10) forced a real
capability out of v1.

---

## 1. Probe results (read these first, they decide the design)

| # | Question | Result |
|---|---|---|
| **P1** | Can a mixin default body read a field of the including class? | **No.** `mixin Describable; def describe = "#{self.name}"` included by a class with a `name` field fails: `error: no field 'name' on type 'Self: Describable'`. Typeck checks a mixin default **once, with `Self` abstract**, and an abstract `Self` has no fields. |
| **P2** | Can a mixin declare a required *field*? | **No.** A field declaration in a mixin body is `error: expected 'def' or 'type' in mixin body`. Mixins carry methods and associated **types** only. |
| **P4** | Can a mixin default call a *required method* the class implements? | **Yes.** The `Enumerable`-over-`each` shape works for any mixin: declare `def label -> String` body-less, implement it on the class, and the default body composes over it. |
| **P6** | Can a class method be generic over a visitor bounded by a mixin, and call typed per-field methods on it? | **Yes, end to end.** A hand-written `def accept[V: FieldVisitor](v: &var V)` calling `v.visit_string("name", &self.name)` then `v.visit_int("age", self.age)` compiles, links, and prints `name=ada;age=36;`. |
| **P7** | Can a *class* method generic over a reader construct `Self`? | **Yes.** A hand-written `def self.from_fields[R: FieldReader](r: &var R) -> User` calling `User.new(r.read_string("name"), r.read_int("age"))` compiles and runs. |
| **P5** | What happens when two included mixins define the same method name? | A raw internal error: `Failed to define function 'C_tag': DuplicateDefinition("C_tag")`. **No error code, no source span, no mention of which two mixins collided.** |
| **P8** | Does `include A, B` (the comma form the naming spec calls "the loud derive form") parse in a class body? | **No.** `error: expected field, method, or include directive in class body, found Comma`. One `include` per line. |
| **P9** | Can a mixin declare a *generic* required method (`mixin Walkable; def accept[V: FieldVisitor](v: &var V)`)? | **No.** `error: undefined type 'V'` at the declaration. A required-method signature inside a mixin does not resolve its own type parameter, so `Walkable` cannot be used as a bound meaning "has an `accept`". |
| **P10** | Can a synthesized `accept[V]` recurse into a nested walkable field by calling `self.addr.accept(v)`? | **No.** `MIR lowering error: cannot monomorphize generic method 'accept': its mixin-bound type argument did not resolve to a concrete type at this call site (generic-calling-generic across methods requires a concrete leaf)`, a documented limit (`docs/decisions/q17-cross-package-monomorphization.md`). |
| **P11** | Does the same nested recursion work when the leaf is **concrete** (`self.addr.accept_dump(v)` where the visitor type is fixed)? | **Yes**, prints `name=ada;city=london;`. So the mechanism is sound and only the generic-to-generic forwarding is blocked. This is what a monomorphization-time synthesis would emit. |
| **P12** | Does the read direction handle heap-owning and class-typed fields, and repeated reads from one reader? | **Yes.** `User.new(r.read_string("name"), r.read_addr("home"), r.read_addr("work"))` with `String` and two `Addr` fields compiles, runs, and prints correctly. No ownership obstacle found for these cases. |

Two consequences settle the whole design:

- **P1 kills the obvious approach.** "Let a mixin body iterate the including
  class's fields" is not a small extension: a mixin body cannot touch a field at
  all today. Any design in that direction has to introduce per-implementor
  typechecking of mixin defaults, which is a large and risky change to how
  `Self` is handled.
- **P6 and P7 show the whole feature already works** when the per-field body is
  written by hand. The language needs nothing new. **The only thing missing is
  that a human has to write those bodies.**

So the substrate's job is narrow and mechanical: **write those bodies from the
field walk.**

---

## 2. The mechanism

### Decision

**The compiler synthesizes, from a class's field walk, a small fixed set of
ordinary typed methods whose bodies are straight-line sequences of ordinary
calls. Every feature is then an `.rx` type implementing a plain mixin.**

Three synthesized members, and nothing else:

| Synthesized member | Shape | Direction |
|---|---|---|
| `def self.field_names -> Array[String]` | the declared field names, in declaration order | metadata |
| `def accept[V: FieldVisitor](v: &var V) -> nil` | one `v.visit_<type>("field", self.field)` call per field, in order | write / read-out |
| `def self.from_fields[R: FieldReader](r: &var R) -> Result[Self, FieldError]` | one `r.read_<type>("field")` per field, passed positionally to the class's constructor | read / build-up |

`FieldVisitor` and `FieldReader` are **`.rx` mixins declared in the stdlib, not
in Rust.** The compiler never names them and never names a `visit_*` method: it
emits a call whose method name is derived from the field's type by one uniform
rule, and ordinary method resolution does the rest. If the visitor does not
provide that method, it is an ordinary "no such method" failure, upgraded to a
proper diagnostic per §6.

The consequence is the property the north star asks for: **the compiler learns
one mechanism (walk fields, emit a call per field) and zero features.** `to_json`,
`Model#to_row`, `FromParams`, `Struct#to_h`, and anything invented later are
`.rx` visitors written by a library author, not Rust.

### Why not the alternatives

**Alternative A: extend `derive.rs` with a Rust synthesizer per feature.** This
is what exists today: `synthesize_struct_clone`, `synthesize_struct_to_debug`,
`synthesize_struct_hash_code`, `synthesize_struct_default`, `synthesize_struct_cmp`,
each a hand-built MIR emitter of 60 to 150 lines. Rejected: every new consumer
(`Model`, `Serialize`, `Deserialize`, `FromParams`, `to_xml`) becomes another
Rust MIR builder with the feature's semantics baked into the compiler. That is
the exact "hardcoded stdlib knowledge in Rust" the acceptance criterion forbids,
and it scales linearly in Rust code with the number of features.

**Alternative B: a compile-time `for field in Self.fields` loop in `.rx`,
unrolled per implementor.** More expressive, and superficially the natural
reading of "attribute-driven codegen". Rejected for v1 on three grounds: it
needs a new expression-level construct and a new expansion stage; per P1 it
additionally requires making mixin defaults typecheck per implementor, since the
unrolled body's shape depends on the concrete field list; and it buys expressive
power no known consumer needs. Every consumer in the queue is expressible as a
visitor. Worth revisiting only if a consumer appears that genuinely cannot be
(see §11 open question 5).

**Alternative C: synthesize per-field accessor methods and let mixins compose
over them (the P4 shape).** This is what folio Phase 0 does by hand. It works for
the metadata half, but it cannot express `to_json`, because a mixin default body
that wants "the value of field *i*, at its real type" has no way to name field
*i* generically. It degrades to stringly `Array[String]`, which is adequate for
SQL binding and wrong for JSON (`JSON.make_int` versus `make_string`) and for
deserialization. Kept as the *metadata* member (`field_names`), discarded as the
whole answer.

### The type-to-method-name rule

The one rule the compiler owns. For a field of static type `T`, `accept` emits a
call to `visit_<slug(T)>` and `from_fields` emits `read_<slug(T)>`, where
`slug(T)` is a total, mechanical lowering of the type name (`Int` → `int`,
`String` → `string`, `Bool` → `bool`, `Float` → `float`, `Array[Int]` →
`array_int`, a user class `Todo` → `class` with the type passed generically).

This rule is knowledge about *the shape of a name*, not about any particular
type or method, so it does not violate the acceptance grep: no stdlib class name
is hardcoded, the slug is derived from whatever the field's type is called.

**Nested user-class fields are refused in v1, and the reason is a hard compiler
limit, not a preference.** The intended shape was that a field of walkable class
type recurses through the nested value's own `accept`. Two probes closed it:

- **P9:** a mixin cannot declare a generic required method (`undefined type 'V'`),
  so there is no way to spell a `Walkable` bound meaning "has an `accept[V]`".
- **P10:** even without the bound, a synthesized `accept[V]` calling
  `self.addr.accept(v)` fails at MIR lowering, because forwarding a mixin-bound
  type argument from one generic method into another has no concrete leaf. That
  is the documented Q17 boundary, not a bug to fix in passing.

**P11 shows the shape is sound once the leaf is concrete**, which is exactly what
a monomorphization-time synthesis would emit (`self.addr.accept__mono__JsonWriter(v)`).
So nested fields are not impossible, they are **deferred to the same sub-batch as
generic including classes (B33f)**, where synthesis participates in
monomorphization and both problems dissolve together. Until then a walkable field
whose type is itself walkable is **E1206**, a clean refusal.

**This is a real dent in the design and it is worth stating plainly.** `gems-web.md`
§6 lists "the derive recurses into fields that are themselves `Serializable`" as a
requirement, and v1 will not do it. What v1 does cover is flat classes, which is
what folio models, params structs, and DTOs actually are. A nested value object
must be flattened, or hand-write `accept` (§7.1 guarantees an own method wins).

---

## 3. Where it runs in the pipeline

### Decision: synthesize at the end of typeck, as real HIR, before MIR sees anything

The field list and every field's **resolved type** must exist before a call can
be emitted for it, which rules out resolve. The synthesized bodies must be
**typechecked** (a field whose type the visitor cannot handle has to be a
diagnostic with a span, not a link error), which rules out MIR lowering.

So: run the walk after typeck has resolved class fields and their types, emit
real HIR methods onto the class, and let the *existing* typeck and MIR pipeline
treat them as ordinary user-written methods. MIR needs no knowledge of the
substrate whatsoever.

### Why this matters more than it looks

The current derive machinery is gated **in two places that must agree**: typeck
resolves `clone`/`to_s`/`default` through `resolver.rs::structural_fallback_resolvers`
gated on `ty_has_derive_trait`, and MIR synthesizes the bodies in
`mir/lower/derive.rs` gated identically in `lower_item`. The spec's own words
for that pattern, in the general case, are "a marker the two stages must keep in
lockstep" and it is listed as the anti-pattern.

Synthesizing into HIR **removes the lockstep entirely**: there is one gate, at
one place, and both later stages see a method that simply exists. B33 should not
add a second lockstep. Over time it gives a path to retiring the existing one
(§11.4), though that is explicitly not in scope here.

### Ordering constraint

Synthesis must run **after** all classes' fields are resolved and **before**
method collection finalizes, because the synthesized methods must be visible to
every call site including ones in other modules. Practically that is a distinct
pass at the typeck boundary, not an interleaved hook, so it is deterministic and
debuggable: dumping the post-synthesis HIR shows exactly what a user would have
had to write by hand. That dump should be an `--emit` mode from day one (§10),
because "show me what was generated" is the difference between a debuggable
codegen system and a magic one.

---

## 4. Composition with generics

This is the sharpest interaction in the design and the reason one sub-batch is
deferred.

**Non-generic including class (the v1 target).** Field types are concrete, the
slug rule resolves, the emitted calls typecheck normally. Nothing special.

**Generic including class (`class Box[T]`, field `value: T`).** The walk sees a
field whose type is a type parameter. `slug(T)` is not knowable until `T` is
concrete, so the call `v.visit_<slug(T)>` cannot be emitted at synthesis time.
Two options:

1. **Emit `visit_generic[T](name, value)`** and require every visitor to provide
   a generic method. Total, but it pushes the type dispatch into the visitor,
   which then hits exactly the constraint that blocks the collection batches: an
   `.rx` body cannot dispatch on an abstract `T` today.
2. **Defer synthesis for generic classes to monomorphization time**, so each
   concrete `Box[Int]` gets its own `accept` with `visit_int`. Correct, and it
   depends on the per-`T` monomorphization work landing (enabler **E1**, in
   flight).

**Recommendation: v1 refuses generic classes with a clean diagnostic (E1204) and
option 2 lands after E1.** Rationale: every consumer in the queue is a concrete
class. A folio model, a params struct, and a serializable DTO all have concrete
fields. Supporting generic classes before E1 exists would mean building option 1,
discovering it cannot dispatch, and rebuilding. Refusing loudly costs one
diagnostic and buys the right to do it once.

**The interaction to watch when it does land:** a synthesized body monomorphized
per `T` and generic over the visitor `V` is monomorphized on **two** axes. See
§7 for the size analysis, which is the reason to be careful rather than the
reason not to do it.

---

## 5. The attribute surface

### Decision: metadata is an overridable `.rx` class method, not a compiler-parsed attribute

Ruby carries per-field metadata with class macros (`serialize :prefs, coder:
JSON`, `column :label, name: "todo_label"`). Rust carries it with
`#[serde(rename = "…")]`. Ruxen has neither, and **`@[...]` attribute syntax was
deliberately retired** (`docs/specs/syntax/ruby-naming.spec.md`: "`@[...]` syntax
overall: retired"). The house replacements are in-body directives (`inline :f`,
`private :a, :b`, `test`) or keyword options on an existing construct (`lib "x",
...`). Reintroducing `@[...]` for this batch would reverse a settled decision.

An in-body directive (`column_name :label, "todo_label"`) is closer to house
style but has a fatal flaw for the north star: the compiler would have to *parse
and understand* `column_name`, which is per-feature knowledge in Rust, exactly
what the acceptance grep forbids.

So metadata rides on the mechanism that already exists and costs the compiler
nothing: **the mixin declares a metadata method with a default body, and a class
overrides it.**

```
# In the .rx mixin, not in the compiler:
mixin Model
  def self.column_name_for(field: &String) -> String
    field                        # default: the column is the field name
  end
end

# In the user's class, an ordinary method override:
class Todo
  include Model
  label: String

  def self.column_name_for(field: &String) -> String
    if field == "label" then "todo_label" else field end
  end
end
```

**Typed, not opaque strings**, and the justification is that it is not a
metadata *system* at all: it is a method, so it is typechecked, greppable,
refactorable, documentable, and per-feature by construction. A `Model` author
decides `Model` needs `column_name_for`; a serializer author decides
`Serializable` needs `json_key_for` and `skip_field?`. The compiler learns
neither. A per-field attribute table would have needed a schema, a lookup API, a
story for typing the values, and compiler knowledge of every key.

**The cost, stated honestly:** a field-name string comparison inside an
override is clumsier than `@[column("todo_label")]` sitting on the field, and it
is not checked against the real field set (a typo in the `if field == "labl"`
branch silently does nothing). Mitigation: the mixin can also declare a
`self.validate_metadata` hook the substrate calls with the real field-name list,
so a library author can turn a typo into a compile-time failure in `.rx`. That
mitigation is `.rx` code, so it costs the compiler nothing.

**Not expressible today:** `gems-db.md` proposes `include Model, primary_key:
:uuid`. Symbols do not exist (B29), and P8 shows `include` does not even take a
comma-separated list in a class body. The class-method override above is the
substitute, and `gems-db.md`'s row should be corrected when that file is next
free to edit.

---

## 6. Error behavior

Nothing in this substrate may fail silently or panic in lowering. Every failure
is a coded diagnostic with a span, and every code gets a `docs/errors/E####.md`
file (85 exist today; the registry is `diagnostics/codes.rs`, and a test enforces
that every emitted code is registered).

**Code range: E12xx is free** (in use today: E00xx, E06xx, E07xx, E10xx, E11xx,
E14xx, E16xx, E17xx). Proposed allocation:

| Code | Condition | Span points at |
|---|---|---|
| **E1201** | A field's type has no corresponding method on the visitor or reader (the slug rule produced `visit_json` and `FieldVisitor` has no `visit_json`) | the **field declaration**, with a secondary note on the `include` and on the mixin that lacks the method |
| **E1202** | Two included mixins synthesize or define the same method name | the second `include`, with a note naming both mixins and the method |
| **E1203** | A field-walk mixin is included on a type with no fields, or on an enum, where the walk is undefined | the `include` |
| **E1204** | Synthesis requested on a generic class (the v1 restriction, §4) | the `include`, with a note naming the type parameter |
| **E1205** | The read direction cannot construct `Self`: no `init` whose parameters match the walked field list | the `include`, with a note showing the field list and the constructors that were considered |
| **E1206** | A walked field's type is itself walkable, which v1 cannot recurse into (P9 and P10, §2) | the **field declaration**, with a note that it is lifted by B33f and that hand-writing `accept` is the workaround |

The most important of these is **E1201**, because it is the one a user hits by
accident (adding a field of an unsupported type to a model that previously
compiled). Its message must say which field, which type, which mixin, and what
the visitor would need to provide, since the fix is always "add a `visit_x` to
the visitor" or "do not walk this field".

**E1202 has a precedent to follow.** P5 shows a mixin method-name collision
currently reaches codegen as `Failed to define function 'C_tag':
DuplicateDefinition("C_tag")`. **E0727** exists precisely because the same class
of late `DuplicateDefinition` failure was converted into an early, clear
resolve-time diagnostic for type-name collisions, and its registry comment says
so. E1202 is that same move for method names, and it is worth doing regardless
of B33 because it is a bad error today. B33 makes it urgent because synthesis
multiplies the number of methods that can collide.

**No silent skip, ever.** A field the visitor cannot handle must not be quietly
omitted from the walk. Silent omission is how a serializer drops a column and
nobody notices until production data is missing. If skipping is wanted it must
be *requested*, through the mixin's own `.rx` metadata hook (§5), never inferred
from a failure.

---

## 7. Hygiene, collisions, and precedence

The rules, all decidable at compile time:

1. **A method the class defines itself always wins.** This is already the rule
   for mixin defaults (an own `def tap` beats `Kernel`'s) and it must hold for
   synthesized methods too, so a user can always hand-write `accept` when the
   generated one is wrong. This is the escape hatch that keeps the substrate from
   being a trap, and it should be pinned as a case.
2. **Two mixins synthesizing the same method name is an error (E1202), not a
   silent last-wins.** Ruby resolves this by ancestor order; Ruxen has no runtime
   MRO and a silent winner in generated code is undebuggable.
3. **Synthesized names live in the same flat namespace as everything else.**
   There is no hygiene mechanism (no gensym, no module-scoped names), so the
   three synthesized names are effectively reserved on any class that includes a
   walking mixin. That is a real constraint and it argues for few, distinctive
   names, which is why the design synthesizes three members and not thirty.
4. **A class including two different walking mixins** (`Model` and
   `Serializable`) is the common case, and it must work. Since both need
   `accept`, the synthesized `accept` is **generic over the visitor and shared**,
   not one per mixin. That is the deciding reason the visitor is a type parameter
   rather than each mixin getting its own synthesized method: it makes the
   substrate emit one method regardless of how many walking mixins are included.

Point 4 is load-bearing. It is what keeps code size linear (§8) and what avoids
the E1202 collision in the exact case that matters most.

---

## 8. Code size and compile-time cost

**The synthesis itself is linear.** One `accept` and one `from_fields` per class
that includes a walking mixin, each with one call per field. A 20-field model
contributes 40 emitted calls of HIR, once, regardless of how many walking mixins
it includes (§7.4).

**The multiplication is at monomorphization, and it is bounded by real usage.**
`accept[V]` is generic, so it monomorphizes per (class, visitor) pair *actually
called*. With N walked classes and K visitors, the worst case is N×K bodies, but
only pairs a program really uses are emitted. A realistic app: 30 models, 3
visitors (JSON out, row out, params in), 15 fields average. That is up to 90
bodies of about 15 calls each, roughly 1,350 emitted calls. That is small
compared to the existing derive output for the same 30 classes (`Clone`, `Debug`,
`eq`, `hash`, `cmp` are five hand-built bodies per class already).

**Where it could actually explode**, and the mitigations:

| Risk | Mitigation |
|---|---|
| Generic including classes (§4) monomorphize on two axes: per `T` **and** per visitor | The v1 restriction (E1204) removes this entirely. When it lands, cap it by monomorphizing only concrete `(T, V)` pairs reached from real call sites, which is what E1's design already does for `T`. |
| Nested user-class fields recursing through `visit_class[C]` pull in a body per nested type per visitor | Bounded by the type graph, which is finite and usually shallow. Cycles (a type reachable from itself) must be detected and refused, since the walk would not terminate. **This must be an explicit check, not an assumption.** |
| The generic small-method inliner (`generic-compiler.spec.md` item 5) inlining 15 straight-line calls into a hot serialize loop | Measure before assuming it is a problem. The inliner is generic and does not know about the substrate; if per-field calls turn out to be hot, the fix is a size heuristic in the inliner, not a change here. |

**Compile-time cost of the pass itself** is one walk per (class, walking mixin)
pair, proportional to total field count across the program. It is not the
expensive part; the monomorphized typechecking of the emitted bodies is. Worth
measuring at B33b, so there is a baseline before consumers arrive.

---

## 9. Worked examples

Both real consumers, showing what the author writes and what the compiler emits.
If either did not fall out cleanly the design would be wrong.

### 9.1 `to_json` / `from_json` (B44's derive half)

**What the library author writes**, once, in `.rx`, with no compiler involvement:

```
mixin FieldVisitor
  def visit_int(name: &String, value: Int) -> nil
  def visit_string(name: &String, value: &String) -> nil
  def visit_bool(name: &String, value: Bool) -> nil
  def visit_float(name: &String, value: Float) -> nil
end

class JsonWriter
  include FieldVisitor
  node: Json

  def visit_int(name: &String, value: Int) -> nil
    JSON.object_set(&var self.node, name, JSON.make_int(value))
  end

  def visit_string(name: &String, value: &String) -> nil
    JSON.object_set(&var self.node, name, JSON.make_string(value))
  end
  # … bool, float
end

mixin Serializable
  def to_json -> Json
    var w = JsonWriter.new(JSON.empty_object)
    self.accept(&var w)          # <- the synthesized member
    w.node
  end
end
```

**What the user writes:**

```
class Todo
  include Serializable
  id:    Int
  label: String
  done:  Bool
end
```

**What the compiler synthesizes onto `Todo`** (ordinary `.rx`-equivalent HIR,
the exact shape probe P6 proved compiles and runs):

```
def self.field_names -> Array[String]
  ["id", "label", "done"]
end

def accept[V: FieldVisitor](v: &var V) -> nil
  v.visit_int("id", self.id)
  v.visit_string("label", &self.label)
  v.visit_bool("done", self.done)
end
```

`from_json` is the mirror image over `FieldReader`, using the synthesized
`self.from_fields` (probe P7's shape). Adding a field to `Todo` updates both
directions with no other edit, which is the entire point.

### 9.2 folio's `Model` (B55)

`gems-db.md` asks for `table_name`, `columns`, `to_row`, and `from_row`, and
states that once a model exposes those four as real typed methods, **everything
else in the ORM is plain `.rx`**. All four fall out of the same three synthesized
members:

| Wanted | How it is delivered |
|---|---|
| `self.columns -> Array[String]` | **directly** the synthesized `self.field_names`, optionally mapped through the mixin's `.rx` `column_name_for` (§5) |
| `to_row -> Array[String]` | an `.rx` `RowWriter` visitor whose `visit_int` pushes `value.to_s`, `visit_bool` pushes `"1"`/`"0"`, and so on, driven by the synthesized `accept` |
| `self.from_row(row: &Json) -> Self` | an `.rx` `JsonRowReader` implementing `FieldReader`, driven by the synthesized `self.from_fields` |
| `self.table_name -> String` | **not a field walk at all.** It is derived from the class *name*, which is B34's territory (`.class.name` folding), or simply a `Model` default the user overrides. Worth calling out: this one row does not belong to B33. |
| `self.column_types -> Array[String]` | an `.rx` `TypeNameVisitor` whose `visit_int` pushes `"INTEGER"`, `visit_string` pushes `"TEXT"`. The SQL type map lives in folio, where it belongs, not in the compiler. |

That last row is the strongest evidence the design is right: the mapping from
Ruxen types to **SQL** types is exactly the kind of domain knowledge that must
never enter the compiler, and under this design it is fifteen lines of `.rx` in
the ORM package.

The one thing `Model` needs that this design does *not* give it is
`self.build(...)`, a keyword-ish constructor over non-`id` fields, which needs a
synthesized *signature*, not a synthesized body. That is a genuinely different
mechanism (it changes a method's arity from the field list). **Recommendation:
drop it from the ORM's requirements and use `from_fields` plus the ordinary
`init`.** Flagged as open question §11.3.

---

## 10. Non-goals, and the line against B34

**Non-goals, explicitly:**

- **Not a macro system.** There is no user-facing way to generate arbitrary code,
  no quoting, no AST manipulation. The substrate emits three known shapes.
- **Not runtime reflection.** Nothing is queryable at runtime that was not
  written into a value at compile time.
- **Does not let a mixin body read fields.** P1 stays true after B33. Mixins
  compose over the synthesized methods, exactly as `Enumerable` composes over
  `each`.
- **Does not synthesize arbitrary method names.** `define_method(:"find_by_#{col}")`
  has no substitute here. The `.rx` answer stays a typed builder (`Relation`),
  which is what `gems-db.md` already decided.
- **Does not replace the existing `Clone`/`Debug`/`Default` derives** in this
  batch (§11.4).

**The line against B34.** They are easy to confuse and they are opposites:

| | B33 | B34 |
|---|---|---|
| What it does | **generates code** from the field list | **removes code**, folding a reflective query to a constant |
| Output | new methods on a class | a literal, or a direct call |
| Runs | after typeck, emitting HIR | a fold pass in typeck |
| Needs symbols (B29) | no | yes, for literal-name folding |

They meet at exactly one point: B34's `instance_variables` wants a list of field
names, which is B33's `field_names`. **B34 should consume B33's synthesized
metadata rather than build a second field walker.** That is the only coupling,
and it is a consumption, not a dependency inversion.

**Debuggability requirement.** Generated code that cannot be read is a support
burden. An `--emit=synthesized` (or an extension of the existing `--emit=hir`)
must print the synthesized methods in `.rx`-equivalent form. A user filing "my
model serializes the wrong column" needs to see the generated `accept`, and a
maintainer diffing two compiler versions needs to see what changed. Ship it in
B33b, not later.

---

## 11. Open questions

1. ~~**Nested user-class fields.**~~ **CLOSED by probes P9 and P10** (§2): a
   mixin cannot declare a generic required method, and a synthesized `accept[V]`
   cannot forward `V` into a nested `accept` (the Q17 concrete-leaf boundary).
   Nested walkable fields are refused in v1 as **E1206** and move to B33f, where
   synthesis happens at monomorphization time and P11 shows the concrete-leaf
   form works. Recorded here because it is the one requirement in `gems-web.md`
   §6 that v1 does not meet.
2. **The opt-in spelling.** How does a class say "walk my fields"? Three options:
   (a) *shape recognition*, the compiler notices the class includes a mixin that
   requires a method matching the `accept` shape, which is implicit and magical;
   (b) *an in-body directive* (`synthesize :accept`), loud but the compiler
   learns a directive name; (c) *a marker mixin* the walking mixin itself
   includes (`mixin Model; include FieldWalk`), so the compiler knows exactly one
   name. **Recommend (c)**: one hardcoded name is the same budget the implicit
   `Kernel` include already spends, it reads like `include Clone`/`Debug` which
   users know, and it makes the walk opt-in at the *mixin* level so a user never
   writes it. The exact spelling is open.
3. **`Model.build(...)`** (§9.2): synthesizing a *signature* from the field list
   is a different mechanism from synthesizing a body. Recommend dropping it and
   correcting `gems-db.md`. If it is genuinely wanted, it should be its own batch
   with its own design, not smuggled into this one.
4. **Retiring the existing hand-built derives.** `Clone`, `Debug`, `Default`,
   `eq`, `hash`, `cmp` are five Rust MIR builders in `derive.rs` that this
   substrate could express as `.rx` visitors, deleting real Rust and moving the
   acceptance grep closer to empty. Attractive, and risky: they are load-bearing,
   heavily pinned, and on the hot path. **Recommend leaving them alone until
   B33e has proven the substrate on a real consumer**, then evaluating as a
   separate batch.
5. **Whether the visitor shape is enough forever.** Every consumer in the queue
   fits it. A consumer that needs to see all fields *simultaneously* rather than
   one at a time (a checksum over a canonical ordering is fine, but a
   cross-field validation such as "start_date before end_date" is not) would
   need Alternative B's field loop. Recommend building the visitor now and
   treating the loop as a possible future extension, since the visitor's
   synthesized `accept` remains valid either way.
6. ~~**Ownership in the read direction.**~~ **CLOSED by probe P12**: a
   `from_fields` reading a `String` and two class-typed `Addr` fields from one
   reader compiles, runs, and prints correctly, so heap-owning fields, nested
   class values, and repeated reads are all fine. The residual risk is narrower
   than feared but not zero: a field type with a user `Drop` was not tested, so
   B33c should still pin one before shipping.

---

## 12. Implementation sub-batches

Each is one agent, one session, in dependency order.

| Sub-batch | Goal | Depends on | Risk |
|---|---|---|---|
| **B33a: the walk and the metadata member** | Settle walk semantics (declaration order, inherited fields via the existing `collect_all_class_fields`, private fields, refusal on enums and field-less types) and synthesize `self.field_names` only. Plus the opt-in spelling from §11.2 and diagnostics E1203 and E1204. | nothing | **Low.** No visitor, no generics, no per-field typing. It is the cheapest place to get the walk semantics pinned, and everything later inherits them. |
| **B33b: the write direction** | Synthesize `accept[V: FieldVisitor]`, the slug rule, E1201, and the E1206 refusal of nested walkable fields (§2). Plus `--emit=synthesized`. First consumer is a trivial `.rx` dump visitor so it is testable without JSON. | B33a | **Medium.** The first place synthesized HIR must typecheck. |
| **B33c: the read direction** | Synthesize `self.from_fields[R: FieldReader]` and E1205. | B33b | **Medium.** Downgraded from medium-high by probe P12: heap-owning fields, class-typed fields, and repeated reads from one reader all work. Still pin a field type with a user `Drop` before shipping, since that is the untested corner and it fails as memory corruption, not a compile error. |
| **B33d: collisions and precedence** | E1202 replacing the raw `DuplicateDefinition` (following E0727's precedent), plus the precedence rules of §7, each pinned: own method beats synthesized, synthesized beats nothing, two walking mixins share one `accept`. | B33b | **Low**, and independently valuable: the current error is bad today. |
| **B33e: the metadata hooks and the first real consumers** | The `.rx` side: overridable `column_name_for` / `json_key_for` / `skip_field?` defaults (§5) plus `Serializable` and `Deserializable` written as `.rx` visitors. This **is** B44's derive half, so it should land under B44's pins, not B33's. | B33c | **Low** (it is `.rx`), but it is the acceptance test for the whole design. If a real consumer does not fall out cleanly here, stop and revisit §2. |
| **B33f: generic classes and nested walkable fields** | Lift **both** deferred restrictions, which dissolve together because both need synthesis to run at monomorphization time rather than at typeck: E1204 (generic including classes, synthesized per concrete `T`, two-axis with the visitor) and E1206 (nested walkable fields, emitting the concrete-leaf call P11 proved works). | B33e **and** E1 (per-`T` mono) | **Highest in the batch.** Two-axis monomorphization, the code-size risk of §8, and it moves synthesis out of the clean typeck-time position §3 argues for. Explicitly deferred. |

**Ship order:** B33a, B33b, then B33d in parallel with B33c, then B33e. B33f only
after E1.

**The riskiest is B33f**, and it is deferred. Of the work actually in scope,
**B33c is the riskiest**, because ownership errors in synthesized constructor
calls are silent at compile time and corrupt memory at runtime, which is the
worst failure profile in the batch. Give it the same treatment §10 of the B35
spec got: pin the hard cases before writing the mechanism.

**Pins:** B33 owns 1320-1329 under the QUEUE.md formula, which is enough if
B33e's pins land under B44 as recommended. If all six sub-batches need their own
blocks, take a fresh run from 1960 and record it in the exception table in
QUEUE.md §0, per the rule established for B35.

---

*Written 2026-07-29 for QUEUE.md batch B33 (enabler E6). Probe results in §1 were
established by compiling with `./target/release/ruxen compile` on that date, and
P1, P6, and P7 are the ones that chose the design. No implementation exists yet.*
