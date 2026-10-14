> Historical document. It predates the Ruby-only direction and may show Rust syntax that Ruxen no longer uses.

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

## Q1 · S1 — `&str`-vs-closure method overload misdispatches → heap corruption  ✅ FIXED (and DISSOLVED 2026-06-11)

> **DISSOLVED by the `&str` removal (2026-06-11, one-string-type ADR).** With
> exactly one string borrow type (`&String`), there is no `&str` arm left to
> collide with a closure arm in overload selection — the entire class of bug is
> structurally impossible now. The overload fixture `643_overload_str_vs_closure`
> was swept to `&String` and still pins the sound behavior. The original fix
> (below) remains the defensive backstop.
>
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

## Q2 · S1 — `Option[any Fn[...]]` class field returns garbage  ⏸ STILL OPEN (scoped, independent of blocks)

> **STILL OPEN — and explicitly NOT fixed by the Ruby-block work** (2026-06-10).
> Root cause is that `any Fn` is a 16-byte fat value (data_ptr + vtable_ptr) but
> an enum payload slot is one 8-byte word, so half the fat pointer is lost on the
> round-trip (`f.()` reads a garbage pointer). The Ruby-block-semantics ADR
> (`docs/decisions/ruby-block-semantics.md`) was the redesign this was deferred
> behind — but the ADR's block slot uses the **8-byte closure-pair-pointer +
> null sentinel** representation (ADR D1), which sidesteps the fat-value enum
> payload entirely and so does NOT touch this path. Q2 is therefore independent:
> it is the residual `any Fn`-in-enum-payload LAYOUT bug, to be fixed on its own
> (widen the enum payload slot to 16 bytes for fat-pointer payloads, or box the
> `any Fn`). Blocks no longer depend on it.

> ### DESIGN NOTE — closure/block model rework  ✅ DONE (Ruby-block-semantics ADR)
> The block redesign that this note sketched has LANDED — see
> `docs/decisions/ruby-block-semantics.md`. Delivered: explicit optional
> `&block: Fn[(T…) -> R]` (canonical square-bracket spelling, paren form kept for
> back-compat), `yield` / `yield(args)` with the block's value typed `R`,
> `block_defined?` / `block_given?`, optionality with a clean LocalJumpError-style
> runtime panic on blockless `yield`, and a single `do…end`/`{ }` attachment rule
> for free fns and methods. The block is the 8-byte closure-pair-pointer (NOT an
> `any Fn` value), so the fat-pointer enum-payload issue that motivated this is no
> longer in the block path.


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
>
> **Migration-wave follow-up (2026-06-10):** quiver's `CLAUDE.md` landmine list
> still warns "do…end blocks segfault when passed to free functions with explicit
> closure params". That landmine is FIXED (this Q3 + pin 642) and re-confirmed
> green against a fresh build during the Ruby-block-semantics work. The migration
> wave should DELETE that landmine entry from `quiver/CLAUDE.md` — it is stale.


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

## Q16 · S4 — library builds, `ruxen check`, and `ruxen test` can't see dependency symbols  ✅ FIXED

> **FIXED** (feat/drop-elaboration, 2026-06-08): the dep-source flat-merge that
> only the BINARY path performed (`compile_project`) is now a shared helper
> `build::gather_dep_sources(&[PathBuf]) -> String`, reused by all four build
> kinds:
> - **library** (`compile_piece`) — flat-merges the project's full dependency
>   closure ahead of its own source; a dependency's own (transitive) deps are
>   flat-merged when building that dependency's rlib
>   (`transitive_dep_source_dirs`).
> - **`check`** — resolves deps via the new shared `build::resolve_dep_source_dirs`
>   and prepends their sources before type-checking.
> - **`ruxen test`** — `ruxen_cli`'s `main.rs` resolves the dep dirs (the
>   resolver lives there; `ruxenc` only dev-depends on `ruxen_cli`) and threads
>   them into `TestOptions::dep_source_dirs`; the runner flat-merges them ahead
>   of the project's own lib source in each synthesised wrapper.
>
> **Soundness:** dependency symbols enter by SOURCE flat-merge (one object, one
> definition per symbol), never by extern-rlib link, so there is no
> duplicate-symbol / double-link risk and binary builds are byte-for-byte
> unchanged (skip-extern-link when deps are merged is preserved). Design:
> `docs/decisions/q16-dep-symbols-in-lib-check-test-builds.md`. Pins:
> `src/ruxen_cli/tests/dep_visibility.rs` (two-package fixture: `dep-color`
> exposing `struct Color`; `consumer` library `use`-ing it in `src/lib.rx` and
> `tests/color_test.rx` — `build`/`check`/`test` all green) and
> `test_runner::tests::synthesise_merges_dependency_source_before_project_and_main`.
> Not covered (sound boundary): namespacing (`use <pkg>.X` is still flat — Q14);
> a missing transitive symbol surfaces as a normal typeck error, never a
> miscompile.


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

## Q17 · S4 — generic-free-function / mixin-bound monomorphization for consumer types  ✅ FIXED (free fns + generic-calling-generic) · generic METHODS staged

> **FIXED for generic FREE FUNCTIONS** (feat/drop-elaboration, 2026-06-10).
> **Empirical re-scope first:** after Q16 flat-merged dep `src/**.rx` into the
> consuming unit, this is NO LONGER a cross-PACKAGE problem — it reproduces in a
> SINGLE FILE. The real defect is in MIR lowering: a generic free function bound
> by a mixin (`def paint_all[T: Paintable](s: &var T, …)`) was lowered ONCE with
> `T` abstract, so the bound method call inside it (`s.fill_rect(…)`) mangled to
> the bound-placeholder callee `T: Paintable_fill_rect`, which link-failed. The
> single-implementor case was masked because mixin dispatch devirtualized
> (`unique_bound_impl`) to the sole impl — exactly why quiver was capped at ONE
> `PaintSurface` implementor.
>
> **Design** (`docs/decisions/q17-cross-package-monomorphization.md`): a new
> demand-driven pass in `mir/lower/monomorphize.rs` collects every concrete
> instantiation of an eligible generic free fn (recovered by unifying declared
> param types against the call's actual arg types), emits one specialized body
> per instantiation (`paint_all__mono__TallySurface`) via the existing
> `subst_type_params_in_func`, and redirects call sites (`fn_call.rs::
> fn_mono_callee`). Generic-CALLING-generic is handled by a WORKLIST FIXPOINT
> that re-scans each substituted body for further generic calls. The opaque
> (un-monomorphized) body of every eligible generic free fn is suppressed (it
> could only emit placeholders); the single-implementor case monomorphizes to
> one concrete copy (byte-equivalent to the old devirtualize path). Both
> backends consume the same MIR — no backend-specific work.
>
> **Acceptance** (quiver's exact blocker): a consumer binary defines a SECOND
> `Paintable` implementor and runs the dep's generic against BOTH the dep's
> `RecordingSurface` and the consumer's type in one program, printing distinct
> correct values from each. Pins:
> `src/ruxen_cli/tests/cross_package_mono.rs` (staged-install, two-package:
> binary AND `ruxen test`, compile+run+assert stdout `dep=20 mine=9`),
> `compiler/ruxen_core/tests/q17_generic_fn_mixin_mono.rs`, and release-e2e
> cases `655` (two implementors) / `656` (generic-calling-generic) / `657`
> (mixin default body) / `658` (single-implementor regression bar).
>
> **STAGED REMAINDER — generic METHODS over a mixin.** A generic `def` INSIDE a
> class (`def measure[T: Sized](item: &var T)`) is NOT yet monomorphized: it
> still resolves the bound method to a placeholder. This is now a CLEAR lowering
> ERROR (`method_call.rs` / `field_access.rs` guard: "cannot monomorphize
> generic method … move the generic into a free function"), NOT a placeholder
> symbol / link failure. Quiver's entire paint pass is generic FREE functions
> (`paint_all`/`paint_node`/`frame`/… in `src/paint.rx`, `src/run.rx`), so the
> framework is fully unblocked; generic methods are tracked in `docs/TASKS.md`
> as a follow-up. Also out of scope: true separate-compilation/rlib generics
> (Q16's flat-merge is the compilation model).


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

## Q23 · S4 — `ruxen fmt` strips `##` doc comments and cannot parse test files  ✅ FIXED

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

> **FIXED (2026-06-08).**
> **(a) Doc-comment stripping** — `compiler/ruxen_core/src/formatter/format_items.rs`.
> The strip was NOT top-level docs (those round-tripped) — it was docs on a
> method NESTED inside a class/struct/enum/impl/mixin body. `format_program`
> emits leading comments only for top-level items; nested methods are formatted
> by direct `format_func_def` calls that bypass that path. New
> `format_func_with_leading_comments` emits the leading `##` (and plain)
> comments at every nested method site (class/struct/enum/impl/mixin-default),
> mirroring the existing class-body `lib "..."` FFI-def doc handling. Idempotent.
> **(b) Top-level `Tester.describe`** — the main parser ALSO rejected it (not
> just the formatter): `parse_top_level_item` had no top-level-expression-
> statement arm. `ruxen test` only works because it HOISTS top-level items and
> wraps the remaining statements in a synthesised `def main` before compiling
> (`src/ruxenc/src/test_runner.rs::split_test_body`). Fix: the SHARED parser now
> accepts a CLEAN top-level expression statement (identifier/type-ident/`self`
> head, parses without error, lands on a newline/EOF/`end` boundary) as a new
> `TopLevelItem::Expr`. The formatter formats it like an in-body statement expr,
> so test files round-trip. The DIRECT compile path rejects it at resolve with
> the new **E0728** ("wrap it in `def main`") — so direct-compile semantics stay
> well-defined and unchanged; `ruxen test` never reaches resolve with an
> unwrapped file. A genuine top-level typo (`foo bar baz`) still errors (the
> accept guard requires a clean boundary-landing parse). Match-site ripple
> (~8 exhaustive arms) handled; new error doc `docs/errors/E0728.md`. Pins:
> `compiler/ruxen_core/tests/q23_fmt_nondestructive.rs` (5 cases incl.
> idempotence + the garbage-still-errors negative). Regression-checked:
> formatter corpus round-trip (1), formatter unit (72), parser unit (79),
> error-code registry (3), implicit-codes (3), all green.

## Q24 · S4 — stale incremental cache replays false move/borrow diagnostics  ✅ FIXED

Surfaced 2026-06-08 (canvas), and the same mechanism amplified the Q18 stale-
toolchain confusion. `ruxen build`/`ruxen test` can replay **poisoned** move/borrow
errors (`E1001`/`E1009`) with bogus line numbers from a *prior* compile, even
across unrelated directories — i.e. a cached diagnostic from one build surfaces in
another. `ruxen check` is always correct (it doesn't hit the stale cache path).
Workaround: `rm -rf target/ruxen/incremental target/ruxen/test-build` clears it.
Fix: key the incremental/diagnostic cache on the actual source+toolchain identity
so a stale entry can't be replayed; never surface a cached diagnostic whose source
span no longer matches.

> **FIXED (2026-06-08).** Root cause: the cache key's toolchain component
> (`compiler_version()`) is derived ONLY from `CARGO_PKG_VERSION` + a schema
> tag, so it is INVARIANT across a `ruxen upgrade --from-source` rebuild at the
> same version (the exact Q18 dev loop) and across embedded-stdlib `.rx`/`.c`
> changes (those are baked into the binary, not bumped in the version string).
> With an unchanged key, the per-object cache HIT replayed a stale object built
> by the OLD compiler — whose move/borrow behaviour (and therefore its
> diagnostics) differed — instead of recompiling. `ruxen check` was correct
> because it never consults the object cache. Diagnostics are not persisted
> separately: they are recomputed on every cache MISS, so the fix is to force a
> miss when the toolchain changes. `compile.rs` now folds a `toolchain`
> fingerprint (the running compiler binary's path + size + mtime) into the cache
> `flags`, and `CacheKey` gained a `flags` component so the PER-OBJECT key — not
> just the manifest header — reflects it (the per-object key previously ignored
> backend / opt-override / runtime_c / toolchain entirely). A rebuilt toolchain
> now invalidates both gates → recompile → fresh diagnostics with current spans.
> `compiler_version()` stays hermetic (the ambient `current_exe` read lives in
> `compile.rs`, where ambient reads — the project `runtime/*.c` fingerprint —
> already happen). Pin: `cache_key_differs_on_flags` (`src/ruxenc/src/cache/
> hash.rs`). Regression-checked: ruxenc cache unit (60) + `cache_integration`
> (6), all green.

## Q25 · S1 — `Hash.key?`/`Hash.get` on an EMPTY hash SEGFAULTs; `&Hash`/`&Set` params unsound  ✅ FIXED

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

> **FIXED (2026-06-08).**
> **(a) Empty-hash/set segfault — `library/std/hash/runtime/hash.c`.** The
> backing table was NEVER null (`ruxen_hash_new` allocates 16 buckets); the
> real cause was the tristate `string_keys` flag (-1 unset / 0 int / 1 string),
> resolved only on the first insert. `ruxen_hash_key_hash` and
> `ruxen_hash_keys_equal` tested it with plain C truthiness — and -1 is truthy —
> so a lookup on an empty table took the STRING path and `strcmp`'d the integer
> key as a `char*`, dereferencing a small bogus address (`(char*)9`). Changed
> both predicates to `string_keys > 0`: an unresolved table hashes by raw bits
> (safe), and a string-keyed table always has the flag set to 1 by its first
> insert before any lookup. Fix covers Set too (it reuses the same predicates
> via `ruxen_hash_contains_key`).
> **(b) `&Hash`/`&Set` param "unsoundness" was an INCONSISTENCY, not a
> miscompile.** Once (a) was fixed, a `&Hash[K,V]` method param works correctly
> — it is a pointer to the hash struct, exactly like the widely-used (and
> sound) `&Array[Int]` params in `std/bufio`/`net`/`io`. The real defect was a
> free-fn **false-positive E1118** on `&Hash[Int,Int]`: the TEC-13
> `Hash → Hashable` alias makes the bare name resolve to the static-dispatch
> `Hashable` mixin, and `try_resolve_dyn_mixin_ref` rejected it ignoring the
> generic args (which mean the COLLECTION type). The method path happened to
> resolve `Hash[Int,Int]` to the collection and compiled. **Chosen resolution
> (sound + minimal): make `&Hash[K,V]`/`&Set[T]` consistent — both positions
> accept it.** A generic-args-bearing collection builtin in `&Name[..]` position
> now falls through to ordinary collection-ref resolution
> (`resolve/types.rs::try_resolve_dyn_mixin_ref`). The bare `&Hash`/`&Set` (no
> args = the `Hashable` mixin) is still rejected at compile time in both
> positions (free fn → E1118; method → "could not infer type for parameter" —
> a less precise but still load-bearing rejection; unifying the two messages is
> a DX follow-up, NOT a soundness gap). Did **not** pursue "reject `&Hash` on
> methods too" because that would also wrongly reject the sound `&Hash[K,V]`
> collection param and break parity with `&Array`. Pins:
> `tests/release-e2e/cases/617_empty_hash_lookup.rx`,
> `618_empty_set_contains.rx`, `619_hash_ref_param.rx` +
> `compiler/ruxen_core/tests/q25_hash_set_soundness.rs` (5 cases). Regression-
> checked: `stdlib_map` (7), `stdlib_set` (5), `stdlib_map_negatives` (8),
> `mixin_vtables` (17), `runtime_safety` (6), all green.

## Q26 · S1 — a capturing closure STORED under a `&var *self` reborrow loses its captures  ✅ FIXED

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

> **FIXED (2026-06-08, mir/lower closure path).** Root cause was NOT the
> reborrow — it reproduces with ANY closure nested inside another closure's
> body that re-captures an outer capture (the `&var *self` shape was just how
> quiver hit it). The nested closure's free-variable analysis
> (`mir/lower/captures.rs::collect_captures`) only consulted the enclosing
> frame's `def_to_local`; a variable captured by the OUTER block lives in
> `capture_map`, not a local, so it was never identified as capturable. The
> nested closure got a NULL captures pointer and read the value as slot
> garbage (0 → `box.call0` == 1; class handle → deref of garbage → SIGSEGV).
> Fix: closure lowering now treats `def_to_local ∪ capture_map` as the visible
> set, and fills a re-capture slot by reading the value out of the **enclosing
> captures pointer** at the outer slot index (through the cell when the
> enclosing capture is `ByRef`) instead of `def_to_local[def].unwrap()`. The
> Q22 drop-taint story is unaffected: the original owning local is already
> tainted out of the drop set by the OUTER closure's capture `SetField`; the
> nested closure only copies a pointer value forward. The rare doubly-nested
> *mutate-an-outer-by-value-capture* shape (would require retroactively
> cell-promoting an enclosing by-value capture) is **rejected with a clear
> lowering error**, not miscompiled. Pins:
> `tests/release-e2e/cases/615_nested_closure_capture_reborrow.rx`
> (asserts `call0 == 43`), `616_nested_closure_capture_class_handle.rx`
> (class handle, asserts no segfault), and the cargo pin
> `compiler/ruxen_core/tests/q26_nested_closure_capture.rs`. Regression-checked
> against `closures_dyn_dispatch` (5) + `drop_fixtures` (18), all green.

---

## Q27 · S4 — `any` is a reserved word, but as a local-variable name it gives a cryptic parse error

Surfaced 2026-06-08 building canvas `draw_paragraph` pixel-readback tests. A
local named `any` (`var any = false`) fails to parse with errors that name an
internal token, not the real cause:

```
error: expected pattern, found AnyBound (at <line>)
error: expected expression, found AnyBound (at <line>)
```

`any` is evidently a reserved keyword (the existential/`AnyBound` type form), so
it cannot be used as an identifier. That's a legitimate reservation, but two
things make it a DX trap:

1. The diagnostic surfaces the internal token name `AnyBound` and a downstream
   "expected pattern/expression" cascade, with no hint that the offending token
   is the identifier `any` or that it is reserved. A user reads it as a parser
   bug, not "rename your variable."
2. `any` is an extremely common loop-flag name (`var any = false; … any = true`),
   so it gets hit early and often in exactly the kind of pixel-scan loops GUI
   tests are full of.

Severity S4 (DX). Fix options: (a) emit a targeted diagnostic when a reserved
word like `any` appears in identifier position ("`any` is a reserved keyword;
choose another name"), and/or (b) list the reserved words in the tutorial.
Workaround: rename (`inked`, `found`, …) — canvas `tests/canvas_paragraph.rx`
uses `inked`.

---

## Q28 · S1 — `Float32` field/payload store-via-local miscompiles to 0 / crashes  ✅ FIXED 2026-06-09

> **FIXED (2026-06-09, feat/drop-elaboration).** Root cause: the struct / enum /
> tuple constructor lowering stored each field's value **width-blind** — it
> emitted `SetField { value: Use(temp) }` and codegen stored `temp` at its OWN
> SSA width into the field's fixed 8-byte slot, with NO coercion to the field's
> declared type. So an f64 value (a bare `120.5` literal, or any `Float` local)
> placed into a `Float32` field stored 8 bytes, and the later `GetField` (typed
> by the f32 pattern binding / field) loaded 4 bytes → read 0. The inline
> `120.5f32` literal and `expr as Float32` cast worked only because THOSE paths
> already produced an f32-typed SSA value before the store (the `Assign` /
> `Cast` `coerce_value` narrow), and a `Float32` fn-param coerced at the call
> boundary — which is exactly why the inline-literal-only audit saw green.
>
> Fix: a new `coerce_to_field_ty(val_local, field_ty)` re-materialises the value
> at the FIELD's declared width via a target-typed `Assign` (routing through the
> shared codegen `coerce_value` — fdemote/fpromote/fcvt, the same path the
> `as`-cast uses) BEFORE the width-blind `SetField`. Field types come from
> `lookup_construct_field_types` (struct/class, parent-prepended) /
> `lookup_variant_field_types` (enum) / the tuple's own `Ty::Tuple`. Applied in
> `mir/lower/expr/constructors.rs` (Construct / EnumVariant / Tuple) and the
> struct auto-constructor in `mir/lower/expr/method_call.rs` (the `Ty::Struct`
> `.new` path, which builds Alloc+SetField directly and bypasses
> `lower_constructors`). It is in SHARED MIR lowering, so the value is already
> the field's width by the time either backend's (still width-blind, by design)
> `SetField` runs — Cranelift and LLVM agree. A no-op when the value already
> matches the field type (the inline-f32 path).
>
> All shapes from the matrix now compute 204.75 (the uncast f64 local
> auto-narrows at the field, matching the existing fn-param-boundary UX —
> nothing crashes). The pins COMPILE + RUN the binary and assert exact stdout
> (the reopen's non-negotiable): `tests/release-e2e/cases/650_f32_field_store_via_local`
> (struct, all four shapes), `651_enum_f32_payload_via_local` (enum payload,
> load-from-local) + 647/648 + `compiler/ruxen_core/tests/q28_enum_float_payload.rs`.
> NOTE: while pinning this, a SEPARATE drop-elaboration crash surfaced — see Q31.
>
> <details><summary>REOPENED (2026-06-09, coordinator) — kept for history</summary>

> **REOPENED (2026-06-09, coordinator).** The earlier "already sound" verdict
> below was tested ONLY against inline `f32`-suffixed literals via the release-e2e
> harness, which did not assert runtime stdout. On the REAL `ruxen compile` path
> (installed feat/drop-elaboration HEAD), `Float32` struct/enum field stores are
> only correct when the value reaches the constructor as an INLINE expression
> (`120.5f32`, or `expr as Float32`) or narrows at a `Float32` fn-param boundary.
> When the f32 value is first bound to a LOCAL and then placed into the field by
> the constructor, the slot reads **0**; an UNcast f64 local into an f32 payload
> **crashes (SIGTRAP/133)**. Full repro matrix:
> `tmp/test-cache/q28-f32-field-store-matrix.md`.
>
> | shape into a `Float32` field/payload | result |
> |---|---|
> | inline `120.5f32` literal | ✓ 204.75 |
> | inline `expr as Float32` constructor arg | ✓ 204.75 |
> | f64 value through a `Float32` fn-param | ✓ 204.75 |
> | bare `120.5` (f64) literal constructor arg | ✗ **0** |
> | `let ia = … as Float32` (f32 LOCAL) then construct | ✗ **0** |
> | f64 local into payload, no cast | ✗ **crash 133** |
>
> Canvas's event decode is the `let ia = event_a() as Float32; Ev.Move(ia, ib)`
> shape → 0, so the `Int`→`Float32` coord revert stays BLOCKED until the
> load-from-local → f32-slot store path is fixed. The fix must (a) make the typed
> `SetField`/`GetField` f32 path width-correct for a value loaded from a local
> (not only an inline-narrowed arg), (b) reject-or-coerce an f64 local into an f32
> field instead of crashing, and (c) the e2e pin must actually RUN the binary and
> assert stdout (the prior 647/648 pins passed while real codegen was wrong).

</details>

<details><summary>Original (incomplete) 2026-06-09 verdict — kept for history</summary>

Surfaced as a standing deviation in `canvas/src/event.rx`: the event enum carries
pointer coordinates as `Int` logical pixels with a TODO — "return to Float32
payloads once enum float payloads compile correctly" — costing sub-pixel
precision on every pointer event, even though the C ABI already carries doubles.

```ruxen
enum Event
  PointerMove(Float32, Float32)   # the shape canvas was forced AWAY from
  Scroll(Float, Float)
  KeyDown(Int)
  CloseRequested
end
def main
  match Event.PointerMove(120.5f32, 84.25f32)
    Event.PointerMove(x, y) -> puts "move #{x},#{y}"   # want 120.5,84.25
    _ -> nil
  end
end
```

> **VERDICT (2026-06-09, feat/drop-elaboration): NOT A LIVE BUG — already fixed,
> the canvas TODO is STALE.** Like Q22, the deviation note outlived the defect.
> Enum `Float`/`Float32` payloads round-trip correctly through construction +
> `match` in every shape audited: named-field and positional-tuple variants,
> single and double float payloads, `Float32` and `Float`, MIXED with `Int`
> variants in the same enum, passed through function boundaries and stored in /
> iterated from an `Array`, with sub-pixel arithmetic on the extracted value
> (120.5 / 84.25 / 0.125 all exact).
>
> **Why it works now (mechanism, end to end):**
> - A `Float32` literal (`3.5f32`) lowers to
>   `Assign { dest: <f32 temp>, value: Literal::Float(_) }`
>   (`mir/lower/expr/literals.rs`). The Cranelift/LLVM `Assign` handler emits an
>   f64 const, then `coerce_value`-NARROWS it to the destination local's declared
>   f32 — so the temp is a real f32 BEFORE the constructor ever sees it
>   (`codegen/cranelift/emit.rs` `MirInst::Assign`).
> - The constructor stores each payload field with
>   `SetField { value: Use(temp) }` at slot `idx*8`
>   (`mir/lower/expr/constructors.rs`); codegen stores the value at ITS OWN width
>   (4 bytes for f32, 8 for f64) — no width-blind f64 store into an f32 slot.
> - `match` payload extraction loads with `GetField`, whose load type is the
>   PATTERN BINDING's declared type (f32 for an f32 field), reading the slot back
>   at the same width — `codegen/cranelift/emit.rs` `MirInst::GetField`.
> Both backends share this MIR-level typed `SetField`/`GetField` slot path, so
> they behave identically (the drop/free concern is also MIR-level, per the
> brief). Root cause of the prior breakage: the Q5 `as Float32` compiler crash +
> the case-218 / commit `1b6ced0` struct/enum inline-method float-codegen gap.
> Fixing THOSE incidentally fixed enum float payloads; nobody updated the canvas
> TODO, which is why it lingered.
>
> **No code change required.** Pinned as a regression guard so the typed slot
> path can't silently revert (which is what canvas relies on when it reverts the
> TODO to `Float32`): `tests/release-e2e/cases/647_enum_float32_payload` (f32,
> sub-pixel + extracted-value arithmetic), `648_enum_float_mixed_payload`
> (f64 mixed with Int variants, through a fn + an Array) +
> `compiler/ruxen_core/tests/q28_enum_float_payload.rs`. Affected site:
> `canvas/src/event.rx` — the `Int`-coordinate deviation can now be reverted to
> `Float32` (canvas owner handles that repo).

</details>

## Q29 · S1 — borrowed `&String` into a `lib "C"` FFI call (claimed wrong pointer)  ✅ FIXED / NOT-A-BUG (pinned)

The ledger and canvas ROADMAP claimed `measure_text` "forwards a char count, not
the string, over the FFI — a borrowed `&String` into an FFI call passes the wrong
pointer." But `canvas/src/raw_host.rx` now declares real `&String` FFI calls that
work (draw_text renders correctly in the live window), e.g.
`measure_text_raw as "ruxen_canvas_measure_text"(self, text: &String)` (alongside
the legacy `measure_text_n_raw(n: Int)` char-count fallback).

> **VERDICT (2026-06-09, feat/drop-elaboration): NOT A BUG — RESOLVED.** A
> borrowed `&String` (owned by the caller) passed into a `lib "C"` FFI function
> forwards the correct data POINTER and a recoverable LENGTH today.
>
> **Why (ABI mechanism):**
> - Ruxen's `String` is a bare NUL-terminated `char*` — there is NO length
>   header (`library/std/string/runtime/string.c`: `ruxen_string_from`,
>   `ruxen_string_len`, `ruxen_string_eq` are all `(const char *)`). A `String`
>   VALUE *is* the `char*`. Length is recovered C-side via `strlen`.
> - `MirInst::Ref` (the `&` in `&String`) is by-VALUE in both backends
>   (`codegen/cranelift/emit.rs` `MirInst::Ref`), so it forwards the `char*`
>   unchanged. The canvas shim consumes exactly that:
>   `ruxen_canvas_measure_text(int64_t self, int64_t text)` →
>   `rx_measure_impl((const char *)text, …)` (`canvas/runtime/skia_shim.c`).
> - The OLD "wrong pointer / char count" claim described the LEGACY
>   `measure_text_n_raw(n: Int)` workaround — a precomputed char count that
>   existed *because* `&String` FFI was distrusted, NOT a defect in `&String`
>   itself.
>
> **Evidence (pin).** A borrowed `&String` threaded through pointer/length-
> sensitive `String` stdlib FFI (`include?`→strstr, `find`→byte offset,
> `replace`, `starts_with`) returns exact results — `find("sub-pixel")` in
> `"hello, sub-pixel world"` yields byte index 7, `size` yields 22, `replace`
> substitutes correctly. A wrong pointer or a char-count would corrupt all of
> these. Pin: `tests/release-e2e/cases/649_ffi_borrowed_string_arg` +
> `compiler/ruxen_core/tests/q29_ffi_borrowed_string.rs`. Canvas's deviation note
> can be reverted (canvas owner handles that repo); the legacy
> `measure_text_n_raw` char-count fallback is now redundant.
>
> **Addendum (2026-06-10, release gate): the ABI was sound but the BORROW
> CHECKER was not.** The release-e2e harness (which runs the full CLI pipeline
> incl. `borrow_check`) tripped on 649 with a false `value used after move`
> (E1001): passing the owned `needle: String` value to the `&String` parameter
> of `include?` / `find` / `replace` was recorded as a MOVE, so the second use
> was rejected. The Q29 in-process pin had been BLIND to this because it skipped
> `borrow_check`. Root-caused + fixed in `borrow_check/checks.rs`
> (`check_method_call` / `check_fn_call` now treat an owned value passed to a
> `&T`/`&var T` parameter as an auto-borrow, resolving the param type by name);
> added a full-pipeline borrow-check pin
> (`borrowed_string_arg_passes_borrow_check_with_no_false_move`). See CHANGELOG
> `[Unreleased] › Fixed`.

## Q30 · S4 — `ruxen fmt` rewrites builder-closure call shapes into a known segfault form  ✅ FIXED 2026-06-09

> **FIXED (2026-06-09, feat/drop-elaboration).** Two formatter defects in
> `compiler/ruxen_core/src/formatter/format_expr.rs`:
> - A zero-param closure dropped its `||` header. `format_closure_params`
>   returned `nil()` for empty params, so `{ || expr }` formatted as `{  expr }`
>   (double space, no header). The AST can't tell `{ || expr }` from a no-pipe
>   `{ expr }` (both parse to a `ClosureExpr` with empty params via
>   `parse_brace_closure`), and the bare-brace form re-parses ambiguously — a
>   documented GUI-stack crash shape. Fix: a zero-param closure ALWAYS formats
>   with an explicit `||` (it is always a legal, idempotent closure header).
> - A zero-arg CALL lost its parens. The `ExprKind::Call` arm only emitted
>   `(...)` when there were args/a block, so `row_height()` → `row_height` — a
>   call→identifier semantic change. A `Call` node only exists when the source
>   wrote `()` (a bare name parses as an identifier/path), so the arm now ALWAYS
>   emits the parens.
> The third claimed behaviour (brace block-arg → `do…end`) did NOT reproduce —
> the inner `{ |ui, root| … }` single-expr closure body already formats as a
> brace block and is preserved. Round-trip pins (closure header, brace block-arg
> stays braces, zero-arg call parens, and the combined builder shape
> byte-for-byte) added to `compiler/ruxen_core/tests/q23_fmt_nondestructive.rs`.
> All 72 formatter `--lib` tests + the q23 pins stay green; the change is
> idempotent.

<details><summary>Original OPEN report — kept for history</summary>

Surfaced 2026-06-09 independently by BOTH GUI agents (quiver + canvas) when a
session touched `.rx` source. `ruxen fmt` is still destructive on the GUI
stack's idioms despite Q23 — it does not just reflow whitespace, it REWRITES the
call shape:

- **Drops a no-arg closure header and converts a brace builder block to a
  `do…end` passed to a free-function-style call.** quiver's example/test entry
  shape `{ || App.build({ |ui, root| … }) }` is rewritten to
  `{ App.build(do |ui, root| … end) }` — both the outer `||` is dropped AND the
  builder closure becomes a `do…end` argument. That `do…end`-to-a-call form is a
  DOCUMENTED segfault shape on this stack (a `do…end` block passed to a
  free-function with an explicit closure param — see the quiver landmines), so
  `fmt` turns compiling code into crashing code.
- **Strips parens off a zero-arg call used as a value:** `row_height()` →
  `row_height` (changes a call into a bare-name expression).
- **Blast radius is the whole tree:** the canvas agent reported `ruxen fmt`
  reformatting 43 files in one run; the quiver agent had 3 fresh test files
  corrupted (6/2/1 failures) before restoring them. Both agents had to hand-revert
  fmt's output and commit unformatted-by-fmt (matching the repo's actual hand
  convention: every existing test uses `{ || … }` and `row_height()` with parens).

Repro (minimal):
```ruxen
# before fmt (compiles):
let app = { || App.build({ |ui, root| root.text("hi") }) }
let h = row_height()
# after `ruxen fmt` (segfault shape + semantic change):
let app = { App.build(do |ui, root| root.text("hi") end) }
let h = row_height
```

Severity S4 (tooling/DX, but it produces CRASHING code from working code, so it
is the high end of S4 — it is unsafe to run `ruxen fmt` on the GUI stack today).
Related to Q23 (the prior `fmt` non-destructiveness fix, which covered doc-comment
stripping + test-file parsing but NOT call-shape rewriting). Fix should make
`fmt` preserve: (a) an explicit no-arg closure header `{ || … }`, (b) a
brace-delimited block argument as braces (never auto-convert `{…}` arg →
`do…end`), and (c) parens on a zero-arg call expression. Pin with before/after
round-trip cases over these three shapes. **No app workaround beyond "do not run
`ruxen fmt` on these repos"** — recorded in both apps' notes.

</details>

---

## Q31 · S1 — repeated `Float`-payload enum construction crashes (enum UNDER-ALLOCATION)  ✅ FIXED 2026-06-09

> **ROOT CAUSE (not a drop double-free — an under-allocation).** `alloc_size`
> (`mir/lower/emit.rs`) sized an enum allocation to its PACKED `layout.size`, but
> codegen addresses an enum's payload on a FIXED 8-byte slot stride: `GetPayload`
> reads at `base + 8` and payload field *N* at `N * 8` (cranelift `emit.rs` +
> llvm `emit/instructions.rs`). For `Move(Float32, Float32)` the packed size is
> 16, yet codegen stores field 1 at offset `8 + 1*8 = 16` — a 4-byte write **4
> bytes past the 16-byte allocation**, corrupting the adjacent heap chunk's
> metadata. The FIRST construction corrupted silently; the SECOND float-format
> `malloc` (inside dtoa) then faulted — which is exactly why it needed ≥2
> float-payload constructions and why `Int` payloads (already on 8-byte slots)
> survived. Not a drop bug at all; the enum dealloc was sound (the leak audit
> shows 3 enum allocs / 3 frees, `ruxen_alloc_outstanding == 0`).
>
> **FIX.** `alloc_size` now slot-rounds an enum to the footprint codegen actually
> addresses: `8` (tag / payload-base slot) + `widest_variant_field_count * 8`,
> for any payload width. Both backends share the slot addressing, so both honour
> it. ~40 lines in `mir/lower/emit.rs`, no codegen/drops change.
>
> **PINS (run + assert stdout / clean exit — a revert crashes them at runtime).**
> `tests/release-e2e/cases/652_enum_float_payload_double_construct` (decodes a
> `Float32` payload TWICE, asserts `frame1=204.75`/`frame2=204.75`);
> `compiler/ruxen_core/tests/q31_float_enum_payload_drop.rs` (double-construct,
> loop, Int-unaffected); `drop_fixtures.rs::q31_float_payload_enum_double_construct_no_leak`
> asserts the enum allocations balance (`ruxen_alloc_outstanding == 0`). Full
> workspace **1940 passed / 0 failed** with the fix. **Unblocks** canvas reverting
> event coords to `Float32` (a poll loop constructs many `Event`s per frame).
>
> *Note:* the leak-audit fixture's `puts "#{int}"` interpolation leaks one raw
> string-formatter temporary (`raw_outstanding == 1`) — a SEPARATE, pre-existing,
> non-enum leak the drop pass doesn't yet collect (tracked under the Drop ADR's
> open items), deliberately out of Q31's scope.

<details><summary>Original OPEN report (2026-06-09) — kept for history</summary>

Surfaced while pinning the Q28 fix. Constructing a payload-carrying enum variant
whose payload is `Float`/`Float32` **two or more times by value in a single
function** crashes at runtime (SIGTRAP / 139 / 138). It is **independent of
Q28**: it reproduces with purely-inline `f32` literals (zero coercion) and on
the BASELINE compiler before the Q28 fix, so it is not the width defect.

Boundary (each row a `match Ev.<variant>(…) … end` repeated N times in `main`):

| payload type | N=1 | N=2 | N=3 |
|---|---|---|---|
| `Int`     | ok | ok | ok |
| `Float32` | ok | **crash** | crash |
| `Float`   | ok | **crash** | crash |

Minimal repro (`/tmp/rxprobe/q31f2.rx`):
```ruxen
enum Ev
  Move(Float32, Float32)
  Tick
end
def main
  match Ev.Move(1.0f32, 2.0f32)
    Ev.Move(x, y) -> puts "#{x + y}"
    Ev.Tick -> puts "t"
  end
  match Ev.Move(3.0f32, 4.0f32)   # second construction → SIGTRAP
    Ev.Move(x, y) -> puts "#{x + y}"
    Ev.Tick -> puts "t"
  end
end
```

The values are CORRECT when it doesn't crash, so this is a drop / dealloc memory
bug specific to the float-payload enum layout under repeated allocate+drop, not a
codegen-of-the-value bug. Likely in `mir/lower/drops.rs` / the enum-payload
dealloc path (Float vs Int payload alignment/size at the dealloc site). The Q28
e2e pins are deliberately kept under the crash threshold (struct case 650 carries
the full four-shape matrix crash-free; enum case 651 uses a single construction)
so they isolate the Q28 fix from this. Severity S1 (silent → crashing code) but
narrow (needs 2+ float-payload enum constructions per function); the canvas event
loop matches once per frame, so it is not immediately on the hot path. Filed for
a dedicated drop-elaboration pass.

</details>

## Q32 · S3 — Q16's flat-merge pulls an FFI dependency's bodies into a test/binary build without linking its C runtime  ✅ FIXED 2026-06-10

Surfaced 2026-06-09 by quiver after Q16 landed. A package that declares a
path/git dependency on an **FFI-backed** library (one with `lib "C"` bindings +
its own `runtime/*.c` + `[system_libs]`) gets that dependency's full `src/**.rx`
flat-merged into its `ruxen test` EXECUTABLE — including the FFI-calling method
bodies — but the dependency's C shim objects / system libs are **not** compiled
or linked into that executable. Result: `Undefined symbols for architecture
arm64: "_ruxen_canvas_begin_frame", …` (every `ruxen_canvas_*` symbol) at link.

Repro: quiver with `canvas = { path = ... }` in `[dependencies]` →
`ruxen test` fails at link with all `ruxen_canvas_*` undefined. `ruxen check`
passes (no codegen) and `ruxen build` (library rlib) passes (symbols stay
deferred); only executable-producing builds (test, binary) hit it — and binary
builds only worked so far because the app packages ALSO declare canvas directly,
which brings its `runtime/` + `[system_libs]` into the link.

quiver's workaround is actually the better architecture for ITS case (the L2
library is platform-agnostic and never referenced canvas symbols, so the
dependency was dropped — see `quiver/Ruxen.toml`'s comment + CHANGELOG). But the
gap is real for any consumer that legitimately `use`s an FFI dependency in its
own `src/` and wants `ruxen test`: rondo-style stacks will hit it. Fix options
(architectural choice): (a) demand-driven merge — only flat-merge dependency
sources actually referenced; (b) when flat-merging a dep, also compile+link its
`runtime/**.c` and propagate its `[system_libs]` into the link line (the same
thing binary builds get when the dep is declared directly). (b) matches Q16's
"same mechanism as binaries" story.

**FIXED 2026-06-10 — option (b).** The break was real and reproduced with a
minimal FFI dep (a `lib "C"` fn backed by a 5-line `runtime/shim.c`): `ruxen
test` failed with `Undefined symbols: _ruxen_shim_answer`. The test runner
(`src/ruxenc/src/test_runner.rs`) only gathered the consuming project's OWN
`runtime/*.c`; it never looped over the flat-merged dep dirs the way
`compile_project` (the `ruxen build` binary path) already did. Fix:
1. `test_runner::build_one` now, for each flat-merged `dep_dir`, adds
   `--runtime-c=<dep>/runtime/*.c` (`codegen::find_runtime_sources_in_dir`) and
   forwards each dep's `[system_libs]` (`codegen::parse_system_libs`) as
   `--link-arg=-l<lib>`.
2. `src/ruxenc/src/compile.rs` gained a repeatable `--link-arg=` flag (mirroring
   `--runtime-c=`) that appends raw linker flags into the executable's link line
   (and folds them into the compile cache fingerprint).
3. `compile_project` (`src/ruxen_cli/src/build.rs`) also gained the dep
   `[system_libs]` propagation it was silently missing — `collect_system_lib_flags`
   only walks the STDLIB root, so a user dep's link needs were dropped on binary
   builds too (latent; canvas's `[system_libs]` is empty so it never bit).
The actual canvas break was purely the missing dep `runtime/*.c` (it dlopens
SDL2/Skia, so its `[system_libs]` is `[]`); the system-libs half is the
completeness fix for rondo-style stacks with non-empty `[system_libs]`.
Pins (`src/ruxen_cli/tests/ffi_dep_link.rs`, staged-install + RUN + assert):
FFI-dep `ruxen test` links + the test passes (4*10+1 == 41); a binary that ALSO
declares the dep directly builds + runs with NO duplicate-symbol; a non-FFI dep
still links. The Q16 `dep_visibility.rs` suite stays green.

## Q33 · S2 — `Float32 == <negative Int literal>` comparison miscompiles to false  ✅ FIXED 2026-06-10

Surfaced 2026-06-09 by canvas's `Scroll(-1, 3)` round-trip pin while reverting
event coords to `Float32`. Comparing a `Float32` value against a **negative**
Int literal evaluates false even when the value is exactly equal; the stored
value itself is CORRECT, and every other shape agrees:

```ruxen
let f: Float32 = -1 as Float32
puts "#{f == -1}"          # false  ← BUG (plain local, no enum involved)
puts "#{(f as Int) == -1}" # true
puts "#{f < 0}"            # true
let m1: Float32 = -1 as Float32
puts "#{f == m1}"          # true   (Float32 == Float32 fine)
# positive literals are fine: a Float32 holding 3 == 3 → true
```

Full repro: `tmp/test-cache/q33-negative-literal-f32-compare-repro.rx`. Likely
the comparison-position literal is narrowed to f32 through a path that
mishandles the sign (or compares at mismatched widths only when the literal is
negative — the unary-minus lowering of the literal is the prime suspect, since
`-1` is plausibly lowered as `neg(1)` AFTER a width decision). S2: silent wrong
answer in ordinary numeric code, but narrow trigger (equality against a negative
literal specifically; `<`/`>`/cast-compare all fine). Workaround in canvas
`tests/scroll_resize.rx`: compare through `as Int`.

**FIXED 2026-06-10.** Root cause was NOT the unary-minus lowering per se but the
width-blind `Compare` codegen: `codegen/cranelift/emit.rs` coerced the rhs to
the lhs's SSA type with the signedness-BLIND `coerce_value` (defaults
`signed=false`). For an `Int(-1)` operand (i64 `0xFFFF_FFFF_FFFF_FFFF`) coerced
to f32 that selects `fcvt_from_uint` → `1.84e19`, so `f32 == -1` `fcmp`'d false.
The bug was WIDER than the filing's "`<`/`>` all fine" note: `>= -1` was false
and `< -1` true (the bogus huge value only accidentally satisfied `<=`/`>`),
`Float` (f64) vs negative Int broke identically, and the literal-on-the-LEFT
shape (`-1 == f`) broke symmetrically through `fcvt_to_uint_sat` clamping `-1.0`
to `0`. Positive literals and f32==f32 were accidentally correct. Fix (shared
MIR, both backends agree): `coerce_compare_operands` in
`mir/lower/expr/binops.rs` re-materializes a mismatched numeric operand pair to a
common float width via a target-typed `Assign` BEFORE the `Compare`, invoking
codegen's Q5 signedness-aware int→float path (`fcvt_from_sint` for a signed
source) — exactly as a `let`-bound `as Float32` cast already does. Mirrors Q28's
`coerce_to_field_ty`. Int-only and equal-type pairs pass through untouched (zero
extra instructions on the hot matched-width path). Pins (RUN + assert stdout):
`tests/release-e2e/cases/653_f32_negative_int_literal_compare`,
`654_enum_f32_payload_negative_compare`, and
`compiler/ruxen_core/tests/q33_negative_literal_float_compare.rs`.

## Q34 · S2 — `ruxen fmt` drops grouping parentheses, silently changing arithmetic  ✅ FIXED 2026-06-10

Surfaced 2026-06-10 by the quiver text-metrics work. `ruxen fmt` removed the
grouping parentheses from a rounding expression, changing its meaning:

```ruxen
# before fmt (correct: add half the divisor, THEN divide — round-to-nearest):
let v = (rel * span + track_w / 2) / track_w
# after fmt (WRONG: division binds first; the parenthesized sum is gone):
let v = rel * span + track_w / 2 / track_w
```

This silently broke quiver's slider screen-x→value math (3 slider tests failed)
until the fmt churn was hand-reverted. A formatter must NEVER drop parentheses
unless the re-parse of its output is structurally identical to the input AST —
here the precedence of `/` over `+` makes the parens load-bearing.

Third formatter-destructiveness facet: Q23 covered doc-comment stripping +
test-file parsing, Q30 covered closure-header/call-paren rewriting, Q34 is
expression-grouping. The recurring root cause is that fmt re-emits from an AST
shape that doesn't preserve (or re-derive) grouping.

**FIX (2026-06-10, `feat/drop-elaboration`, ADR
`docs/decisions/syntax-parity-harness.md`).** Taken the re-parenthesize-by-
precedence route. `formatter/prec.rs` mirrors `parser::expr::infix_binding_power`
(the single precedence source) as AST-node tiers; `format_binary_op` /
`format_unary_op` / the `Range` + `Cast` arms wrap any operand whose precedence
is looser than its position requires (`needs_parens`, with the standard
left-assoc rule: left child wraps on strictly-lower precedence, right child on
lower-or-equal). All Ruxen binary operators are left-associative (verified
against the parser). Pinned by `tests/q34_fmt_grouping_parens.rs`
(reparse-identity over mixed +/−, ×/÷, logical, comparison, bitwise/shift,
unary, cast groupings + idempotence) AND the new syntax-parity harness, whose
fmt axis asserts reparse-identity over the WHOLE stdlib + sibling corpus.

The harness surfaced **four** more fmt-destructiveness bugs in the same class,
all fixed alongside Q34: zero-arg `MethodCall` → field access (`s.bytes()` →
`s.bytes`); method visibility-section drop (`private` method round-tripped
public); `async` modifier drop (`async def` → `def`); plus the catalogued
(non-bug) import-member reordering. With these fixed the standing "do not
bulk-run `ruxen fmt`" caution is LIFTED — `ruxen fmt` is now reparse-faithful
over the entire corpus (491 files, binary + in-process, green).

## Q35 · S3 — a STRUCT's `include <Mixin>` does not satisfy a generic's mixin bound  ✅ FIXED (2026-07-14)

Found 2026-06-10 while independently verifying Q17 on the installed toolchain.
A struct that `include`s a mixin is rejected by the generic bound-satisfaction
check (E1015) when passed to a mixin-bounded generic — even as the ONLY
implementor, so this is orthogonal to Q17's per-implementor monomorphization
(which is class-proven: the 655 fixture runs `dep=20 mine=9` on the installed
CLI). Classes with the identical `include` + method bodies work.

```ruxen
mixin Paintable
  def fill_rect(w: Int, h: Int) -> Int
end

def paint_all[T: Paintable](s: &var T, w: Int, h: Int) -> Int
  s.fill_rect(w, h)
end

struct OnlyOne          # ← struct, not class
  include Paintable
  tag: Int
  def fill_rect(w: Int, h: Int) -> Int
    w * h + self.tag
  end
end

def main
  var s = OnlyOne.new(1)
  puts "one=#{paint_all(&var s, 4, 5)}"   # E1015: `OnlyOne` does not satisfy `Paintable`
end
```

Root cause: `typeck/mixins.rs::collect_item_impls` registered a class's body
methods, derives, and `impl_blocks`, but the `HirItem::Struct` and
`HirItem::Enum` arms registered only derives. Consequently their positive
`include Mixin` edges never reached `MixinResolver::nominal_impls`, which is the
registry consulted by E1015 generic-bound satisfaction. Direct method calls,
marker `Send`/`Sync`, and MIR's downstream implementor collection use separate
paths, explaining why only this seam failed.

Fixed by registering struct and enum inner impl blocks under their concrete
(module-qualified when applicable) type names, including parameterized mixin
arguments, exactly as the class path does. The change does not register bare
body methods as nominal includes: a struct without `include` still emits E1015.
Release-e2e case `659_generic_fn_mixin_struct_implementor.rx` compiles, links,
runs through Q17's generic free-function monomorphization, and asserts
`one=21`; focused typeck pins cover the non-including struct and enum symmetry.

## Q36 · S2 — `yield` with TWO `&var` reference args miscompiles (block sees empty target)  ✅ FIXED 2026-07-14

Found 2026-06-10 migrating quiver's builder DSL to `&block`/`yield` (after the
block-semantics feature `8a783f9`). A function declaring
`&block: Fn[(&var Ui, &var Col) -> nil]` and invoking it with
`yield(&var app.ui, &var app.root)` — TWO `&var` references to fields of the same
object in one `yield` — COMPILES but the block's `&var` params do not bind: the
block runs against an empty/wrong target.

```ruxen
def self.build_blk(&block: Fn[(&var Ui, &var Col) -> nil]) -> App
  var app = App.new
  yield(&var app.ui, &var app.root)   # block sees an EMPTY Col
  app.mount
  app.arrange
  app
end
# var app = App.build_blk do |ui: &var Ui, root: &var Col| root.text("a"); root.text("b") end
# EXPECTED app.root.size == 2 ; ACTUAL 0
```

A single-`&var`-arg `yield(&var *self)` works (quiver's `row`/`col`/`list`/
`row_styled`/`col_styled` converted fine). The ordinary closure-call form
`f.(&var app.ui, &var app.root)` works (it's what `App.build` keeps). So the bug
is specifically **N≥2 `&var` reference arguments through the `yield` ABI**.

Repro: `quiver/tmp/test-cache/ruxen-two-var-yield.md`. S2 (silent wrong build,
not a crash). Workaround in quiver: `App.build` stays an explicit closure param.
Likely the synthetic-`__block` call ABI mishandles multiple by-reference args.

**Related report (same pass, smaller):** the original report also observed an
untyped `do |c| … end` against `&block: Fn[(&var Col) -> nil]` leaving `c` as
`?T` (surfacing at codegen as `no runtime symbol for ?T::text`). This was
retested below against the current tree.

**ROOT CAUSE.** `parser/expr/atoms.rs` parsed the token after `yield` as a
general expression. For `yield(&var app.ui, &var app.root)`, the general
parenthesized-expression parser correctly produced a `TupleLiteral`, but that
meant `ExprKind::Yield` contained ONE argument: the tuple. MIR therefore
materialized both references into a tuple and emitted `CallIndirect(captures,
tuple)`, while the block closure ABI expected `CallIndirect(captures, ui,
root)`. A single parenthesized argument has no comma and is unwrapped by the
parenthesized-expression parser, explaining why single-`&var` yield worked;
ordinary `f.(a, b)` already used the call-argument parser and never formed the
tuple.

**FIX.** Parenthesized `yield(...)` now uses the same `parse_call_args` path as
ordinary calls, preserving each argument as its own ABI slot. Bare Ruby-style
`yield a, b` remains supported. Release-e2e pin
`929_yield_two_var_refs.rx` runs through the in-process compiler and proves
both references mutate the original `App` fields (`ui=2 root=1`).

**Inference sub-gap.** No residual remains on the current tree: an in-process
pin with an untyped `do |c|` against `&block: Fn[(&var Col) -> nil]` resolves
`c` as `&var Col` and runs successfully. The existing signature-based closure
parameter seeding had already closed this path; Q36 adds the missing regression
coverage.

## Q37 · S1 — a yielding/`&block` METHOD poisons an unrelated same-named free fn with a phantom `__block`  ✅ FIXED 2026-06-10

Found 2026-06-10: all three quiver examples fail `ruxen build` on the installed
toolchain (reproduces at pristine quiver HEAD, clean rebuild — NOT introduced by
the block migration):

```
Error: error: could not infer type for parameter `__block` in function `frame` (at ####:1)
```

`frame[S: PaintSurface]` / `first_frame[S: PaintSurface]` (`quiver/src/run.rx`)
have NO `yield` and NO block, yet the binary-consuming build synthesizes a
`__block` parameter onto `frame` and then fails to infer its type. The quiver
**library** builds clean (`ruxen build` in quiver/); the 157-test headless suite
is green (so `frame`/`first_frame` are fine in test mode — `tests/counter.rx`
exercises them). The failure is specific to **binary-consumes-library** builds
after the block-semantics feature landed.

**ROOT CAUSE (confirmed in code 2026-06-10).** `resolve/yield_scan.rs::
collect_yield_fns` recorded yielding functions into `Resolver::yield_fns`, a
`HashMap<String, usize>` keyed by **bare function name**. The synthetic-`__block`
decision in `funcs.rs::resolve_function` looked the function name up in that map
(`self.yield_fns.get(&f.name)`). In the flat-merged binary build, canvas's
newly-added block-taking METHODS `Window#frame` / `Canvas#frame` (which `yield`
their surface) registered `frame` in the map → quiver's unrelated generic free
fn `frame[S: PaintSurface]` (no `yield`, no `&block`) then matched the same key
and was handed a phantom `__block` it never uses → `could not infer type for
parameter __block` + an inflated arity. The minimal earlier repro missed it
because the colliding definition must be a *yielding/block-taking method* with
the *same bare name* as the free fn — exactly the quiver↔canvas `frame` clash
introduced when canvas gained its `frame` methods (after the examples last built
green). This is **S1**: ANY user name collision between a block-taking method
and a same-named free fn breaks compilation.

**FIX (`feat/drop-elaboration`).** The synthetic-`__block` decision is now made
**locally** from the function's OWN body
(`super::yield_scan::find_first_yield_arity_in_block(&f.body)`), not from a
name-keyed map — a function gets a `__block` iff it itself yields. The buggy
`yield_fns` map, its `collect_yield_fns` populator, and the two
`bootstrap_merge.rs` pre-scan calls are deleted (the local check is strictly
more correct and collision-free). `find_first_yield_arity_in_block` /
`first_yield_self_mask_in_block` stay (they drive the arity + `yield self`
typing). Pin: release-e2e **920** (a block-taking method `Canvas#frame` + an
unrelated generic free fn `frame`, both called, compiled + run). Verified: the
quiver counter example builds again (`canvas`/`quiver`/`counter` pieces compile
clean) with the fixed compiler.

## Q38 · S4 — a `String` local bound from a BARE LITERAL was not freed at scope exit (leaked)  ✅ FIXED 2026-06-10

Found 2026-06-10 while sweeping `String.from("literal")` → `"literal"` across all
shipped `.rx`. A `String` local bound from `String.from("x")` was drop-elaborated
and freed at scope exit, but the SAME local bound from a bare literal `"x"` was
NOT — it leaked to process exit.

```ruxen
def main
  let s = "hello"      # bare literal: WAS &str-typed → not dropped → leak
  let _len = s.size
end
# before: string_frees=0, raw_outstanding=1
# after : string_frees>=1, outstanding=0  (identical to String.from("hello"))
```

**ROOT CAUSE (confirmed in MIR).** Not a drop-analysis bug — a TYPE-INFERENCE
one. The resolver types a string literal `Ty::Str` (`resolve/exprs.rs`), and an
un-annotated `let s = "x"` adopted that, so the local `s` was typed `&str`. MIR
still lowers the literal through `ruxen_string_from` to a heap-owned `String`
(the P0.7 owned-literal wrap), but the drop filter (`drops.rs`) only frees
`Ty::String` locals — a `&str`-typed local holding an owned heap copy is skipped
→ leak. (`s.size` also dispatched to `&str_size` instead of `String_size`.)
`String.from("x")` and `let s: String = "x"` both forced `s: String`, so they
were freed; the bare un-annotated form was the gap. S4 (leak only; no
double-free / UAF / wrong value). Pre-existing; surfaced by the sweep.

**FIX (`feat/drop-elaboration`).** Typeck `infer/mod.rs::
promote_bare_string_literal_binding` (run in the `HirStatement::Let` arm, next
to the `auto_wrap_option_some` / `auto_call_fn_reference` sugar): when the
binding slot is unconstrained (annotation is an unresolved `Infer` var) and the
initializer is a bare `StringLiteral`, bind the local `Ty::String` (owned). So
`let s = "x"` now owns + drops exactly like `let s = String.from("x")`. Narrowly
scoped: an explicit `let s: &str = "x"` keeps the borrow (slot not `Infer`), and
every call-site/argument coercion path is untouched (literals still coerce to
`&str`/`&String` params). All `String.from("literal")` across the shipped `.rx`
corpus (tutorials, stdlib, release-e2e cases, fixtures, src fixtures) are now
swept to bare literals — including the `drop_fixtures.rs` String drop pins, which
this fix lets stay green on the bare form. Pins:
`drop_fixtures.rs::string_local_is_freed_on_scope_exit` (fixture is now
`let s = "hello"`, asserts `outstanding==0`, `string_frees>=1`),
`string_literal_wrap.rs::bare_string_literal_let_binds_owned_string` (type
promotion + the `&str`-annotation-untouched negative).

## Q39 · S3 — a bare string literal in a TUPLE-ELEMENT position expecting `String` was not coerced (stayed `&str`)  ✅ FIXED 2026-06-11

Found 2026-06-11 in rondo (workaround W21) while finishing the `String.from`
deletion arc. The literal-coercion work (Q38 + the "all positions" pin
`922_string_literal_coercion_all_positions`) promoted a bare `"x"` to owned
`String` in owned/borrow/struct-field/`Err(...)`-payload positions — but NOT in
a TUPLE constructor element. So a helper returning `(String, Bool)` with a bare
`""` first element typed as `(&str, Bool)`, which then failed to unify with the
declared `(String, Bool)` return. rondo hit this on its `(String, Bool)`-shaped
returns and worked around it by spelling `("".to_string(), false)`.

```ruxen
def split_pair(found: Bool) -> (String, Bool)
  if found
    ("hello", true)
  else
    ("", false)        # before: tuple typed (&str, Bool) → E: expected (String, Bool)
  end
end
# workaround was ("".to_string(), false); now ("", false) typechecks + runs.
```

**ROOT CAUSE.** A type-inference gap, not a drop bug. Tuple-literal synthesis
(`typeck/infer/expr.rs`, `HirExprKind::Tuple`) types each element bottom-up with
NO expected-type context, so a bare `StringLiteral` element stays `Ty::Str`. The
expected-type-directed coercion seams that fire at the return/let positions
(`coerce_array_literal_to_fixed`, `promote_bare_string_literal_binding`,
`auto_wrap_option_some`) had no tuple sibling. The all-positions pin missed
tuple elements because it tested owned/borrow/field/`Err()` positions, not a
tuple constructor. S3 — clean error, one-line workaround (`.to_string()`).

**FIX (`feat/drop-elaboration`).** New `infer/mod.rs::
coerce_tuple_literal_elements`, wired at BOTH the return seam (after
`auto_wrap_option_some`) and the `let`-binding seam (after
`promote_bare_string_literal_binding`). When the expected type is a `Ty::Tuple`
and the value is a `Tuple` literal, each element that is a bare `StringLiteral`
typed `&str`/`str` against a `String` expected slot is re-typed `Ty::String`
(the literal already lowers through `ruxen_string_from` to an owned heap copy,
so this matches codegen with no drop hazard — same precedent as the `Err("msg")`
payload coercion). Descends into `if/else`/`block`-tail/`match`-arm tails (like
`auto_wrap_option_some`) so a branchy return body is covered, re-pinning the
structural node's type to the coerced tuple. Narrow: only a String-literal in a
String slot is rewritten — a genuine `(Int, Bool)` vs `(String, Bool)` mismatch
still errors. Pin: `924_tuple_element_string_literal_coercion` (rondo's exact
`(String, Bool)` shape, return-position + let-annotation; RUNS + asserts stdout).
rondo can drop W21's `("".to_string(), false)` workaround for bare `("", false)`.

## Q40 · S2 — `Mutex.lock` link failure FIXED; bare-`Mutex[String]` set/get drop-timing edge filed separately  ✅ (a) FIXED 2026-06-11 · ⏳ (b) FILED

ROOT CAUSE of the link failure was NOT a `Mutex[String]`-specific
monomorphization gap — it is the **ergonomic `lock` wrapper having no codegen
body, for EVERY `Mutex[T]` (Int included)**:

- typeck (`method_resolvers/concurrency.rs:162`) advertised `Mutex.lock` →
  `Result[MutexGuard[T], PoisonError]` (and `lock!`/`try_lock`/`into_inner`),
  but `library/std/sync/src/mutex.rx` only implemented the `_raw` FFI
  (`lock_raw`). So `m.lock` typechecked then emitted an undefined `Mutex_lock`
  symbol → `Undefined symbols: _Mutex_lock` at link. The coordinator's repro
  used `m.lock` (the wrapper), not `m.lock_raw`; that is why "direct" appeared
  to link in the original note — `lock_raw` always linked, `lock` never did, for
  any payload type. (Bisected: `Mutex[Int].lock` link-fails identically.)

**FIX (a):** implemented the wrappers as real `.rx` bodies in `mutex.rx`,
layered on `lock_raw` + `is_poisoned` (`lock` → `Result`, `lock!` → guard,
`into_inner` → `Result`); `PoisonError` (an empty marker) gained an `init` so
the `Err(PoisonError.new)` arm constructs. Proven RUN+stdout-correct for: a
`&Mutex[String]` borrow param, a captured `Mutex[String]` closure, and quiver's
real **`SharedSync`-owns-a-`Send`-class-owning-`Mutex[String]`** write→read
round-trip (the `ClipboardCell` shape — `got=payload-xyz`). Pins: release-e2e
`925_mutex_string_lock_borrow`, `926_mutex_string_lock_capture`,
`927_mutex_string_sharedsync_roundtrip`. quiver's `ClipboardCell` workaround
note can add a "can now revert (optional)" line — but it ALSO works as-is.

**(b) FILED — two SEPARATE, pre-existing, deeper drop-timing issues the link
fix surfaced (NOT fixed here; `Mutex[String]` is sound for the realistic
single-lock-per-scope / SharedSync shapes above, which is what quiver uses):**

1. **`MutexGuard` drops at FUNCTION exit, not lexical BLOCK exit.** Drop
   elaboration inserts drops before `Terminator::Return`, not at block end, so a
   `MutexGuard` bound in a nested block (or two sequential `match m.lock` in one
   function scope) holds the pthread lock until the function returns → the
   second `lock` DEADLOCKS. Reproduces with bare `lock_raw` twice in one scope
   (independent of `lock`/String). General block-scoped-drop limitation; affects
   any RAII guard, not just Mutex. Fix needs block-scope drop elaboration.

2. **A heap value stored through a generic FFI setter is freed by the caller →
   dangling read.** `g.set("x")` (`ruxen_mutex_guard_set`, classified a borrow
   helper by its `ruxen_` prefix) stores the `char*` into the Mutex payload, but
   the caller's `"x"` String temp is freed at the writer scope exit (the C
   `ruxen_mutex_drop` never frees the i64 payload — the generic-stripping ABI
   doesn't know it's heap). Read-back THROUGH A CLOSURE then returns empty
   (`read=`). A targeted classification (mark `ruxen_mutex_guard_set` arg1
   transferred) would convert the UAF into a *leak* (Mutex never frees the
   String) — a net safety improvement but not soundness; the real fix is
   generic-heap-payload ownership on the i64-stripped Mutex/SharedSync ABI
   (the `State[Array]`/`Mutex[Array]` family). Deferred — quiver's SharedSync
   pattern (each `lock` in its own method frame, value flows through a param
   hop) round-trips correctly and is the recommended shape.

Repros: `tmp/test-cache/q40-mutex-string-closure-repro.rx` (link, now passes);
the (b) shapes in `compiler/ruxen_core/tests/q40_mutex_string_repro.rs` during
investigation. Also reconfirmed by the same quiver report: the
non-Copy-call-result-as-method-arg landmine at a fresh site (quiver
`cut`/`clip_write` — let-bind first, as documented).

## Q41 · S2 — LLVM backend can't lower a `DataAddr` class-info ref for a `dispatch runtime` mixin type → `ruxen build --release` blocked for async projects  ✅ FIXED 2026-07-14

Found 2026-06-14 in rondo (workaround **W22**, `rondo/docs/ruxen-issues.md`)
while benchmarking the framework against a Go `net/http` baseline. `ruxen build`
(default Cranelift) compiles and runs; `ruxen build --release` (LLVM backend)
aborts in codegen:

```
$ ruxen build --release
  Compiling piece `rondo` v0.1.0
Error: mixin-vtables: LLVM backend cannot lower DataAddr { data_sym:
'__rx_classinfo_TimeSleepFuture' } — use the Cranelift backend (default)
for code that includes a `dispatch runtime` mixin.
```

**Trigger.** Any type that participates in a `dispatch runtime` mixin and whose
`__rx_classinfo_<T>` symbol is referenced as a `DataAddr` operand. Here it is
`TimeSleepFuture`, pulled in transitively by `AsyncTcpStream.read_with_timeout`
(the per-poll idle-timeout timer — rondo F9), so the whole async server depends
on it. The error fires while compiling the `rondo` library piece itself, so it
is **not** specific to one binary: every release build of any rondo-dependent
program hits it, and more generally any project using the async timer / a
runtime mixin.

**Repro (minimal):**

```
# any binary depending on rondo (e.g. the rondo-smoke bench crate)
ruxen build              # OK   (Cranelift)
ruxen build --release    # FAIL (LLVM, error above)
```

`RUXEN_BACKEND=cranelift ruxen build --release` does NOT help — `--release`
still routes to LLVM and fails. There is no flag to get Cranelift *with*
optimizations, so async projects currently have **no optimized build path**.

**Why it matters.** Severity **S2**: no crash/corruption and a working debug
path exists, but there is no release/optimized build for async code, which
distorts every performance number. In a like-for-like HTTP bench (single
plaintext route, keep-alive, CPU-pinned 8-core server / 8-core load generator on
a 16-core Linux box), debug-Cranelift rondo peaked ~113k RPS and cliffed past
c≈200, versus optimized Go `net/http` ~164k RPS stable through c=400 — the
debug-build handicap is a real confound.

**Root cause.** LLVM's function-emission pass had no equivalent of Cranelift's
Pass 1.5/Pass 3 mixin-data handling. It neither declared nor initialized the
`__rx_vtable_*` and `__rx_classinfo_*` globals, so the `MirInst::DataAddr` arm
deliberately returned the quoted error. A second guard rejected any program
whose MIR contained vtables/class-info even if instruction lowering could have
continued.

**Fix.** The LLVM emitter now creates each vtable and class-info as an internal,
constant array of LLVM 18 opaque pointers after declaring all user functions
and before emitting bodies. Vtable initializers contain method-function
pointers; class-info initializers contain pointers to the matching vtable
globals. `DataAddr` resolves the same symbol in the module, converts its opaque
pointer to the MIR destination's `Int` slot, and stores it. The module also
uses the target machine's data layout, leaving native/wasm pointer size and
alignment to LLVM rather than encoding a typed-pointer or host-width
assumption. Both former LLVM-only bailouts are removed; an unknown data symbol
still produces a hard codegen error rather than null/garbage.

**Regression pin.** `q41_runtime_mixin_class_info_lowers_links_and_runs` compiles
a Ruby-syntax future-like class including `Q41Future dispatch runtime` with
LLVM `-O2`, links it, executes a call through an `any Q41Future` field, and
asserts the dynamically dispatched result. Before the fix it failed on
`__rx_classinfo_TimeSleepFuture`; after the fix it prints the success marker.
Run with `cargo test -p ruxen_core --features llvm --test llvm_backend
q41_runtime_mixin_class_info_lowers_links_and_runs -- --exact`.

Cross-linked from rondo W22.

Toolchain: `ruxen 0.1.0`.

## Q42 · S2 — deliver a DOM event to a Ruxen handler on wasm — NOT a compiler bug; a pure-Ruxen pattern  ✅ RESOLVED 2026-06-16 (verified by spike)

> **RESOLVED 2026-06-16 (same day it was filed).** The falsification spike is
> GREEN: a `.rx` with an exported `def boot() -> Registry` (a class holding
> `handlers: Array[any Fn[...]]`), `def dispatch_event(reg: &var Registry, id,
> kind)` calling `reg.handlers.get(id)` via `f.(&var reg.count)`, and `def
> read_count` — compiled to wasm32 (`ruxen compile --target
> wasm32-unknown-unknown`) and run in node: JS calls `boot()`, holds the returned
> i32 handle, calls `dispatch_event(reg,0,1)` TWICE in SEPARATE calls, then
> `read_count(reg)` → **2**, with ZERO host imports. So the registry round-trips
> through JS as an i32 across calls and the stored closure dispatches correctly —
> **no `__indirect_function_table`, no `call_indirect` trampoline, no codegen
> edit.** Repro + evidence: `tmp/spikeA/` and
> `docs/superpowers/plans/2026-06-16-wasm-native-element-gates.md` (RESULTS). No
> L0 work needed for event delivery. (Naming gotcha discovered: `init` is the
> reserved constructor word — name the exported entry `boot`, not `init`.)
>
> **Original grounding note (2026-06-16), now confirmed:** this is NOT L0
> compiler work. Every top-level free `def`
> is already auto-exported to wasm (`mir/lower/mod.rs:608-628`,
> `codegen/llvm/emit/mod.rs:78-96`), and `f.()` already lowers on wasm to
> `build_indirect_call` (`codegen/llvm/emit/instructions.rs:566-619`). So a
> plain exported `def dispatch_event(reg, id, kind)` that looks up a stored
> closure in an `Array[any Fn]` and calls it needs **no** new export plumbing,
> no `__indirect_function_table`, no `call_indirect` trampoline, no codegen
> edit. The one real constraint is **no top-level mutable global** — the handler
> registry must persist across calls via a heap handle round-tripped through JS
> as an i32 (a class lowers to `ptr`; canvas's opaque-handle inversion). That
> round-trip is the **single unverified primitive**, and the whole web backend
> depends on it. **Do not edit the compiler for Q42 until the falsification
> spike is RED.** Spike + plan: `ruxen/docs/superpowers/plans/2026-06-16-wasm-native-element-gates.md` (Task 1).
> Only the sub-cases there (closures-in-`Array` corrupts, or the heap handle
> can't survive a second call) would escalate to genuine L0 work (a new Q43).

Found 2026-06-16 designing quiver's **native-element web backend** (real
DevTools-inspectable DOM instead of CanvasKit paint — see
`quiver/docs/decisions/native-element-backend.md`). **Create-and-mutate DOM from
wasm already works** (the opaque-handle pattern `ruxen_canvas_host_new` ships,
plus linear-memory strings). The OPEN question is only whether the reverse
(event → stored handler) works in pure Ruxen across calls — see the revision
note above.

**Trigger.** Any interactive web app: a DOM `click`/`input` listener must call
back **into** wasm to run the stored quiver handler closure. There is no path.

**Root cause (verified in-tree):**

- wasm exports are **only** concrete top-level free `def`s — `mir.wasm_exports`
  is populated from those alone (`compiler/ruxen_core/src/mir/lower/mod.rs:611-627`),
  and the `export_name` attr is set only for them
  (`compiler/ruxen_core/src/codegen/llvm/emit/mod.rs:78-96`).
- No `__indirect_function_table` is exported and no `call_indirect` trampoline is
  emitted on the wasm path (the only `call_indirect` is in the Cranelift backend,
  which is hard-blocked for wasm).
- A Ruxen `Fn`/`FnMut`/`FnOnce` lowers to an opaque `ptr` (an `i32` into linear
  memory pointing at a closure env) — **not** a JS-callable
  (`compiler/ruxen_core/src/codegen/llvm/types.rs:58-60`).

So the whole runtime is one-shot **pull**: JS calls a single exported `render()`,
wasm calls out via `env.*` imports, returns, done. JS cannot re-enter wasm to
deliver an event.

**Repro (shape):** compile a `.rx` exporting `render()` and a handler closure;
from JS, attempt to invoke the handler on a DOM `click`. There is no exported
symbol or table index to call — the handler is unreachable from JS.

**Why it matters.** Severity **S2**: no crash/corruption, and non-interactive
render works, but **no interactive web app is possible** without it. It is the
single decisive gate for quiver's native-element web milestone.

**Fix direction (library-first).** Write `dispatch_event` as an ordinary
top-level `def` in Ruxen — `def dispatch_event(reg: Registry, id: Int, kind:
Int)` that pulls a stored closure from `reg.handlers: Array[any Fn]` (the
Q2-safe closure-pool pattern quiver already uses for `dyn_text`/`button`
handlers) and calls it with `f.()`. The registry persists across the two wasm
calls (register during `render`, fire on a later `click`) as a heap handle
returned by an exported `def init() -> Registry` and passed back in by JS as an
i32 — NOT a global. **No `__indirect_function_table`, no `call_indirect`
trampoline, no compiler edit** (the spike confirmed the round-trip IS
expressible). De-risked GREEN: the wasm spike has JS call `boot()` (the exported
entry — `init` is the reserved constructor word), then `dispatch_event(reg,…)`
in a separate call, observing the Ruxen-side counter increment. Cross-linked from
`quiver/docs/decisions/native-element-backend.md`.

Toolchain: `ruxen 0.1.0`.

## Q43 · S3 — `Mutex`/`SharedSync` pthread C runtime is not bundled for wasm; quiver's reactive core needs a single-threaded sync shim  ✅ RESOLVED 2026-06-16

> **RESOLVED 2026-06-16.** A single-threaded wasm sync runtime is now bundled in
> the `WASM_RT_C` shim (`compiler/ruxen_core/src/codegen/object.rs`): `Mutex`/
> `SharedSync`/`MutexGuard` are one-slot i64 boxes over the bundled allocator,
> guard handle == mutex pointer, all `ruxen_mutex_*`/`ruxen_sharedsync_*` defined
> with the exact wasm ABI. wasm-only (native still uses the pthread runtime — no
> regression; wasm pins 4/4 + examples 05/07/08 green). quiver's reactive
> `State[T]` (`SharedSync[Mutex[Int]]`) now runs on wasm with NO sync host
> imports — verified end-to-end by `quiver/examples/counter-dom` (the counter
> increments via real `State` on wasm). The JS sync shim is gone.

Found 2026-06-16 in the Phase-0.5 wasm de-risk (Spike C,
`docs/superpowers/plans/2026-06-16-wasm-native-element-gates.md`). GOOD news
first: a minimal `Mutex` program (`Mutex.new(0)` → `lock!` → `guard.set(get+41)`
→ `get`) **compiles to wasm32 cleanly — it does NOT hit the Q41 LLVM
vtable/class_info wall.** The catch: `std.sync` is pthread-backed
(`[system_libs] libs = ["pthread"]`, `library/std/sync/runtime/mutex.c`), and
wasm32-unknown-unknown has no pthread, so the curated wasm runtime does not
bundle `mutex.c`. The four FFI symbols (`ruxen_mutex_new`, `ruxen_mutex_lock`,
`ruxen_mutex_guard_get`, `ruxen_mutex_guard_set`) are therefore emitted as
**undefined host imports** (resolved by `wasm-ld --allow-undefined`).

**Proven workable:** with a thin single-threaded JS sync shim (a JS box per
Mutex; guard handle == mutex handle; ABI: `Mutex`/`MutexGuard` classes are i32
handles, payload `T=Int` is i64), the program runs in node and returns 41
(`tmp/spikeC/run.mjs`). This is the exact approach the canvas web harness already
uses (its Mutex/SharedSync are JS-boxed).

**Why it matters.** Severity **S3**: not a crash/blocker — quiver's reactive
core (`State[T] = SharedSync[Mutex[T]]`) DOES run on wasm, but only if something
provides the sync symbols. **Fix direction:** ship a wasm-targeted single-
threaded sync runtime (a `cfg(wasm)` no-op `mutex.c`/`sharedsync.c` shim —
single-threaded wasm needs no real locking), so quiver-on-wasm is self-contained
and apps don't each hand-roll JS sync shims. Until then, the web shell must
supply the `ruxen_mutex_*` (and `ruxen_sharedsync_*`) imports. Cross-linked from
`quiver/docs/decisions/native-element-backend.md`.

Toolchain: `ruxen 0.1.0`.

## Q44 · S3 — string interpolation's `Formatter` C runtime is not bundled for wasm; needs a wasm fmt runtime (or host shims + JS→wasm string marshalling)  ✅ RESOLVED 2026-06-16

> **RESOLVED 2026-06-16; generalized by Q48 on 2026-07-14.** `fmt.c` was already
> in `WASM_RUNTIME_CORE`, but mangled `Formatter_*` calls leaked as host imports.
> The original fix added wasm-only wrappers over the bundled
> `ruxen_fmt_formatter_*` symbols, which made interpolation self-contained and
> was verified by `quiver/examples/counter-dom`. Q48 fixed the LLVM resolver
> itself: it now maps every FFI alias to its C declaration on native and wasm,
> so the temporary mangled wrappers have been removed. The JS Formatter shim
> and scratch region remain unnecessary.

Found 2026-06-16 wiring the native-element web counter (quiver
`examples/counter-dom`). A `"count: #{n}"` interpolation lowers to compiler-
emitted calls `Formatter_new`/`Formatter_write_str`/`Formatter_buffer` (the MIR
interpolation lowerer; see `library/std/fmt`). On wasm32 the `fmt.c` runtime
behind those symbols is NOT bundled into the curated wasm runtime, so they are
emitted as **undefined host imports** (like Q43's sync symbols).

Two sub-problems for a host shim:
1. **ABI (verified by parsing the module's type section):** `Formatter_new() ->
   i64`, `Formatter_write_str(i32, i32) -> void`, `Formatter_buffer(i32) ->
   i64`. The handle is created as i64 but truncated to i32 at later call sites;
   `buffer` returns the result `String` in an i64 slot (the pointer value fits
   i32). A shim must return BigInt for `new`/`buffer`.
2. **JS→wasm string marshalling (the real gap):** `Formatter_buffer` must return
   a `String` **living in wasm linear memory** so the rest of the program (and
   `dom_set_text`) can read it. JS has the built string but must WRITE it into
   linear memory and return a pointer — and there is no exported allocator, so
   the shim writes into a high scratch region of exported `memory` and bumps a
   pointer (must stay above the wasm bump heap, else corruption — observed as a
   non-deterministic wrong initial value until the scratch was placed high
   enough). This is the same "no JS→wasm string helper" gap the tier-4.09
   findings flagged.

**Proven workable:** with the ABI shim + a high bump-scratch, the counter renders
correct interpolated text and is fully interactive in a real browser
(`examples/counter-dom/web/index.html`, Playwright-verified). **Fix direction:**
bundle a `cfg(wasm)` `fmt` runtime (so interpolation is self-contained, no host
import), OR export a wasm allocator (`ruxen_alloc`/`ruxen_dealloc`) so a host
`Formatter_buffer` shim can safely allocate the result string instead of
scribbling into a guessed scratch offset. Cross-linked from
`quiver/docs/decisions/native-element-backend.md`. Sibling of Q43 (sync runtime).

Toolchain: `ruxen 0.1.0`.

## Q45 · S3 — no JS→wasm string path (host could read strings OUT of wasm, never write them IN)  ✅ RESOLVED 2026-06-16

Found 2026-06-16 planning native `<input>` for quiver's web backend. A host could
read a Ruxen `&String` out of wasm (NUL-terminated `char*` in exported `memory`),
but had no way to put a string IN — no exported allocator, so a native input's
typed value couldn't reach the app's `State`. **RESOLVED:** the `WASM_RT_C` shim
now `export_name`-exports `ruxen_wasm_alloc(i32)->ptr` + `ruxen_wasm_free(ptr)`
over the bundled allocator. JS allocates a buffer in wasm memory, writes
UTF-8+NUL, and passes the pointer to an exported `def f(... text: &String)` (the
i32 ptr IS the `&String`). Verified exported in `quiver/examples/counter-dom`.
Toolchain: `ruxen 0.1.0`.

## Q46 · S2 — a NO-ARG `def init` class's paren-less `.new` CRASHES (exit=None / segfault) when the value is wrapped/moved into another class's field  ✅ FIXED (2026-07-14)

Found 2026-06-19 building the §4.5 async `Stream` adapters. A class whose `def
init` takes **no parameters**, constructed paren-less as `X.new`, compiles and
runs fine in isolation, but the program **crashes (no exit code / segfault, empty
stdout)** when that value is moved into another class's constructor (e.g. a
stream adapter holding it as an `any Mixin` field). Giving the source an
ARG-taking `init` makes the crash vanish — so the trigger is the no-arg-init
constructor in the wrap-into-field position, NOT the wrapping itself. Pre-existing
constructor-lowering bug, unrelated to Stream.

**Confirmed root cause:** the parser represents paren-less `Class.new` as a
`FieldAccess`, so it uses a separate lowering branch in
`compiler/ruxen_core/src/mir/lower/expr/field_access.rs`. That branch emitted
`Alloc` and called `Class_init(self)` but omitted the
`emit_class_info_init(&expr.ty, obj)` performed by the parenthesized
`Class.new()` branch. For a class including a `dispatch runtime` mixin, slot 0
therefore remained null. Concrete calls did not consult the header, so the
object appeared healthy in isolation; dispatch after moving it into an
`any Mixin` field read slot 0 as a class-info pointer and segfaulted. An
argument-taking `init` necessarily used call syntax and the already-correct
parenthesized lowering path, explaining the original asymmetry.

**Reduced repro** (no async or Stream dependency; release-e2e pin 460):
```ruxen
mixin Readable dispatch runtime
  def read -> Int
end

class Source
  value: Int
  include Readable

  def init
    self.value = 42
  end

  def read -> Int
    self.value
  end
end

class Holder
  inner: any Readable

  def init(inner: any Readable)
    self.inner = inner
  end

  def read -> Int
    (&self.inner).read
  end
end

def main
  var source = Source.new
  var holder = Holder.new(source)
  puts "value=#{holder.read}"
end
```

**Fix:** call `emit_class_info_init` immediately after allocation in the
paren-less constructor branch, before `_init`, matching all other class
allocation paths. The helper is already a no-op for structs, ordinary classes,
and classes with only static mixins. Structural MIR coverage pins the header
write for paren-less construction; the in-process compile/run test and
release-e2e case 460 prove the reduced field-move program exits cleanly and
prints `value=42`. Before the fix, both pins reproduced the missing MIR write
and the signal exit with empty output.

## Q47 · S2 — calling a `Fn[() -> any Future]` closure inside a BOOTSTRAP class POISONS global `any Future`→`Future` mixin satisfaction for every program  ✅ FIXED (2026-07-14)

Found 2026-06-19 building the §4.6 async `RetryFuture`. A bootstrap (stdlib) class
that holds a closure field/param typed `any Fn[Fn() -> any Future]` and CALLS it
(`block.()` / `make.()` — producing an `any Future` value) makes `any Future`
FAIL to satisfy the `Future` mixin for **every** compiled program, not just the
one using it. The typeck error `type \`any Future\` does not satisfy \`Future\``
then fires on unrelated user code (e.g. a future class that polls a stored
`any Future` via `.poll`), and even a previously-passing Race/spike program
breaks. The bootstrap class itself compiles clean (signature-only) — the poison
only surfaces when a user program is typechecked against the now-corrupted
satisfaction state.

**Bisected (verified 2026-06-19):**
- A bootstrap class with an `inner: any Future` FIELD alone → FINE.
- …plus a `make: any Fn[Fn() -> any Future]` FIELD (uncalled) → FINE.
- …a static/`def` that CALLS the closure (`let first = make.()`, result type
  `any Future`) → POISONS. Both the `&block: Fn[() -> any Future]` + `yield`
  form AND the plain `make: any Fn[Fn() -> any Future]` param + `make.()` form
  trigger it. Switching the closure's return type to `Int`
  (`any Fn[Fn() -> Int]`, the future-as-raw-ptr channel) → FINE.

**Root cause:** the call resolver correctly returned
`Ty::AnyMixin([Future])`, but the declared-bound seam then decomposed the
expected `any Future` and asked `MixinResolver::check_satisfaction` whether the
found `Ty::AnyMixin([Future])` satisfied `Future`. The resolver did not honor
the guarantee carried by an opaque/existential mixin value. Instead it formatted
the found type as the synthetic concrete key `"any Future"`, missed both the
nominal implementation map and the concrete method map, and emitted
`type \`any Future\` does not satisfy \`Future\``. There was no mutable
process-global satisfaction cache: the apparent cross-program poison came from
the offending bootstrap shape participating in every user compilation.

**Fix:** `MixinResolver::check_satisfaction` now accepts an `any Mixin` value
nominally, or a `some Mixin` value in static-dispatch mode, when it carries the
exact queried `MixinRef`, including its generic arguments. This is the general
existential/opaque rule; it does not special-case `Future` or associated
`Output`. Concrete values still take the existing nominal/structural paths.

**Workaround (in use, §4.6):** the `Async.retry(sched, block)` block is typed
`any Fn[Fn() -> Int]` — the closure body constructs the attempt future and returns
it AS A RAW `Int` ptr (a future value coerces to `Int` at the closure boundary,
the established `Task.spawn_raw`/`Race` value ABI). The `RetryFuture` then polls
that raw ptr through the C step `ruxen_retry_poll_inner` (wrapping
`Future_dynamic_poll`). No `any Future`-typed value appears in any CALL → no
poison. The workaround remains unchanged in this task because pins `445`–`449`
and the current retry ABI depend on it; a follow-up may simplify `Async.retry`
to a typed `any Future` block. **Severity S2** (corrupts an unrelated global
invariant from one stdlib construct — high blast radius — but a clean,
already-established workaround exists: every dyn-future value already flows as
a raw `Int` in Task/Race/Scope/Queue). Release-e2e pin
`473_any_future_call_no_poison.rx` typechecks the
bootstrap-shaped `make.()` body, then runs unrelated code that polls a stored
`any Future`; the Rust `async_surface` regression asserts the same two-shape
ordering through the production bootstrap/typeck path. Pins `445`–`449` remain
green. Toolchain: `ruxen 0.1.0` (`fix/q47`).

## Q48 · S1 — LLVM leaves FFI-aliased method calls under mangled Ruxen names, blocking optimized binaries  ✅ FIXED (2026-07-14)

`ruxen build --release` compiled String/interpolation programs to object code
but failed at the native link with undefined `_String_*` and `_Formatter_*`
symbols. The runtime archive already exported the corresponding
`_ruxen_string_*` and `_ruxen_fmt_formatter_*` C symbols, and the default
Cranelift backend linked the same MIR successfully.

**Root cause:** LLVM Pass 0 correctly declared each `FfiFuncDecl` under its
linked C symbol, but `codegen/llvm/emit/calls.rs::get_or_declare_func` ignored
the `MirProgram` and searched only the LLVM module by the incoming callee name.
Synthesized MIR paths such as interpolation and derived `Display#fmt` retain
the mangled callee (`Formatter_new`, `Formatter_write_str`,
`Formatter_buffer`) instead of passing through the ordinary lowerer's alias-map
rewrite. LLVM therefore missed the existing C declaration and its fallback
created a fresh external function under the mangled name. Cranelift did not
fail because Pass 0 registers both `FfiFuncDecl::name` (the C symbol) and
`ruxen_name` to the same imported `FuncId`.

**Fix:** LLVM's lazy resolver now looks up residual Ruxen-side and
lib-qualified FFI names in `MirProgram::ffi_libs` and returns the already
declared C-symbol `FunctionValue`, mirroring Cranelift's two-key import lookup.
LLVM no longer creates fake lib-qualified external declarations. This is a
general alias path with no String/Formatter special cases. The obsolete
wasm-only mangled Formatter wrappers from Q44 were removed because wasm now
uses the same C-symbol resolution as native LLVM.

**Pin:** `q48_llvm_ffi_alias_calls_reference_only_c_symbols` emits the LLVM
module for a direct String-construction/push plus interpolation program and
asserts that its call operands use the String and Formatter C symbols while
none of their mangled Ruxen aliases occur. It failed before the fix on
`@Formatter_new`. The existing
`string_interp_identical` fixture now also links and runs through both backends.

## Q49 · S0 — drop elaboration frees uninitialized branch locals at a shared Return  ✅ FIXED (2026-07-14)

An optimized `ruxen build --release` of the minimal `control_flow.rx` printed
the correct branch output and then aborted in the function epilogue. Each arm
created its own owned `String`, but the shared MIR return block unconditionally
called `ruxen_string_free` for both locals. Whichever arm was not taken left its
local uninitialized. LLVM's stack slot therefore held garbage and `free` raised
SIGABRT. Cranelift appeared correct only because its local model yielded a
zero/NULL value and the String free helper is NULL-safe. Zero-initializing LLVM
allocas would not solve the general case: user `<Class>_drop` glue dereferences
its receiver before the backing allocation is freed.

**Root cause:** `mir/lower/drops.rs::insert_drops` selected drop-eligible locals
with the existing exclusive-ownership analysis, but placement ignored the CFG
and appended every cleanup sequence before every `Terminator::Return`. The
ownership proof says a value may be freed if it exists; it does not prove the
value was initialized along every path reaching that return.

**Fix:** drop placement is now path-sensitive and backend-independent. A local
whose complete definition/use range is confined to one reachable, acyclic arm
block ending in `Goto` is destroyed there, in reverse declaration order, before
control reaches the merge. For a local spanning multiple blocks, a
definite-initialization dataflow check keeps the zero-cost return cleanup when
all paths initialize it; otherwise a bool flag is false at entry, set after its
initializing definition, and guards the return cleanup. The real free, user
drop glue, closure drop, and `MirInst::Drop` marker all stay on the initialized
path. `compute_dealloc_safe_locals`, async `__block_on_fut_*` force-rooting,
loop-edge cleanup, alias-source stripping, type-directed collection frees, FFI
transfer exclusions, and return-alias exclusions are unchanged.

**Pins:** the LLVM backend test lowers `control_flow.rx` and asserts each
`ruxen_string_free`/`Drop` pair is in its own arm and absent from the return
block, then runs the fixture through both backends. Release-e2e case `930`
exercises bare owned literals in both `if` and `match` arms. The LLVM comparison
suite also covers the larger class/match programs that previously crashed.

## Q50 · S2 — interpolating a method-returning-`String` formats a raw pointer  ✅ FIXED (2026-07-14)

Surfaced while fixing Q49. In `sample_program.rx`, `puts "  Failed: #{e.message}"`
(where `e.message` is a user `def message -> String`) prints a large integer that
looks like a raw pointer (e.g. `4334782334`), **nondeterministically**, instead of
the returned string. The equivalent `"  Failed: " + e.message` (Ruby `+` concat)
formats correctly, so the receiver value is fine — the defect is in the
interpolation Display path for a `#{...}` part whose expression is a **method call
returning an owned `String`** (as opposed to a bare literal or a variable).

**Root cause:** `MixinResolver::collect_item_impls` registered inline methods for
classes but omitted `HirItem::Struct.methods` and `HirItem::Enum.methods`. For
`MyErr include Error`, lookup therefore skipped the concrete inline
`message -> String` and selected `Error`'s unannotated required-method signature
(`Unit`). MIR allocated the call result temporary as `Unit`, missed interpolation's
string pass-through, and used the legacy `Int_fmt` fallback on the valid `char*`.
ASLR made that pointer-shaped integer change each run; there was no premature
drop.

**Fix:** struct and enum inline methods now enter the direct-method registry with
the same precedence as class methods. Interpolation also recovers an unresolved
or invalid call temporary from its resolved method declaration before Display
dispatch, recognizes aliased/newtype strings, and rejects `Unit`/`Never`/error
values rather than formatting them as integers. The name-based list of
"likely String methods" was removed. `sample_program.rx` now uses
`#{e.message}` again. Release-e2e `94_custom_error_enum` and the method-name-
independent `931_interpolation_method_string` pin the behavior.

## Q51 · S2 — `Enumerable#select` over-counts (returns extra elements)  ✅ FIXED 2026-07-14

The `select` body and collection iteration were correct. Instrumentation around
the push showed exactly two predicate hits, with `out.size` progressing `0 → 1
→ 2` and remaining `2` after `each`; only the caller's subsequent `evens.size`
read heap garbage (`5574` in the instrumented run; baseline runs also produced
`17082`/`5571`). The real defect was closure-capture drop elaboration. Set/Hash
`each` receive a real heap closure, and its synthesized destructor still treated
the directly captured `out` Array as owned. At `select` return it freed `out`,
even though that same source local was the method's return value.

MIR closure-drop metadata now retains each owned capture slot's enclosing source
local. Before running closure cleanup, drop elaboration finds slots whose source
flows into the function return (directly or through a returned aggregate) and
NULLs those slots, transferring the value back to the caller while the normal
destructor still frees the captures block and closure pair. Release-e2e 612 and
613 now report `evens=2` / `big=2`; pin 936 covers `select`, `reject`, `map`, and
`partition` across Array, Set, and Hash.

## Q52 · S1 — `Mutex` guard dropped in a closure/match arm crashes (exit 133)  ✅ FIXED (2026-07-14)

`let reader = { || match m.lock { Ok(g) -> g.get.clone; Err(_) -> "poisoned" } }`
then `reader.()` **deterministically crashes** (SIGTRAP, empty output). The
`Mutex[String]` guard `g` is bound in a `match` arm inside a closure, cloned out,
then dropped at arm/closure exit. Repro: release-e2e `926_mutex_string_lock_capture`
(closure capture) and `927_mutex_string_sharedsync_roundtrip`.

**Origin:** session regression from `fix/blockdrop` (`e70497d`). The compiler
binary built before blockdrop runs 926 and prints `v=captured world`; binaries
built after blockdrop on both implicit-main and Q49 exit 133. Q49 is not the
cause.

**Root cause:** `mir/lower/expr/closure.rs::synthesize_closure_drop` correctly
ran the captured class's registered user drop, but unconditionally followed it
with `heap_free_callee`. `ruxen_mutex_drop` and `ruxen_sharedsync_drop` already
free/release their handle storage, so closure teardown double-freed the captured
pointer. The empty stdout was buffered output lost when the later destructor
trapped; emitted MIR shows the guard itself had already been used correctly.

**Fix:** `mir/lower/runtime_abi.rs::drop_frees_storage` is the single list of FFI
drop hooks that consume their own storage. Ordinary and synthesized closure
drop elaboration consult it and suppress only the redundant outer free. The
`Ok` arm still materializes the cloned String, calls
`ruxen_mutex_guard_drop(g)` exactly once, then deallocates the guard spine once,
and the `Err` arm never touches `g`. Pin `937_mutex_guard_match_arm_closure_drop`
calls the reader twice: the second acquisition proves the first guard really
unlocked, so the fix cannot be a no-op/leak mask. 926 and 927 are stable across
five runs.

## Q53 · S2 — String-returning method interpolation in a generic closure prints a pointer  ✅ FIXED (2026-07-14)

This is a Q50 regression. Q50 removed the name-based
`is_inferred_string_expr` heuristic, correctly eliminating its sample-specific
method allowlist, but the replacement declaration lookup could not recover a
method called through an abstract `.each` parameter. The closure HIR kept the
parameter and call result as `Infer`/`TypeParam`, and the unresolved method had
no `DefId`; `declared_method_return_ty` therefore had neither a resolved method
nor a concrete receiver class to query. Interpolation consequently selected
the opaque `TypeParam -> Int_fmt` ABI fallback for a valid returned `char*`.

MIR inlining now types an Array `.each` parameter from the concrete collection
element, and for-loop lowering likewise consults the refined MIR iterable type
instead of falling back to the `Int` sentinel. No-parens and parenthesized
method calls use that refined receiver for concrete callee selection and type
their destination from the selected method's declared return, substituting a
generic receiver's concrete class arguments into that return. Thus
`Item_to_display`, `Todo_summary`, and any other declared `-> String` method
produce a `String` local, and interpolation takes its ordinary string
pass-through. The recovery is entirely type/declaration-driven; no method-name
list was restored. The Q50 invalid-type guard remains loud for
`Unit`/`Never`/`Error`.

Release-e2e pin `938_closure_interp_string_method` uses an unrelated `caption`
method inside `.each`. The flagship `sample_program_identical` differential and
the full LLVM fixture verifier now pass, so Cranelift and LLVM agree on the
titles/summaries emitted from generic closures and refined for loops. Existing
case `101_two_type_params` also confirms that `Pair[Int, String]#second_ref`
instantiates `&B` as `&String` before interpolation.

## Q54 · S2 — wasm: `ruxen_fmt_formatter_write_str` call-site return width (i32) mismatches the bundled fmt.c definition (i64); wasm-ld traps EVERY string interpolation with an `unreachable` stub  ✅ FIXED 2026-07-22

Found 2026-07-22 re-verifying canvas's web backend (canvas `docs/ROADMAP.md` →
Web backend, the L839 "render() traps" item) after Q44/Q48 bundled the fmt
runtime into wasm. Q44/Q48 correctly removed the `Formatter_*` host imports (the
fmt runtime `library/std/fmt/runtime/fmt.c` is now bundled), but on the wasm
target the interpolation CALL-SITE and the runtime DEFINITION disagree on
`ruxen_fmt_formatter_write_str`'s RETURN width:

- **Definition** (`library/std/fmt/runtime/fmt.c:92`):
  `int64_t ruxen_fmt_formatter_write_str(RuxenFormatter *f, const char *s)` — i64 result.
- **Call-site** (wasm codegen of the interpolation lowering of the Ruxen decl
  `library/std/fmt/src/lib.rx:51`,
  `def write_str as "ruxen_fmt_formatter_write_str"(s: &String) -> Result[nil, FmtError]`):
  lowered with an **i32** result (the `Result[nil, FmtError]` width).
- The native path is already correct — `compiler/ruxen_core/tests/runtime_abi_derivation.rs:61`
  maps this symbol to `([I64, I64], Some(I64))` — so the **wasm** call-site is
  NOT consulting that runtime-ABI derivation, leaving an i32-vs-i64 return
  mismatch that native never sees.

Consequence at link time:
```
wasm-ld: warning: function signature mismatch: ruxen_fmt_formatter_write_str
>>> defined as (i32, i32) -> i32 in <app>.o
>>> defined as (i32, i32) -> i64 in <fmt runtime>.o
```
wasm-ld links the mismatching reference to a generated
`signature_mismatch:ruxen_fmt_formatter_write_str` function whose body is
`unreachable`, so the FIRST interpolation call traps at runtime (`RuntimeError:
unreachable`).

**Minimal repro** (compiles cleanly, traps at run time):
```ruxen
def render -> Int
  let n = 5
  let s = "count: #{n}"
  puts(s)
  0
end
```
```
RUXEN_WASM_CLANG=/opt/homebrew/opt/llvm/bin/clang \
  ruxen compile repro.rx --target wasm32-unknown-unknown -o repro.wasm
llvm-nm repro.wasm | grep signature_mismatch
# → 000000c3 t signature_mismatch:ruxen_fmt_formatter_write_str
```
Calling the exported `render()` from a JS host traps `unreachable`.

**Blast radius:** every wasm program that interpolates a string — which is the
whole point of quiver's reactive `dyn_text` (`"count: #{n.get(ui)}"`). So
canvas's web-backend quiver-counter proof (`canvas/web/verify_counter.mjs`)
traps, while the interpolation-free `draw.wasm` (`canvas/web/verify.mjs`) renders
pixel-exact. This is what still blocks a real quiver app in the browser.

**Root cause (confirmed).** The Ruxen runtime convention is that a pointer-like
Ruxen return type (`Result`/`Option`) crosses the C-ABI boundary as a
POINTER-WIDTH value (`void *`), which the compiler's `ty_to_llvm`/`ty_to_cranelift`
lowering of that type (a pointer) matches on every target: i64 on 64-bit, i32 on
wasm32. Of the ~30 `Result`/`Option`-returning FFI helpers in the stdlib, 28
follow that convention (`ruxen_file_open`, `ruxen_vec_pop`, `ruxen_hash_get`,
`ruxen_stdout_write_str`, … all return `void *`). The two fmt helpers
`ruxen_fmt_formatter_write_str`/`_write_char` were the ONLY violators: they
returned a bare `int64_t` tag (0/1). On a 64-bit target `int64_t` and a pointer
are both 64 bits, so the anomaly was invisible; on wasm32 the pointer is i32 while
`int64_t` stays i64, so `wasm-ld` linked the i32 call-site against the i64
definition through a generated `signature_mismatch:` `unreachable` stub, and the
first interpolation call trapped.

Note the anomaly is NOT derivable from the `.rx` type: `write_str -> Result[nil,
FmtError]` and `stdout_write_str -> Result[nil, IoError]` have identical Ruxen
return types but different C return widths (`int64_t` vs `void *`). So a
compiler-side "map `Result`→i64 on wasm" rule would fix the two fmt symbols and
newly BREAK the 28 conforming `void *` ones. The correct fix restores the
runtime's own convention at the two violation sites.

**Fix (landed).** `library/std/fmt/runtime/fmt.c`: `ruxen_fmt_formatter_write_str`
and `ruxen_fmt_formatter_write_char` now return `void *` (`NULL` = Ok tag, `(void
*)1` = FmtError tag) instead of `int64_t`. Zero compiler-code changes, so the
native backend is byte-identical: the ABI-classification parity oracle
(`runtime_abi_derivation` / `runtime_abi_parity`) stays green (both still derive
`I64` for these symbols on native, where `void *` == `int64_t` width and returns
in the same register), and the full native release-e2e passes. The interpolation
lowerer discards the tag, so no consumer sees the representation change. This is
the direct sibling of Q44/Q48 (fmt-on-wasm ABI): the NAME was resolved by Q48, the
SIGNATURE by Q54.

**Reproducible gate (no wasm release-e2e harness exists).** The release-e2e
harness has no wasm mode, and the `tests/wasm_codegen.rs` lane uses an empty
stdlib bootstrap (no fmt link), so neither can host a faithful interpolation-link
pin without new infrastructure. The reproducible check is the `llvm-nm` gate on
the interpolation repro built for wasm32:
```
RUXEN_WASM_CLANG=/opt/homebrew/opt/llvm/bin/clang \
  ruxen compile repro.rx --target wasm32-unknown-unknown -o repro.wasm
llvm-nm repro.wasm | grep signature_mismatch      # must be EMPTY post-fix
```
plus instantiating the module in Node and calling the exported `render()` (must
return 0 without a trap). Verified against both the source tree (via
`RUXEN_STDLIB_ROOT`) and the installed toolchain.

Cross-linked from canvas `docs/ROADMAP.md` (Web backend → L839).
Toolchain: `ruxen 0.1.0`.

## Q55 · S2 — miscompile: `x = f(x)` where `f` takes `x` by value AND returns it silently corrupts `x` (native)  ✅ FIXED 2026-07-22

Found 2026-07-22 adding rondo's structured-logging `finish` consolidation. A
`var` holding a class value with drop semantics (fields include an FFI-backed
`String`/`Hash`), reassigned from a function that takes **that same var by value
and returns a value of the same type**, comes back corrupted — the result's
fields read as garbage. This is a silent miscompile (no diagnostic), NOT the
`def var` receiver-builder form (`r = r.with_status(201)` borrows `self` and is
safe and used throughout rondo).

**Minimal repro** (single file; compiles cleanly, prints the wrong value):
```ruxen
class Payload
  n: Int
  tag: String
  def init(v: Int)
    self.n = v
    self.tag = "p"
  end
end

def wrap(p: Payload) -> Payload
  var r = p
  r
end

def main
  var x = wrap(Payload.new(222))
  x = wrap(x)              # rebind the SAME var through a by-value fn
  if x.n == 222
    puts "PASS n=#{x.n}"
  else
    puts "FAIL n=#{x.n}"   # ← actually prints: FAIL n=3
  end
end
```
`ruxen compile` + run prints `FAIL n=3` — `x.n` was 222 before the rebind and
reads as `3` after. (The `tag` String field is what makes the value
drop-bearing; a plain-`Int`-only class does not need to reproduce it, but the
corruption is on the whole value.)

**Production evidence.** In rondo, `finish` first written as
```ruxen
var r = self.cors_response(resp)
r = self.with_request_id(r, &req.request_id)   # by-value arg, rebind to r
```
made EVERY dispatched response report a nonsense status (200 → `4`, 404 → `0`)
across the whole suite; rewriting to the single-binding nested form
`var r = self.with_request_id(self.cors_response(resp), &req.request_id)` fixed
all 94 tests. Documented as rondo **W24**.

**Root cause (confirmed).** `x = f(x)` lowers to a pre-reassign
`ruxen_dealloc(x)` before `Assign x = <call result>`. When `f` returns the very
object it was handed for `x`, that dealloc frees the object the assign then
re-reads, a use-after-free. The post-lowering fixup `elide_returns_self_realloc`
(`compiler/ruxen_core/src/mir/lower/drops.rs`) is meant to remove exactly that
dealloc, but it only recognized callees whose returning block was LITERALLY
`Return(Some(Use(params[0])))`: (1) `wrap` returns a local `Move`d from its
parameter, not the parameter literally, so it was never recognized; (2) it only
looked at parameter index 0, missing by-value args at any other position (e.g.
rondo's `with_request_id(self, resp, rid)` returns index 1). So the guarding
dealloc was left in place and the UAF fired. The receiver-builder form
`r = r.with_status(201)` returns `self` (index 0) literally, so it was already
covered and stayed correct.

**Fix + pin.** Generalized the detection to a `name -> returned-param-index`
map: each returning block's local is traced back to a parameter index through
intra-function `Move`/`Copy`/`Assign(Use)` aliasing (seeded `params[i] -> i`,
revoking on any non-aliasing redefinition); a function qualifies with index `k`
only when every returning block agrees on that `k`. The elide site now removes
the dealloc only when the callee returns index `k` AND `x` was passed at exactly
that position (`args[k] == Use(x)`), which is stricter than the prior
"any-arg-aliases" check and also fixes a latent leak. Indirect (closure)
dispatch keeps the documented bounded-leak fallback. Pin case:
`tests/release-e2e/cases/948_by_value_rebind_no_uaf.rx` (asserts `PASS n=222`);
receiver-builder cases stay green.

**Q55b (memory-safe leak residual)  ✅ FIXED 2026-07-22.** The original Q55
returned-param detection traced provenance with a LINEAR walk over blocks in
vector order (last-writer-wins), not a CFG-aware meet-over-paths, so at a
control-flow merge it could misattribute. FIX: `elide_returns_self_realloc` now
runs a forward dataflow over the CFG (`block_successors` + `transfer_param_prov`
in `drops.rs`), meeting per-local provenance at merges by intersection with value
agreement (a local survives only when every predecessor agrees on the same
parameter index), iterated to a fixpoint. The merge below now drops the returned
local, so `pick` is correctly NOT treated as returns-param (`pick`'s
`dealloc(x)` is restored, verified via `--emit=mir`), while a single-path
`var r = fresh; r = a; r` still resolves to the parameter (no UAF false-negative,
verified by the `ftp` repro `PASS n=222`). Gate: release-e2e `1 passed` (all
~354 fixtures), `runtime_abi_parity` 3, `drop_fixtures` 33, `w24_a` `PASS n=222`.
The originally-reported shape:

```ruxen
def pick(a: Payload, cond: Bool) -> Payload
  if cond
    Payload.new(999)   # fresh on the `then` path
  else
    a                  # parameter 0 on the `else` path
  end
end
```

The if-expression's value merges into one local; in block-vector order the
`then` block (fresh, revokes the local) is processed before the `else` block
(sets the local back to param 0), so the trace ends with the returned local at
param 0 and classifies `pick` as always-returns-param-0. A caller `x = pick(x,
cond)` then elides `dealloc(x)` unconditionally (verified via `--emit=mir`: no
dealloc between the `pick` call and the reassign). When `cond` takes the fresh
branch at runtime, `pick` returns a NEW object, so `x`'s old object is never
freed: a LEAK. This is memory-safe (never a use-after-free or double-free, since
elision only ever REMOVES a free), and it is not hit by any release-e2e case or
by rondo's suite. It was not introduced as a regression in the UAF direction:
the old detection would not have recognized `pick` either (the return local is
not `params[0]` literally), so before the fix this shape kept its dealloc.

Note the two cheap "hardenings" are both WRONG: a "must-alias across ALL
definitions of the returned local" check would disqualify a single-path
`var r = fresh; r = a; r` (returns param 0 always) and REINTRODUCE a UAF there
(worse). The sound fix is a proper CFG dataflow: at each return, the returned
local is param `k` only if EVERY predecessor path yields `k` (meet lattice
`param k` / `⊤`), which correctly disqualifies `pick` (leak avoided) while still
eliding the single-path `fresh-then-param` case (UAF avoided). Deferred as a
scoped follow-up rather than rushed inline, because the current state is
memory-safe and a wrong dataflow rewrite of a memory-safety pass is a UAF risk.

Cross-linked from rondo `docs/ruxen-issues.md` W24.
Toolchain: `ruxen 0.1.0`.

## Q56 · S2 — async client I/O via `block_on` (no worker reactor) SILENTLY does nothing

Found 2026-07-23 building the first HTTP client for the data-apps North Star
(`../NORTH-STAR.md`). A standalone program that does async CLIENT I/O through
`block_on` fails silently: the connection appears to open, `write` returns a
byte count as if it sent, but the bytes NEVER reach the peer, and subsequent
reads time out. rondo only ever runs async I/O inside its per-worker reactors
(`spawn_async_workers` + `Task.spawn_raw` + the poll loop), so the standalone
`block_on(AsyncTcpStream.connect(...))` + `block_on(s.write(...))` path was never
exercised.

**Repro** (server logs prove it):
```ruxen
match block_on(AsyncTcpStream.connect(&"127.0.0.1:8799"))
  Ok(stream) ->
    var s = stream
    match block_on(s.write(&"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
      Ok(nw) -> puts "wrote #{nw} bytes"   # prints "wrote 54 bytes"
      Err(_) -> puts "write err"
    end
    match block_on(s.read_with_timeout(65536, 3000000000))
      Ok(c) -> puts "read #{c.size() as Int}"
      Err(_) -> puts "read ERR/timeout"    # prints this: nothing ever arrives
    end
  Err(_) -> puts "connect err"
end
```
The peer (a logging HTTP server) records the request from `curl` but NOT from
this program, so the 54-byte `write` was never flushed to the socket.

**Root cause hypothesis.** `block_on` drives ONE future on a minimal executor;
async net I/O completion depends on a reactor that registers the socket fd and
services readiness (edge-triggered), which the worker loop provides but a bare
`block_on` does not. The async write future's `poll` reports completion (returns
the byte count) without the reactor having actually driven the non-blocking send
to the wire.

**Severity.** S2 for correctness UX: it is a SILENT wrong result (write claims
success), which is a footgun for any app doing client-side networking outside
rondo's server. Either `block_on` should set up a reactor that services client
socket I/O, or the async net methods should error clearly when driven without a
reactor.

**Workaround (in use).** Use the SYNC `TcpStream` for a request/response client:
`TcpStream.connect` + `write(&Array[Int])` + loop `read(&var Array[Int])` (reads
APPEND to the buffer) until `Ok(0)`, then `String.from_bytes`. Blocking syscalls
need no reactor and work correctly. This is what the data-apps HTTP client uses.

Toolchain: `ruxen 0.1.0`.

## Parked Q-candidates (ergonomics / features — not bugs; from the 2026-06-09 GUI push)

Documented at their source, listed here so they aren't lost:

- **DSL P2** — top-level closure-literal param inference (`App.build({ |ui, root| … })`
  params stay `?T`); **P3** — auto-reborrow a `&var` used more than once;
  **P4** — non-Copy class call-result as by-value method arg segfaults (also a
  landmine). → `quiver/docs/decisions/dsl-ergonomics.md`.
- **GPU multi-window** — `gl_get_proc` is per-process/current-context; concurrent
  multi-GL-context windows need explicit make-current per window per frame.
  → `canvas/docs/MULTIWINDOW.md` ("Language gap").

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

## Q57 · S2 — a mixin-DEFAULT method that takes a trailing block silently does nothing (native)

**FIXED 2026-07-28** (typeck; pins `tests/release-e2e/cases/980`–`982`,
CHANGELOG "Mixin-DEFAULT methods that take a block…"). Three coordinated typeck
changes landed the fix direction below: (1) the signature-based closure-param
seeding (`expr.rs`) now PINS the block param's `Self`-substituted concrete type
into the symbol table so `x.field` in the block body resolves; (2) the method-
call return path passes the trailing block as the effective final arg to
`resolve_method_call`, so the block-taking mixin default matches arity in
`lookup_method_with_args` instead of degrading to a fresh `Infer`; (3)
`resolve_method_call`'s trait-default arm runs the same receiver-generic
substitution + method-generic harvest as `bridge_builtin_method`, so the
`then`-shape's `U` binds from the closure's inferred return. The `tap`-shape,
`then`-shape, and `Enumerable#map`-on-a-user-class repros below all compile and
run correctly now. (A residual, SEPARATE pre-existing bug remains: `each` over
an array of user-class *elements* can segfault at runtime — tracked apart from
this typeck fix.) Original analysis retained below.

**Pre-existing** (reproduces on 85b24f5a, before the Ruby-parity session — not
introduced by `tap`/`then`). A method declared on a `mixin` and inherited by a
class via `include`, when it takes a closure/`&block` param and is CALLED with a
trailing `do…end` block, does not run its body: the call produces empty output
(and can segfault if its result is then used, e.g. printed).

Repro:
```rx
mixin Wrappable
  def wrap(f: any Fn[Fn(Int) -> nil]) -> Int
    puts(100)   # never prints
    42
  end
end
class Widget
  v: Int
  include Wrappable
  def init(@v: Int)
  end
end
def main
  let w = Widget.new(4)
  let r = w.wrap do |x| puts(999) end
  puts(r)       # empty output; exit 0 or 139
end
```
A mixin default WITHOUT a block param (`def describe -> Int; 77; end`) dispatches
fine, and an OWN class method with a block works — only the
mixin-default-plus-trailing-block combination is broken.

**ROOT CAUSE (traced 2026-07-28).** Two distinct call-resolution paths in typeck
that do NOT agree on closure-param handling:

1. **Closure-param `Self` is not substituted for a mixin-default call.** An OWN
   class method call seeds its closure argument's type from the (already
   `Self`→receiver-substituted) declared param type at
   `typeck/infer/expr.rs:711-716` (`substitute_generics_in_return` + then
   `seed_closure_param_types`). A MIXIN-DEFAULT method call resolves through a
   SEPARATE path (`select_class_method` returns `None` for an inherited method,
   so that seeding loop is skipped; the call is typed via the
   `bridge`/`lookup_method_with_args` mixin path). That path does NOT seed the
   block's closure parameter, so a `def m(f: any Fn[Fn(&Self) -> nil])` leaves
   the block param at the abstract `Self`/`?T`. The block body's `x.field` /
   `x.method` then mangles to an unresolvable `?T<id>_<name>` symbol and codegen
   errors, or the value is type-confused. Repro (fails with
   `no runtime symbol for ?T::n`):
   ```rx
   mixin Tappable
     def do_it(f: any Fn[Fn(&Self) -> nil]) -> Self
       f.(self)
       self
     end
   end
   class Box
     n: Int
     include Tappable
     def init(@n: Int)
     end
   end
   Box.new(5).do_it do |x| puts(x.n) end   # ?T::n
   ```
   (A block param that does NOT dereference `Self` fields — `do_it { |x| puts(1) }`
   — happens to link, which is why the trivial `Doer` probe passed.)
2. **A mixin default that returns `Self` or the block result is mis-lowered** —
   the `then`-shape (`def m[U](f: any Fn[Fn(&Self) -> U]) -> U; f.(self); end`)
   returns empty. Related to the same mixin-default monomorphization not carrying
   the concrete `Self`/`U` bindings through to the body.

**Fix direction (the generic mechanism, per the `.rx`-first rule in `CLAUDE.md`).**
Make the mixin-default method-call path seed its closure argument from the
`Self`→receiver-substituted param type, exactly like the own-method path — ideally
by factoring the own-method seeding loop (`expr.rs:711-716`) into a shared helper
both paths call, so a `Self`-typed closure param is concrete for own AND inherited
methods. This is what unblocks `tap`/`then` (and `Enumerable#map` on a USER class,
which fails for the same reason) as pure `.rx` mixin methods. It needs its own
careful pass (typeck seam + MIR mono + the full gate + review); it is NOT a
per-feature special-case.

## Q58 · S3 — `alias` to a method on a GENERIC builtin head doesn't canonicalize through the generic suffix (link error)

**FIXED** (pin `tests/release-e2e/cases/990_alias_generic_builtin_head.rx`).

Symptom: an `alias new old` inside a generic builtin-head class (`class
Hash[K, V]`, `class Array[T]` when the receiver carries concrete generic args)
did not rewrite the call-site mangled name to the canonical method.
`h.has_key?(k)` on a `Hash[String, Int]` mangled to `Hash[String, Int]_has_key?`,
which never canonicalized to `Hash_key?`, so the linker reported an undefined
`_Hash[String, Int]_has_key?`.

Repro (link-failed before the fix):
```rx
class Hash[K, V]
  # ... (stdlib) ...
  alias has_key? key?
end
# h.has_key?("a")  =>  undefined _Hash[String, Int]_has_key?
```

Root cause: `mir/lower/mod.rs::select_method_symbol_name` canonicalized the
alias name correctly (`canonical_method_name` already peels `[...]` for the
alias-map lookup), then threw the result away. Its candidate scan matched the
parent def's BARE class name against the call site's INSTANTIATED receiver name,
so a generic head matched zero defs and the function returned `None`. The caller
then fell back to the raw spelling, because typeck does not pin `*method` for a
builtin head. Only the paren-less `FieldAccess` path escaped, since it calls
`canonical_method_name` directly with no candidate scan, which is why
`Array`'s `alias to_a clone` appeared to work and `a.to_a()` did not.

`method_trailing_default_sentinels` (same file) carried the identical bug on a
parallel path: for every generic head it found no signature and returned zero
default sentinels, so a paren-less call to a method with trailing defaults
emitted one argument too few and aborted in the Cranelift verifier
(`mismatched argument count ... got 1, expected 2`). That half is latent today:
no stdlib collection method declares a trailing default, and `extension
Array[T]` is rejected, so it was reproduced with a temporary probe method.

Fix: both sites now share `Lowerer::method_defs_on_class`, which scans the
receiver's exact class name first and falls back to its generic-free base
(`Hash[String, Int]` → `Hash`) only when the exact pass finds nothing. That is
the same full-then-base order `SymbolTable::canonical_method_name` already used
for the alias map, so the two agree on what "this class" means. No method or
type name is hardcoded. `library/std/map/src/lib.rx` reverted its `has_key?` /
`include?` / `member?` / `each_pair` delegating bodies to one-line `alias`es.

Still NOT expressible as an `alias`, for an unrelated reason: the operator
methods on `Array`/`Set` (`def +`, `def |`, `def &`, `def -`). The builtin-head
operator route (`mir/lower/expr/binops.rs::builtin_head_has_op_method`) is a
name-only existence check over `DefKind::Method`, and an `alias` records no def,
so an aliased operator would fall through to the machine floor and bit-op two
heap pointers. Those stay real delegating bodies.

## Q59 · S2/S3 — five `.rx`-authoring miscompiles and limits found building the String Ruby-parity batches (B08-B11)

**OPEN.** Each has a clean workaround, all of them in use in
`library/std/string/src/string.rx`. Grouped because they were all found in one
pass and share a theme: a value that is correct when bound to an explicitly
typed local is wrong when used inline. (If a parallel batch already claimed
`Q59`, renumber; the content is what matters.)

**(a) S2 — a NEGATIVE default argument does not fill on a builtin-head method.**
```rx
class String
  def rindex(sub: &String, offset: Int = -1) -> Option[Int]
    # ...
  end
end
# "hello".rindex("l")  =>  Cranelift verifier: "mismatched argument count
#                          for `v6 = call fn3(v3, v5)`: got 2, expected 3"
```
A POSITIVE default (`pad: String = " "`, `base: Int = 10`) fills correctly, and
a negative default on a FREE function (`def neg(a: Int, b: Int = -1)`) also
works, so the gap is the negated-literal default expression in the builtin-head
fill path added by `09580b95` (`infer/expr.rs` `selected_method` +
`append_method_default_args`). **Workaround:** declare two arities instead
(`def rindex(sub)` delegating to `def rindex(sub, offset)`).

**(b) S2 — indexing an `Array[Int]` / `Array[Char]` / `Array[UInt8]` returned
INLINE by a call yields garbage.**
```rx
puts "#{"ab".chars[0]}"      # => 4352431440   (a pointer, not 'a')
puts "#{"ab".bytes[0]}"      # => 45852142432
let cs = "ab".chars
puts "#{cs[0]}"              # => a            (correct once bound)
```
Pre-existing, not introduced by the String batches: it reproduces on the FFI
`chars` / `bytes`. `Array[String]` is unaffected (`"a,b".split(",")[0]` is
correct), so it looks like the scalar-element read off a temporary that has
already been released. **Workaround:** bind the array to a local first.

**(c) S2 — `<Char> as Int` mis-lowers in argument and tail-expression
position.**
```rx
def pa -> Int
  let c = self.chars[0]
  c as Int                   # => 4337015680
end
out.push(c as Int)           # => pushes garbage
let n: Int = c as Int        # => 65   (correct)
```
Same family as the "rebox a scalar `self` first" rule (`let n = 0 + self`).
**Workaround:** always bind through an explicitly typed local.

**(d) S2 — `String + &String` (a borrowed PARAMETER) concatenates garbage.**
```rx
def probe(other: &String) -> String
  var r = ""
  r = r + other              # => "AIySayAMGGARctFT"
end
```
`r + "literal"`, `r + owned_string` and `r + "#{other}"` are all correct, so the
gap is specifically the borrowed-parameter operand on the right of the concat
route. **Workaround:** `r.push_str(other)`, which mutates `r` in place and is
also cheaper than rebuilding.

**(e) S3 — an untyped `[]` literal cannot disambiguate an overload.**
```rx
"hello".start_with?([])              # => true   (picks the &String overload)
let none: Array[String] = []
"hello".start_with?(none)            # => false  (correct)
```
Argument-type-based overload resolution otherwise works well (it is what lets
`gsub` / `sub` / `split` / `index` take either a `&String` or a `&Regex` under
one Ruby name), but an empty array literal carries no element type for it to
match on. **Workaround:** bind a typed local.

## Q60 · S2/S3 — three limits found landing the `Hash` key-kind protocol (B73)

Found while replacing the `Hash` / `Set` key-kind heuristic with a compiler-
supplied descriptor (B73, CHANGELOG `[Unreleased]`, pins `1720`-`1724`). B73
itself is FIXED; these are the neighbours it exposed. (If a parallel batch
already claimed `Q60`, renumber; the content is what matters.)

**(a) S2 — a static constructor on a generic builtin keeps its RIGID type
parameters at the expression node.** `Hash.new` is declared
`def self.new as "ruxen_hash_new" -> Hash[K, V]`. A static call has no receiver,
so `substitute_generics_in_return` has nothing to substitute against, and the
expression node's type stays `Hash[K, V]` even where the binding is concrete:

```rx
var h: Hash[String, Int] = Hash.new
# local `h`   : Map(String, Int)      ← correct
# the ctor's temp: Map(TypeParam K, TypeParam V)   ← imprecise
```

Unification does not object, because a rigid parameter is not an inference
variable and there is nothing to bind. Anything downstream that reads the
INITIALIZER's own type rather than the binding's therefore sees `K`, not
`String`. The general fix is to thread an expected type through binding,
argument and return checking, or to substitute a static ctor's return against
the expected type at the call.

**OPEN, and severity dropped from S2 to S3**, because B73 no longer depends on
this being right. The key-kind protocol installs its descriptor at USE sites as
well as construction sites (`mir/lower/key_desc.rs`, the `KEYED_TABLE_OPS`
route), and an operation's RECEIVER is concretely typed in exactly the
positions a constructor's result temp is not. `f(Hash.new)` is keyed by the
`insert` inside `f`; pin `1725` holds that.

**A `refine_rigid_generics` pass was written for this and then deliberately
removed** — it refined the initializer from a concrete annotation on the `let`
and assignment paths (~190 lines in `typeck/infer/`). It was measured to be
non-load-bearing once the use-site route existed: with it disabled, release-e2e
stayed at 551 pass / 0 fail and the narrow gate at 655 pass / 0 fail, including
a `self.data = Hash.new` class-field-init probe. Refining expression types
after unification touches every annotated binding in every program, which is a
large blast radius to carry for no measured benefit. If a future consumer
genuinely needs a static constructor's own expression type to be precise,
reintroduce it deliberately with a test that fails without it — do not assume
it is still needed because this entry mentions it.

**(a2) S1, FIXED — the parser erased explicit generic arguments on a
static-call receiver.** `parse_type_expr_primary`
(`parser/expr/atoms.rs:471`) read the `[…]` after a type identifier and threw
it away:

```rust
let _generic_args = self.parse_generic_args();
// Fall through to normal handling as if we had just the TypeIdentifier.
```

The stated reasoning was that generic args are inferred from the
constructor/method arguments. True for `Point[Int].new(3)`; false for any
NO-ARGUMENT constructor. `Hash[String, Int].new` therefore produced a receiver
typed with `class Hash[K, V]`'s declared rigid parameters, and
`substitute_generics_in_return` bound the identity map `{K → K}`, so the call's
type stayed abstract permanently. Verified consequences, all on the pre-B73
build:

* `Hash[Int, Int].new` then `insert(5000, …)` → **SIGSEGV**, while
  `insert(7, …)` worked. The key kind was being guessed from the key's bits
  because no static type was available.
* `h["a"]` on such a table typed as the abstract `V` rather than `Int`.

So this spelling was never correctly typed; B73 replaced the lucky guess with a
demand for a real static type and turned it into a loud panic, which is how it
was found. **Fixed** by carrying the written path as
`ExprKind::GenericTypeRef { name, ty }` and resolving it through ordinary type
resolution, so `Hash` / `Array` / `Set` need no per-collection special-casing.
Three things worth knowing if you touch it:

* The refinement is scoped to the BUILTIN type constructors (the
  `DefKind::Variable` entries `register_type_constructors` installs in VALUE
  scope). A user generic class is a `DefKind::Class` reached through type
  scope; its `.new` goes down the constructor path and already infers its
  parameters from the call's arguments. Routing it through the new node broke
  cases `65`, `73`, `101` and `990` on the first attempt, so it falls back to
  resolving exactly as the bare type name did. **`Pair[Int, String]` is
  therefore still not refined as a TYPE** — it just does not need to be today,
  because it always has arguments to infer from. A no-argument static method on
  a user generic would hit the same class of problem.

* The resolver registers the INSTANTIATED constructor as its own def rather
  than setting `expr.ty`. Typeck's `VarRef` arm (`typeck/infer/expr.rs:125`)
  re-derives an expression's type from its `DefId` unconditionally, so an
  in-place refinement is discarded. Anything else that wants to refine a
  `VarRef`'s type has the same problem and needs the same shape.
* Both formatters print the arguments back (`ruxen fmt` verified idempotent).
  Dropping them there would rewrite the program into the un-inferable shape,
  which is the fmt-destructiveness class (Q23/Q30/Q34).

Pins `1728` (both spellings × every key kind, `Set`, `with_capacity`) and
`1729` (class field, function return, closure body, array element).

**(b) S2 — `Set[String]` set-algebra results double-free at scope exit.**
**FIXED (2026-07-30), together with B86 — they were the same defect.**
Verified 2026-07-29 and **pre-existing** (reproduces identically before B73):

```rx
var a: Set[String] = Set.new
a.insert("red")
var b: Set[String] = Set.new
b.insert("blue")
let u = a.union(&b)
puts "#{u.size}"        # never prints — exit 133 (SIGTRAP, malloc abort)
```

`ruxen_set_union` / `_intersection` / `_difference` shallow-copy their element
words, so the result set ALIASES its source's heap strings — the documented v1
contract, same as `ruxen_hash_clone`. But the drop selector gives both the
source and the result `ruxen_set_drop_string` (`drops.rs::heap_free_callee`,
`Ty::Set(Ty::String)`), so every shared `char*` is freed twice. `Set[Int]` is
fine (no per-element drop) and so is a `Set[UserStruct]` (the selector picks the
spine-only `ruxen_set_free`); only heap-owning element types are exposed.
**Workaround:** none at the `.rx` level. Pin `1724` deliberately uses a struct
element rather than a `String` so it does not depend on this. The fix is either
a deep copy in the three set-algebra helpers or an aliasing-aware drop.

**Resolution — the deep copy, and the diagnosis above generalizes.** This is
not a `Set` bug; it is one instance of a contract violated by five helpers.
Any callee listed in `FRESH_ALLOC_CALLEES` has its result deep-dropped by the
helper `heap_free_callee` picks, so a result built from a borrowed source's
element pointers is a double free by construction. `ruxen_vec_skip`
(`Array#drop`, filed as B86), `ruxen_vec_chain` and `ruxen_set_from_iter`
(`to_set`) were failing identically and were NOT filed anywhere.

The three set-algebra helpers now insert through `ruxen_set_insert_owned`,
which clones per the table's own B73 `key_kind` — a Set's element IS its key,
so the descriptor needed was already on the table. The `Vec` side needed a new
`RUXEN_ELEM_*` descriptor injected by MIR. Full audit table and per-helper
verdicts are in `docs/ruby-parity/QUEUE.md` under B86; the ownership contract
is now a doc comment on `FRESH_ALLOC_CALLEES` itself. Pins `1870` / `1871`.

Note for whoever reads this next: the "aliasing-aware drop" alternative above
was considered and rejected. Dropping a helper from `FRESH_ALLOC_CALLEES` (or
making its result's drop spine-only) does stop the crash, but it converts it
into a silent leak, and for a result derived from a TEMPORARY source
(`["a","b","c"].drop(2)`) it would leave the result holding freed pointers —
trading an abort for a use-after-free.

**(c) S4 — `Array#to_h`'s `lib` declaration launders its return through `Int`.**
`library/std/array/src/lib.rx:106` reads
`def to_h as "ruxen_hash_from_iter" -> Int`, with the header comment at
`:49-50` explaining why: the result key/value types come from the receiver's
TUPLE element and are "not expressible" in the current return-type surface.

**The user-visible surface is fine** — the `collections.rs` resolver runs ahead
of the builtin bridge and supplies the real `Hash[K, V]`, so `[("a", 1)].to_h`
does have `size` / `key?` / `get` on it (pin `1726`). An earlier draft of this
entry claimed the value arrives as a bare `Int` with no `Hash` methods; that is
wrong, and the pin disproves it.

What the laundered decl DID break was B73's descriptor: the construction site
read the key type off the RESULT local, which for `to_h` is `Int` and says
nothing about the key. Fixed by reading it off the SOURCE instead — a
`from_iter` constructor's input is always a concrete `Array`, and collecting it
is what fixes the key type (`key_desc.rs::key_ty_from_iterable`). That is the
better witness for both `from_iter` forms regardless of the decl, because these
two constructors populate their table INSIDE the C runtime, where no use-site
`ensure` can reach it. The `-> Int` decl is still worth correcting when per-`T`
mono (E1) allows it, but nothing depends on it now.

## Where the workarounds live (grep-able)

- `quiver/CLAUDE.md` — "Ruxen v1 landmines" section (the working ruleset)
- `quiver/docs/DSL.md`, `quiver/docs/REACTIVITY.md` — API deviations + why
- `tally/src/main.rx` — `to_f32` (Q5), PPM-header hoist (Q6), plain paint
  loop (Q17), `stdin()` (Q19)
- `canvas/tests/window_live.rx` — brace-arm rewrite (Q7)
- `canvas/src/lib.rx` `show_window` — the `&String` FFI calling shape note
