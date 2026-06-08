# Ruxen v1 issues found building the GUI stack (quiver / canvas / tally)

Catalog of compiler/toolchain bugs hit while implementing quiver (L2 reactive
GUI), the canvas SDL window path, and the tally demo app, 2026-06-06/07.
**Every repro below was run against ruxen 0.1.0 (`~/.ruxen/bin/ruxen`) and is
copy-paste verifiable** with `ruxen compile f.rx -o f && ./f` unless noted.

Severity key: **S1** silent miscompile (wrong values / memory corruption),
**S2** compiler crash, **S3** spurious compile error / parse gap,
**S4** toolchain / docs gap.

Partial fixes already exist locally (see "Existing partial work" at the end).

---

## Q1 · S1 — `&str`-vs-closure method overload misdispatches → heap corruption  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): both overload selectors (typeck
> `method_accepts_args` and MIR `method_signature_accepts_args`) now treat a
> CALLABLE argument — a closure literal (whose type may still be `Infer` at
> selection time) or a `Fn`/`any Fn`/`some Fn` value — as matching ONLY a
> callable parameter, never `&str`. A closure argument therefore selects the
> closure overload instead of the first-declared `&str` one (which stored the
> closure pointer as a String → heap corruption). Pin:
> `tests/release-e2e/cases/643_overload_str_vs_closure`. NOTE: testing this via
> the `ruxen` CLI is unreliable — it caches compiled artifacts by SOURCE hash,
> so a compiler-only change isn't re-lowered for an unchanged `.rx`; use the
> in-process release-e2e harness.


Two methods on one class sharing a name, one `&str` param, one `any Fn[...]`
param: calls dispatch wrongly; the program corrupts the heap and crashes later
(often while interpolating an unrelated `String`).

```ruxen
class Col
  labels: Array[String]
  computes: Array[any Fn[Fn() -> String]]
  def init
    self.labels = Array.new
    self.computes = Array.new
  end
  def var text(label: &str) -> nil                 # overload A
    self.labels.push(String.from(label))
  end
  def var text(f: any Fn[Fn() -> String]) -> nil   # overload B — same name
    self.computes.push(f)
  end
end

def main
  var c = Col.new
  c.text("Counter")           # should hit overload A
  if let Some(s) = c.labels.get(0)
    puts "label=[#{s}]"        # segfault / garbage
  end
end
```

Renaming overload B (e.g. `dyn_text`) fixes it — overload selection or the
call lowering is the culprit. Suspect: typeck overload resolution for
`&str` literals vs closure-typed args, or the arg coercion in MIR call
lowering. Workaround in quiver: distinct method names (`text` / `dyn_text`).

## Q2 · S1 — `Option[any Fn[...]]` class field returns garbage  ⏸ DEFERRED (closure redesign)

> **DEFERRED** (stdlib-rust-cleanup): root cause is that `any Fn` is a 16-byte
> fat value (data_ptr + vtable_ptr) but an enum payload slot is one 8-byte
> word, so half the fat pointer is lost on the round-trip (`f.()` reads a
> garbage pointer). Fixing it properly is entangled with the
> **closure/block redesign** below — storing closures as first-class `any Fn`
> VALUES is exactly the design being reworked toward Ruby semantics, so this is
> deferred to that plan rather than patched against the current model.

> ### DESIGN NOTE — closure/block model rework (draft-a-plan, another day)
> The current model treats a block as a first-class closure value with a typed
> `Fn() -> T` signature passed as an `any Fn` argument. Decision (user): move to
> **exact Ruby semantics** —
> - **exactly one** block per call, **implicit**, automatically the **last**
>   argument (`&block`);
> - the block is NOT a typed function value — it follows **`yield` / `block.call`**
>   and is rendered in place (no return-type inference, no fat-pointer value);
> - so blocks stop being stored/passed as `any Fn` values (which is what makes
>   Q2 and the fat-pointer enum-payload issue exist in the first place).
> This needs its own brainstorm + plan; not started.


```ruxen
class Holder
  compute: Option[any Fn[Fn() -> String]]
  def init { self.compute = nil }
end

def main
  var h = Holder.new
  h.compute = Some({ || String.from("hi") })
  match &h.compute
    Some(f) -> puts "got: #{f.()}"    # prints a garbage Int, e.g. 838718384
    nil     -> puts "none"
  end
end
```

The closure's fat representation does not survive the enum payload round-trip
through a field. Suspect: enum payload layout/size for `any Fn` (vtable +
env pair?) in MIR/codegen treats it as one word. Workaround: parallel arrays —
`computes: Array[any Fn[...]]` + `compute_of: Array[Int]` with `-1` sentinel.

## Q3 · S1 — `do…end` block to a FREE function with explicit closure param segfaults  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): in `resolve/exprs.rs`, a block-bearing free
> call was lowered as `runit.call(block)` (treating the function as a closure
> value) unless it was a yield-fn. Now a trailing block on ANY function callee
> is forwarded as the last argument (block-as-arg sugar → `FnCall`); the
> `.call` path is reserved for closure-typed VARIABLE identifiers. Subsumes the
> old yield-fn special case. Pin: `tests/release-e2e/cases/642_free_fn_do_block`
> (no-arg, plain `Fn`, and a block with params).


```ruxen
def runit(f: any Fn[Fn() -> nil]) -> nil
  f.()
end

def main
  runit do
    puts "plain"
  end                         # compiles; segfaults at runtime (exit 139)
end
```

All of these work: `runit({ || puts "plain" })` (braces), the same `do…end`
on instance/class **methods** (`r.go do … end`, `Runner.sgo do … end`),
`yield`-based implicit blocks, stdlib `.each do … end`. Only
free-function + explicit closure param + `do…end` is broken — the block
sugar lowering for that path passes a bad closure value. Also broken with
params (`withn do |n| … end`) and with plain `Fn() -> nil` (not just `any Fn`).

## Q4 · S1 — `move` closures cannot capture non-Copy class values (false E1001)  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): `borrow_check/walk.rs::check_closure` recorded
> the move-capture (correctly invalidating the outer binding) and then walked
> the body — where the body's own use of the captured value tripped a false
> E1001. The body owns the captured copy, so it must be live there. Fix:
> snapshot the move state, reinitialize the move-captured bindings for the
> body walk, then restore (so the outer binding stays moved after the
> closure). Pins (`tests/borrow_check_reborrow.rs`): body-use OK + a negative
> guard that using the value AFTER the closure still errors.


```ruxen
use std.sync.{Mutex, SharedSync}

class Handle
  cell: SharedSync[Mutex[Int]]
  def init(initial: Int) { self.cell = SharedSync.new(Mutex.new(initial)) }
  def get -> Int
    let m = self.cell.get
    let g = m.lock_raw
    g.get
  end
end

def main
  let h = Handle.new(42)
  let f = move { || puts "v=#{h.get}" }
  # error[E1001]: value used after move
  #   `h` moved into `closure` … `h` used here after move  (points INSIDE the body)
  f.()
end
```

`move` with Copy captures (Int) works (`tests/release-e2e/cases/91_move_closure.rx`
covers only that). Borrowck models the move-capture then flags the body's use
of the captured name. Non-`move` capture works (pointer-copy semantics) and is
what quiver relies on — but that in turn depends on Q22 (drop semantics).

## Q5 · S2 — every `as Float32` cast crashes the compiler  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): `coerce_value_signed` (cranelift `emit.rs`)
> now emits `fcvt_from_sint/uint` (int→float) and `fcvt_to_sint/uint_sat`
> (float→int); the `MirInst::Assign` handler picks the signedness source by
> direction (int→float = source operand, float→int = destination local) so
> negatives are correct; `mir/lower/expr/misc.rs` re-materialises any
> numeric↔numeric cast (not just int↔int) through that path. Pin:
> `tests/release-e2e/cases/635_numeric_casts_float` (covers signedness).


```ruxen
def takes_f32(x: Float32, y: Float32) -> nil
  puts "got #{x} #{y}"
end

def main
  let i = 21
  let a: Float32 = i as Float32     # Int -> Float32: compiler panic
  takes_f32(a, 1.5)
end
```

Compiler output (not a user diagnostic — a panic):

```
thread 'main' panicked at …cranelift-frontend-0.130.1/src/frontend.rs:509:21:
declared type of variable var2 doesn't match type of value v2
```

Also crashes: `(i as Float) as Float32` (Float→Float32 narrowing), with or
without a type-annotated `let`, in let-position or argument-position. Only
`f32` literals (`1.0f32`) and f32 arithmetic work. Note `Float32 as Float`
(widening) DOES work (canvas uses it). Suspect: `ty_to_cranelift`/cast
lowering emits an i64→f32 conversion under a variable declared f32 without
the convert instruction. Workaround (tally): unit-addition loop —
`var f = 0.0f32; while i < n { f = f + 1.0f32; … }`.

## Q6 · S1 — arithmetic inside string interpolation miscomputes  ✅ FIXED (pinned)

> **ALREADY FIXED** on this branch (master merge); was reproducing as a bug
> on the released 0.1.0. Verified `#{w / 2} #{h / 2}` prints `56 84`. Added a
> regression pin so it can't silently break again:
> `tests/release-e2e/cases/636_interp_arithmetic`.


```ruxen
def main
  let w = 112
  let h = 168
  puts "P3\n#{w / 2} #{h / 2}\n255"
  # expected "56 84" — printed "112" (division result wrong/joined)
end
```

Verified in tally's PPM header: `"#{cv.width / 2} #{height_px / 2}"` printed
`112` instead of `56 84`. Hoisting to `let half_w = cv.width / 2` fixes it.
Suspect: interpolation segment parsing splits on `/` or the embedded
expression lowering. (Method calls and plain idents in `#{}` work fine.)

## Q7 · S1/S3 — brace-block match arms parse as closures; stale captures when executed  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): per the decision, `-> do … end` is now the
> multi-statement block-arm form (`{ expr }` stays the single-expression arm).
> `parse_match_arm` parses a `do` right after `->` as a statement block
> (`parse_body` + `end`) instead of letting the expression parser take it as a
> closure literal (the `Fn() -> T` type error). A block arm is not a closure,
> so it sees surrounding bindings live — the stale-capture manifestation is
> gone too. Pin: `tests/release-e2e/cases/641_match_block_arm`.


Two manifestations:

1. Type error in expression arms:
   ```ruxen
   match self.subs.get(id)
     Some(scopes) -> {            # error: expected `()`, found `Fn() -> ()`
       for s in scopes.to_a
         self.dirty.insert(s)
       end
     }
     nil -> nil
   end
   ```
2. When such an arm DOES execute as a block (statement position), captured
   variables go stale: in canvas's `tests/window_live.rx` the arm
   `Ok(_) -> { match w.show … w.hide }` observed `w`'s field writes from
   before the match as **unset** (a second `w.show` re-ran its full body with
   `windowed=false`), and the test failed in ways the inline rewrite did not.

Multi-statement arms need a real grammar decision (block arms vs mandatory
`if let`/helper-fn). Until then: never write `-> { … }`; use `if let` or a
named helper. Both quiver and canvas now carry comments warning about this.

> **DECISION** (user, stdlib-rust-cleanup): `{ … }` stays the **single-line /
> single-expression** arm form; **`-> do … end` is the multi-statement block
> arm**. Plan: make `do … end` in arm-body position parse as a statement
> block (leave `{` for the single-expression case so the closure ambiguity
> never arises), and fix the stale-capture codegen bug for executed blocks.

## Q8 · S2 — recursive class types crash the compiler (stack overflow)  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): two type-walks recursed through the
> self-reference with no cycle guard — the structural auto-derive check
> (`resolve/symbols.rs::ty_has_derive_trait`, typeck) and the layout/size
> computation (`codegen/layout.rs`). Both now carry a visited-set: the
> derive check returns `true` on a cycle (coinductive — a recursive type
> auto-derives iff its non-recursive parts do, like Rust); the layout
> returns pointer-size on a cycle (a class instance is a heap pointer, and
> `alloc_size` sizes slot-by-slot anyway). Recursive classes now compile
> AND run. Pin: `tests/release-e2e/cases/638_recursive_class` (Array[Node]
> + Option[Node] self-ref, field access through the recursion).


```ruxen
class Node
  label: String
  children: Array[Node]        # Self-referential field
  def init(@label: String) { self.children = Array.new }
end

def main
  var root = Node.new(String.from("root"))
  root.children.push(Node.new(String.from("kid")))
  puts "kids: #{root.children.size}"
end
```

```
thread 'main' has overflowed its stack
fatal runtime error: stack overflow, aborting
```

Classes are heap/pointer values, so `Array[Self]` should be representable;
at minimum this must become a cycle-detected diagnostic. Suspect: type
layout/size computation recursing without indirection through class
references. Workaround (quiver): flat arena — parallel arrays indexed by
Int node ids.

## Q9 · S3 — `arr[identifier]` parses as a generic-argument list; no index assignment  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): the non-literal-index READ (`xs[i]`) was
> already fixed on this branch; index ASSIGNMENT was compiling but silently
> no-op'ing (it fell to the skip arm in `mir/lower/expr/assign.rs`). Now
> `xs[i] = v` lowers to the bounds-checked `ruxen_vec_set` (and a
> fixed-array literal index to a direct slot store), mirroring the read
> path. Map `m[k] = v` still uses `.insert` (unchanged). Pin:
> `tests/release-e2e/cases/637_array_index_assign` (read + write, literal +
> non-literal index).


```ruxen
def main
  let xs = [10, 20, 30]
  let i = 1
  puts "#{xs[0]}"     # OK — literal index
  puts "#{xs[i]}"     # error: expected type, found Identifier("i")
end
```

`expr[expr]` with a non-literal index is consumed by the generics parse.
Separately, there is **no index assignment** at all (`xs[0] = 9` has no
lowering; Array has no `set` FFI either). Workarounds: `.get(i)` (returns
`Option[&T]`) for reads; push-only arrays or `Hash[Int, V].insert`
(insert overwrites) for writes.

## Q10 · S3 — doc comments between signature-only defs in a mixin break parsing  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): `parser/classes.rs::parse_trait_item` now
> treats a `DocComment` after a bodiless signature as a signature terminator
> (it floats forward to the next item). Pin:
> `parser::tests::mixin_doc_comment_between_signatures`.


```ruxen
mixin Surface
  ## comment before first sig — OK
  def a(x: Int) -> nil
  ## comment between sigs — error: expected expression, found DocComment
  def b(x: Int) -> nil
end
```

The parser starts parsing a *body* for the first bodiless `def` and chokes.
Workaround: hoist all docs above the `mixin` keyword.

## Q11 · S3 — Hash tuple-iteration values don't type-resolve for method calls  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): two layers. (1) typeck — the for-loop now
> propagates the `(K,V)` element-tuple component types to the destructured
> sub-bindings (`infer/expr.rs`), so `scopes` is typed (no more `?T_remove`).
> (2) MIR — `for_loop.rs` routed a `Hash` iterable through `ruxen_hash_entries`
> (Array[(K,V)]) instead of mis-reading it as a Vec, and replaced the stale
> `enumerate`-shaped sub-binding hack with real tuple-field extraction
> (`GetField`), so `for (k,v) in &map` / `for (a,b) in pairs` both destructure
> correctly. Pin: `tests/release-e2e/cases/639_for_tuple_destructure_map`.


```ruxen
# self.subs: Hash[Int, Set[Int]]
for (sid, scopes) in &self.subs
  scopes.remove(scope)
  # codegen: no runtime symbol for `?T::remove` (mangled `?T880_remove`)
end
```

The value binding stays an inference variable all the way into codegen
(should also fail earlier, in typeck, with a real diagnostic). Workaround:
`for sid in self.subs.keys` + `if let Some(scopes) = self.subs.get(sid)`.

## Q12 · S3 — a closure passing its `&var T` param as an argument twice: false move error  ✅ FIXED

```ruxen
# u: &var Ui closure param; flag/a: State handles whose .get takes &var Ui
{ |u: &var Ui|
  if flag.get(u) > 0     # error[E1001] on the SECOND use:
    "a=#{a.get(u)}"      #   value moved into `get` … used here after move
  else
    "b=#{b.get(u)}"
  end
}
```

Calling **methods** on `u` repeatedly works (`u.bump; u.bump` fine); only
passing the reference **as an argument** moves it. Explicit reborrow works:
`flag.get(&var *u)`. Decide whether implicit reborrow of reference-typed
params is intended (Rust's behavior); if yes fix borrowck, if not, keep the
rule but make the diagnostic suggest `&var *x`.

> **DECISION** (user, stdlib-rust-cleanup): **YES — implicit reborrow**
> (Rust-like). A reference-typed param passed as an argument is reborrowed,
> not moved. Fix borrowck so `flag.get(u)` then `a.get(u)` is clean.

## Q13 · S3 — zero-arg method + `?` parses as a field access  ✅ FIXED (diagnostic)

```ruxen
cv.begin_frame?      # error: no field `begin_frame?` on type `Canvas`
cv.begin_frame()?    # OK
```

The lexer/parser folds `?` into the member name (Ruby-style predicate names
like `empty?` exist, so this is ambiguous by design — but a `Result`-typed
zero-arg method followed by `?` should try the try-operator parse too, or the
error should hint at adding `()`).

> **DECISION** (user, stdlib-rust-cleanup): keep the **Ruby-like rule** that
> already exists (`?` = predicate-method name, `&.` = safe navigation,
> `foo()?` = try-operator — `ruby-naming.spec.md` §"safe navigation"). No
> operator change. Fix = **diagnostic only**: when `obj.foo?` resolves to
> no field/method, hint "did you mean `obj.foo()?` (try-operator) or
> `obj&.foo` (safe navigation)?".

## Q14 · S3/S1 — flat global symbol namespace: user classes collide with std  ✅ FIXED (diagnostic)

> **FIXED (resolve-time diagnostic)** (stdlib-rust-cleanup): a user top-level
> class whose name matches an auto-loaded built-in/stdlib type (e.g. `Signal`,
> `Runner`) now emits a clear **E0727** at resolve time ("type `Signal`
> collides … rename your type") instead of the late, cryptic codegen
> `DuplicateDefinition("Signal_clone")`. Detected in
> `resolve/ffi_registration.rs` when a non-bootstrap top-level class name is
> already a type in scope (collection-builtin anchors and module-nested
> classes excluded). Pin: `tests/type_name_collision.rs` + `docs/errors/E0727.md`.
> NOTE: this is the "at minimum a diagnostic" fix; full per-package symbol
> namespacing (so the names can coexist) remains the larger follow-up.


```ruxen
use std.sync.{Mutex, SharedSync}

class Signal[T: Send]      # std.sync also exports `Signal`
  cell: SharedSync[Mutex[T]]
  def init(initial: T) { self.cell = SharedSync.new(Mutex.new(initial)) }
end

def main
  let s = Signal[Int].new(0)
end
# Error: Failed to define function 'Signal_clone': DuplicateDefinition("Signal_clone")
```

Same for `class Runner` (std.test.Runner) — any std class name is reserved,
in single files AND in packages. This forced quiver's `Signal[T]` → `State[T]`
rename. Smallest sound fix: package-prefix the generated symbol names
(`quiver$Signal_clone`), keeping the flat resolver; full `use pkg.X`
namespacing is the bigger fix (see the B12 note in
`src/ruxen_cli/src/build.rs::compile_project`). At minimum: a resolve-time
diagnostic instead of a codegen `DuplicateDefinition` panic.

## Q15 · S3 — module-wrapped generic classes lose field resolution  ✅ FIXED (core)

> **FIXED** (stdlib-rust-cleanup): a module-nested class is registered under
> its QUALIFIED name (`Quiver.Signal`), but `self_ty` was built from the bare
> `class.name`, so `self.value` in a method body typed `self` as `Signal` —
> which no registered class matched ("no field value on type Signal").
> `resolve_class` now builds `self_ty` from the same `qualified_key`. Generic
> and non-generic module classes resolve their fields + methods. Pin:
> `tests/release-e2e/cases/646_module_generic_class_field` (constructed via
> `Quiver.Signal.new(42)`).
>
> Two NARROWER construction sub-gaps remain (separate from field resolution):
> the module-qualified turbofish `Quiver.Signal[Int].new` ("undefined enum
> variant") and `use Quiver.Signal; Signal.new` (the imported alias keeps the
> bare name, so method lookup misses). Tracked; the documented field-resolution
> failure is resolved.


```ruxen
module Quiver
  class Signal[T: Send]
    value: T
    def init(@value: T) end
    def get -> T
      self.value      # error: no field `value` on type `Signal`
    end
  end
end
```

Known resolver gap (same note in build.rs: nested classes don't propagate
field DefIds into method-body scope). Fixing this would also unlock module
namespacing as a Q14 workaround.

## Q16 · S4 — library builds, `ruxen check`, and `ruxen test` can't see dependency symbols  ⏳ PLANNED (dedicated)

> **STATUS** (stdlib-rust-cleanup): not yet fixed — needs a build-driver change
> validated against multi-package fixtures (not single-file e2e cases), so it's
> scoped to a dedicated pass rather than rushed here.
> **Plan:** in `src/ruxen_cli/src/build.rs`, extract the dep-source flat-merge
> (currently inline in `compile_project`, lines ~578-587) into a
> `gather_dep_sources(&[PathBuf]) -> String` helper; thread the resolved dep
> source dirs into `compile_piece` (the library-build path) and into `check`
> (which today gathers only its own sources); and give the test runner a
> path-dep source gatherer. **Acceptance:** lib A exporting `struct Color`; lib
> B with `A = { path = "../A" }` using `Color` in `src/lib.rx` — `ruxen build`,
> `ruxen check`, `ruxen test` all pass in B (add a `tests/dep_visibility.rs`
> modeled on `package_manager.rs`).


Only **binary** builds flat-merge dependency sources
(`src/ruxen_cli/src/build.rs::compile_project`). The library path
(`compile_piece`), `check`, and the test runner
(`src/ruxenc/src/test_runner.rs::gather_project_lib_sources` — explicitly
"does not resolve project deps") do not. Consequence: quiver (library,
depends on canvas by path) cannot reference any canvas symbol in `src/` or
`tests/`; its canvas adapter must live in an example **binary** package.

Acceptance test for the fix: lib A exporting `struct Color`; lib B with
`A = { path = "../A" }` using `Color` in `src/lib.rx` — `ruxen build`,
`ruxen check`, and `ruxen test` must all pass in B.
**Partial fix exists locally** — see "Existing partial work" below.

## Q17 · S4 — cross-package generic monomorphization fails for consumer types  ⏳ PLANNED (dedicated)

> **STATUS** (stdlib-rust-cleanup): not yet fixed — this is codegen-deep
> (monomorphizing a dependency's generic body for a type defined in the
> CONSUMING package) and is the hardest of the catalog; it needs the same
> multi-package test scaffolding as Q16 and careful work on the
> generic-instantiation collection across package boundaries
> (`mir/lower/monomorphize.rs` + the build driver's per-piece compile). Scoped
> to a dedicated pass with Q16 (they share the multi-package layering story and
> Q14's "full namespacing" follow-up). Rushing it risks emitting bound-placeholder
> symbols (`S: PaintSurface_fill_rect`) that link-fail — exactly the bug.


A dependency's generic function (`def paint[S: PaintSurface](s: &var S)`)
called with a type defined in the CONSUMING package fails to link:

```
undefined reference to `S: PaintSurface_fill_rect'
```

— the unmonomorphized generic body is emitted with the bound placeholder as
the symbol. Works when the instantiating type lives in the same package as
the generic (quiver's own `RecordingSurface` links fine from an app).
Consequence: apps cannot implement a dependency's mixin and pass it to the
dependency's generics — tally/examples carry a duplicated paint loop instead.

## Q18 · S4 — test-file synthesis gaps (`ruxen test`)  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): `synthesise_wrapper` wrapped the WHOLE test
> file in `def main`, so top-level `def`s (their `end` closed `main` early),
> `use` lines, and `##` doc comments broke. New `split_test_body` hoists
> column-0 top-level items — `use`/`const`/`type` (single-line) and
> `def`/`class`/`struct`/`enum`/`mixin`/`module`/`extension`/`impl` blocks
> (through their column-0 `end`) — ABOVE the synthesised `def main`, keeps the
> `Tester.describe …` statements inside it, and neutralises stray `##` docs to
> `#`. Pin: `test_runner::tests::split_test_body_hoists_top_level_items`.


The runner wraps each test file's body in a synthesized `def main`
(`src/ruxenc/src/test_runner.rs::synthesise_wrapper`), so:

- **Top-level `def`s in a test file break parsing** (the def nests inside
  `main`; its `end` closes `main` early). Helpers must be `let` closures
  inside `Tester.describe`.
- **`##` doc comments at the top of a test file** end up as statements
  inside `main` → "expected expression, found DocComment". Plain `#` works.
- **`use <own-package>.{…}` in a test file breaks** ("expected expression,
  found Use" mid-file) — own-package symbols are already merged; the line
  must be omitted.

## Q19 · S4 — `Stdin.new` doesn't link; tutorial shows the wrong API  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): (1) typeck now emits a clear error for `.new`
> on a class with NO constructor — no `init`, no `new` static (FFI alias), no
> fields, no parent (the FFI-handle case, e.g. `Stdin`/`Stdout`/`Stderr`) —
> instead of the late "undefined `_Stdin_init`" linker error, suggesting the
> free function (`stdin()` etc.). Both `Type.new` (field-access) and
> `Type.new()` (call) forms are covered. Classes with a real `def self.new` FFI
> constructor (e.g. `OpenOptions`) are unaffected. (2) `docs/tutorial/31-io-and-cli.md`
> now uses `stdin()`. Pin: `tests/stdin_new_diagnostic.rs`.


`let s = Stdin.new` → `undefined reference to 'Stdin_init'`. The working API
is the free fn `stdin()` (`library/std/io`). But
`docs/tutorial/31-io-and-cli.md` line ~263 says `let stdin = Stdin.new`.
Either synthesize a default init for FFI-only classes (or error at typeck),
and fix the tutorial.

## Q20 · S3 — `T: Send` bound required for `Mutex[T]`/`SharedSync[T]` construction in generics  ✅ FIXED (diagnostic)

> **FIXED** (stdlib-rust-cleanup): the E1101/E1102 message now detects when the
> non-`Send`-ness traces to unbounded generic parameters nested in the payload
> (`SharedSync[Mutex[T]]` → `T`) and tells the user to add the bound where it's
> declared (`[T: Send]` on the enclosing class/function), instead of the
> misleading "add `include Send` to the class" (which pointed at `Mutex`, not
> editable). A concrete non-`Send` payload keeps the include-Send guidance.
> Pin: `tests/send_bound_hint.rs`.


`class Cell[T] { cell: SharedSync[Mutex[T]] … }` fails with
`[E1101] cannot construct Mutex[T] — payload type T is not Send` until the
class is declared `class Cell[T: Send]`. Probably by design — listed here
because the diagnostic doesn't say *where* to add the bound (the error points
at the construction site, not the class header).

## Q21 · S3 — phantom-generic struct constructors don't infer  ✅ FIXED

> **FIXED** (stdlib-rust-cleanup): `infer_class_generics` now (a) handles
> structs as well as classes, (b) falls back to the declared fields when there
> is no `init`, and (c) mints a FRESH inference var for any generic param not
> determined by a constructor argument (a phantom param) instead of collapsing
> to the bare head — so the call's expected type (`-> Sig[T]`) binds it.
> `infer_constructor_call` now dispatches on both `Ty::Class` and `Ty::Struct`.
> Pin: `tests/release-e2e/cases/640_phantom_generic_struct_new`. (Full generic
> *struct methods* remain a separate, larger item; this fixes `.new` inference.)


```ruxen
struct Sig[T]
  id: Int
end

def make[T](id: Int) -> Sig[T]
  Sig.new(id)      # error: type mismatch: expected `Sig[T]`, found `Sig`
end
```

Unused type params don't propagate through `.new`. (Related, documented
elsewhere: generic **struct** methods aren't supported at all — quiver's
NOTE in its scaffold; classes must be used instead.)

## Q22 · design note — closure captures are pointer-copies; drop semantics will break them

Non-`move` closures capture class values by copying the pointer, and the
capture stays valid after the binding's scope exits **only because drops
don't run yet** (v1 leaks). quiver's stored `dyn_text`/handler closures rely
on this. When deterministic drop lands, captured handles dangle — the
capture/keep-alive story (Q4 is a prerequisite: owning captures must work)
needs a language-level answer before Drop ships. This is the
"signals + ownership ergonomics" risk DESIGN.md predicted; it is now
concrete.

### ✅ VERDICT (2026-06-08, post P0.2 audit): SOUND today — bounded leak, NOT a UAF. S4 (was framed as latent S1).

Drop elaboration now runs (P0.2 resolved — see `docs/decisions/drop-elaboration.md`).
I traced the exact interaction. A captured class local is **NOT freed** at the
capturing frame's scope exit, so the stored closure's copied pointer does **not**
dangle. Mechanism, end to end:

- Closure lowering writes every capture (ByValue **and** ByRef, `move` and
  non-`move`) with a single `MirInst::SetField { base: cap, value: Use(outer_local) }`
  — `compiler/ruxen_core/src/mir/lower/expr/closure.rs:136-150`. There is no other
  path into the captures block.
- The drop-elaboration ownership analysis `compute_dealloc_safe_locals`
  (`mir/lower/drops.rs:728-734`) treats `SetField { value: Use(l) }` as an ownership
  transfer into the aggregate: it inserts `l` into `tainted_perm` and removes it from
  `alloc_rooted`. `tainted_perm` is **append-only** (every reference is `.insert`;
  there is no `.remove` anywhere in the file), so the taint is permanent.
- A tainted local is excluded from `drop_locals`, so `insert_drops` emits neither
  `ruxen_dealloc` nor a `{Class}_drop` for it. The capture outlives the frame.

This reconciles with quiver's green suite (42/42, exercises capture + the counter
handler): nothing crashes because the captured handle is never freed. The cost is a
**leak** — the captured value lives until process exit, exactly as in pre-Drop v1.
So the "deterministic teardown / no-GC" win does **not** yet extend to values that
escape into stored closures; they are kept alive by omission, not by ownership.

**Not the next Drop increment.** Because it is sound (no double-free / UAF), it does
not gate the conditional-move drop-flags work. The real fix is the **owning-capture /
keep-alive** story (Q4 prerequisite): give stored closures explicit ownership (move
capture that actually transfers + frees on closure drop, or a shared/Rc handle) so
captured values are freed deterministically instead of leaked. That is a
language-design increment, tracked separately, not a soundness hotfix.

**What quiver should change in its landmine note (a separate agent owns quiver/):**
flip the "sound today *because drops don't run yet*" framing — it is now sound
*because the capture's `SetField` permanently taints the local out of the drop set*,
i.e. captured handles **leak** rather than dangle. No quiver code change is required
for safety; the open item is that escaped-into-closure handles are not yet
deterministically freed (intentional leak), pending the owning-capture design.

## Q23 · S4 — `ruxen fmt` strips `##` doc comments and cannot parse test files  ⏳ OPEN

Surfaced 2026-06-08 building canvas `draw_path`. Two distinct `ruxen fmt` faults,
both currently making the formatter **unsafe to run** — which matters because the
project convention (and the app `.claude` Stop hooks) tell contributors to run it:

1. **Doc-comment destruction.** A repo-wide `ruxen fmt` strips every `##` doc
   comment and collapses methods to brace form. In canvas it wiped all 86 `##`
   doc comments from `src/canvas.rx`. Round-trip is lossy.
2. **Can't parse `Tester.describe` blocks.** `ruxen fmt` fails on any file whose
   top level is a `Tester.describe(...) do ... end` block — i.e. every
   `tests/*.rx`. Minimal repro: a file containing only
   `Tester.describe("d") do |t: &var Tester| ... end` → `ruxen fmt` errors
   `expected top-level declaration, found TypeIdentifier("Tester") (at 1:1)`,
   while `ruxen check`/`ruxen test` accept the same file.

Severity S4 (toolchain/DX), but high-friction: until fixed, format `.rx` by hand.
The formatter's parser must round-trip `##` doc comments and accept the same
top-level forms the main parser does (notably a method/`describe` call with a
trailing `do…end` block at module top level).

## Q24 · S4 — stale incremental cache replays false move/borrow diagnostics  ⏳ OPEN

Surfaced 2026-06-08 (canvas), and the same mechanism amplified the Q18 stale-
toolchain confusion. `ruxen build`/`ruxen test` can replay **poisoned** move/borrow
errors (`E1001`/`E1009`) with bogus line numbers from a *prior* compile, even
across unrelated directories — i.e. a cached diagnostic from one build surfaces in
another. `ruxen check` is always correct (it doesn't hit the stale cache path).
Workaround: `rm -rf target/ruxen/incremental target/ruxen/test-build` clears it.
Fix: key the incremental/diagnostic cache on the actual source+toolchain identity
so a stale entry can't be replayed; never surface a cached diagnostic whose source
span no longer matches.

## Q25 · S1 — `Hash.key?`/`Hash.get` on an EMPTY hash SEGFAULTs; `&Hash`/`&Set` params unsound  ⏳ OPEN

Surfaced 2026-06-08 building quiver's arena nesting. Two faults:

1. **Empty-hash lookup segfaults.** `.key?`/`.get` on a `Hash` with zero entries
   SIGSEGVs (any value type). `ruxen check` + `ruxen build` pass; the binary
   exits 139 on the first lookup. `.size` is always safe (returns 0). Once the
   hash has ≥1 entry, `.key?`/`.get` work, including for ABSENT keys. Repro:
   ```ruxen
   def main
     var h: Hash[Int, Int] = Hash.new
     let k = h.key?(9)   # SIGSEGV — hash is empty
   end
   ```
2. **`&Hash[...]` / `&Set[...]` parameter types are unsound** — they're mixins
   without runtime dispatch. A *free fn* errors `E1118`; a *method* with a
   `&Hash` param **compiles then segfaults** (silent miscompile).

Workaround in quiver: inline every hash accessor (no `&Hash` params) and guard
every lookup behind `if h.size as Int > 0`. Real fixes: bounds-check the empty
backing table in `key?`/`get` (return false/None, never deref a null bucket
array), and either reject `&Hash`/`&Set` params on methods too (consistent with
the free-fn `E1118`) or give the mixins runtime dispatch.

## Q26 · S1 — a capturing closure STORED under a `&var *self` reborrow loses its captures  ⏳ OPEN — blocks reactive nested widgets

Surfaced 2026-06-08 (quiver `Row`/`Col` containers). A builder method runs a
user block by reborrowing its own receiver (`build.(&var *self)`); a closure
that captures an outer value and is **stored** inside that block reads its
capture as garbage when later invoked — wrong `Int`, or SIGSEGV for a captured
class handle. `check`+`build` pass. Minimal repro:
```ruxen
class Box
  fns: Array[any Fn[Fn() -> Int]]
  def init; self.fns = Array.new; end
  def var add(f: any Fn[Fn() -> Int]) -> nil; self.fns.push(f); end
  def var build(b: any Fn[Fn(&var Box) -> nil]) -> nil; b.(&var *self); end  # self-reborrow
  def call0 -> Int
    match self.fns.get(0); Some(f) -> f.(); nil -> -1; end
  end
end
def main
  let v = 42
  var box = Box.new
  box.build({ |c: &var Box| c.add({ || v + 1 }) })   # through reborrow
  puts "#{box.call0}"   # prints 1, expected 43 — capture v read as 0
end
```
Calling `box.add({ || v + 1 })` directly (no reborrow) gives the correct 43.
Related to Q12 (reborrow) and Q22 (capture is a pointer-copy), but the failure
mode is distinct: captures are corrupted specifically when the storing closure
is reached *through* a `&var *self` reborrow frame.

**Impact (why this is high-priority for the GUI stack):** quiver's `row`/`col`
containers build children via `build.(&var *self)`. Static `text` children
(no capture) work, but **reactive `dyn_text`/`button` children inside a
container are blocked** — the stored compute captures the `State` handle and
segfaults on invocation. `tests/nesting.rx` keeps the reactive-child assertion
as `xit` pending. This gates the widget library's whole point (reactive nested
widgets), so it ranks with Q16/Q17 on the GUI critical path. `App.build` is
unaffected (it passes `&var local.field`, not a self-reborrow).

---

## Existing partial work on this machine (`~/Documents/ruxen-lang/`)

Stopped mid-flight (resource limits) — useful starting points, all LOCAL:

| Where | Branch | State |
|---|---|---|
| `ruxen-ws-namespace/` (worktree of ruxen) | `fix/namespacing-deps` | **Q16 partial**: `src/ruxen_cli/src/build.rs` modified + new `src/ruxen_cli/tests/dep_visibility.rs` (uncommitted). Plan recorded: extract `compile_project`'s dep-source flat-merge into a helper, reuse for `compile_piece`/`check`; lightweight path-dep gatherer for the test runner. |
| `ruxen-ws-codegen/` (worktree) | `fix/codegen-miscompiles` | empty — brief written for Q1–Q4 (see this doc) |
| `ruxen-ws-parser/` (worktree) | `fix/parser-crashes` | empty — brief written for Q8–Q12 |
| `quiver-wire/` (worktree of quiver) | `feat/canvas-window` | partial `examples/counter` window wiring (superseded by tally + quiver work on master) |
| `ruxen` main checkout | `closure-fixes` | committed struct/enum codegen fixes (`18df435`) + this document |

Recommended order: **Q16 → Q17** (unblocks the layering story), then
**Q1/Q2/Q3/Q5** (silent miscompiles, all small repros), then **Q8/Q9** (DX),
then **Q14/Q15** (namespacing, biggest), with **Q4/Q22** as the
language-design pair to settle before Drop semantics land.

## Where the workarounds live (grep-able)

- `quiver/CLAUDE.md` — "Ruxen v1 landmines" section (the working ruleset)
- `quiver/docs/DSL.md`, `quiver/docs/REACTIVITY.md` — API deviations + why
- `tally/src/main.rx` — `to_f32` (Q5), PPM-header hoist (Q6), plain paint
  loop (Q17), `stdin()` (Q19)
- `canvas/tests/window_live.rx` — brace-arm rewrite (Q7)
- `canvas/src/lib.rx` `show_window` — the `&String` FFI calling shape note
