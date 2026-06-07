# Spec — Generic compiler / zero hardcoded stdlib in Rust

**North star:** the compiler is GENERIC — it has no knowledge of specific stdlib
types or methods. The stdlib defines everything (`Int`, `Float`, `String`,
`Array`, `Mutex`, …, plus operators and shared method families) in `.rx`, over
the compiler's generic mechanisms. Adding a stdlib class or method requires
ZERO compiler (Rust) edits. No `#[lang]`/registry mechanism — generic language
features only.

## The dividing line (what stays in the compiler vs the stdlib)
**Compiler-only (allowed):**
- Generic mechanisms: class/method definition, generics, mixins/traits, FFI +
  `repr`, derive machinery, closures.
- Machine primitives: i64, f64, bool, raw-ptr, byte-slice, array-slice.
- Syntactic protocols: a literal lowers to a *primitive*; an operator desugars
  to a *method call by symbol* (`a + b` → `a.+(b)`); operator precedence.

**Stdlib (`.rx`) — must NOT live in compiler Rust:**
- Every named type (`Int`/`String`/`Array`/`Mutex`/…) and all their methods.
- Operator method bodies (`def +`, `def []`, `def []=`, `def <=>`, …).
- Shared method families via mixins (see below).

**Acceptance:** `grep` for any stdlib class name (`"String"`, `"Array"`,
`"Mutex"`, `"Duration"`, …) across `compiler/ruxen_core/src/` is empty, modulo
machine primitives and the operator-desugar protocol. `method_resolvers/mod.rs`
registers only `declared_method_resolvers` + `builtin_bridge`.

## Key architecture decisions (locked)
1. **Method resolution from `.rx`** via `builtin_bridge` (done for most types).
2. **Operators are overridable methods** named by the symbol: `a + b` → `a.+(b)`,
   `a[i]` → `a.[](i)`, `a[i]=v` → `a.[]=(i,v)`, `-a` → `a.-@()`, `!a` → `a.!()`.
   **Desugar is POST-TYPECK, not in the parser** (corrected during Task OP, Fork 4):
   the machine-primitive floor (`Int`/`Float`/`Bool` + op → direct instruction) vs.
   real-method split needs `left.ty`, which the parser lacks. So `ExprKind::BinaryOp`
   /`UnaryOp`/`Index` stay parser nodes; the desugar lives in `mir/lower/expr/binops.rs`
   + `typeck/infer/ops.rs` (shrink to machine-primitive instruction selection;
   non-primitive head → `def OP` method call; delete the migrated operator-synthesis
   arms). The parser learns only operator-symbol method NAMES (`def +`, `a.+(b)`).
   Comparison/equality (`==`/`<`/…) + `Comparable`/`<=>` are a SEPARATE later
   increment — left on the existing `binops.rs` paths (PartialEq derive, Vec/Map/Set
   `==`, Ord-derive) to avoid double-defining.
3. **Shared methods via mixins with default bodies** (Ruby modules). The mixins
   already exist as markers in `library/std/core/src/lib.rx` — fill them:
   - **`Enumerable[T]` over required `each`**: `map`/`select`/`reject`/`reduce`/
     `find`/`count`/`first`/`include?`/`to_a`/`all?`/`any?`/`partition`/`min`/
     `max`/`sum`/… defined ONCE. `Array`/`Set`/`Hash`/`Range` `include
     Enumerable[T]` + `def each`.
   - **`Comparable` over required `<=>`**: `<`/`<=`/`>`/`>=`/`clamp`/`between?`.
     Derives the comparison operators. `Int`/`Float`/`String`/`Duration`
     `def <=>` + `include Comparable`.
   - `Clone`/`Default`/`Hash`/`Displayable(to_s)` via default bodies or derive.
4. **De-primitivize the type heads**: `Ty::String`/`Ty::Array`/`Ty::Set`/`Ty::Map`
   dissolve into primitives + `.rx` classes; literals lower to primitives.
5. **Generic small-method inliner** at codegen for perf (inline trivial leaf
   methods like `Int.+`, combinator bodies) — generic, hardcodes no name.
   Required so operator-as-method and combinator-as-`.rx` don't regress hot
   paths >5×. Measure each.
6. **No `Co-Authored-By`. No `git push`. Never `git reset --hard`** (fix forward).
   Frequent small gated commits (API is flaky — every green commit is a
   recovery checkpoint). Never leave a broken tree.

## Gate (every commit)
`cargo build -p ruxen_repl` → `release_e2e_smoke -- --ignored` `[fast-path]` +
all fixtures 0-fail → golden re-record + `git diff` (only intended lines) →
`cargo test -p ruxen_core --lib` (≥561/0) → `cargo fmt --all`. NOT `--features
llvm` (no llvm-config here; changes are backend-agnostic). Never stage
`src/ruxenc/target/` (gitignored).

**Testing convention (NON-NEGOTIABLE):** every compile-and-run Ruxen program
is a release-e2e fixture (`tests/release-e2e/cases/<name>.rx` +
`expected/<name>.out`). NEVER embed a compile-and-run program as an inline
`r#"..."#` raw string in a Rust test, and never leave scratch/debug files
behind.

## Phases & status (update as you go)
- [x] ABI derivation; class-typed resolvers → `.rx`; `*Iter` teardown.
- [x] String/Array/Set/Hash/Int delegation via `builtin_bridge`.
- [x] Feature A: width-correct FFI receiver (Float→F64).
- [~] Feature C: combinators → `.rx`. Array `map/select/reject/all?/any?/
      partition/each_with_index/find/index/sort_by/reduce/zip` done.
      `mixin Enumerable[T]` now CONSOLIDATES `map/select/reject/all?/any?/
      partition/each_with_index/find/index/reduce` over a single required
      `each`; `Array`, `Set`, and `Hash` all `include Enumerable[T]` (Hash
      over `(K, V)` pairs) — no per-collection combinator bodies. Set/Hash
      supply only `each` (over the FFI-on-self `self.to_a`). Mixin generic
      params now scope into method sigs (`resolve_trait`); MIR emits the
      included defaults as opaque `<Class>_<m>` bodies and registers them in
      `lib_body_methods` (`collect_lib_body_methods` walks includes).
      Array-specific `sort_by`/`zip`/`to_h`/`sum`/`select!`/`each` stay on the
      class (positional / arg-typed / FFI). Option/Result `map`/`map_err`/
      `unwrap_or_else` migrated to `.rx` bodies (match-arm + closure call).
      Three compiler fixes for Set/Hash routing (`50825e6`): closure_inline
      fall-through for the `Ty::Class{Set|Hash|Map}` opaque-body self;
      `mixin_element_subst` (records each include's trait args so `Fn(T)`
      seeds to `(K, V)` for Hash); cell-promotion retypes the captured local
      to pointer width (fixed `load.i8` verifier error when a real capturing
      closure is passed to `Set_each`/`Hash_each`). Pins 612/613/614.
      **RESIDUAL:** `collections.rs` Option/Result arms are now `try_op`
      (`?`-intrinsic), `map` (fresh-var return), `ok_or` (arg-derived err);
      Array `each`/`to_h`/`sum`/`select!`/`get_mut`/`get_var`. `ok_or` left
      as documented residual (err type comes from the ARG, not a static
      return — same shape as `Array.zip`; no clean static `.rx` return).
- [x] Feature D: derive Clone/Debug/Default (retire `resolver.rs` structural).
      `clone`/`to_s`/`default` now resolve via the DERIVE MECHANISM in
      `resolver.rs::structural_fallback_resolvers`, gated on
      `ty_has_derive_trait(ty, "Clone"|"Debug"|"Default")` (`resolve/symbols.rs`)
      in lockstep with the MIR synthesis (`mir/lower/derive.rs`
      `synthesize_*_clone`/`_to_debug`/`_default`, gated identically in
      `mir/lower/mod.rs::lower_item`). `include Clone`/`Debug`/`Default` →
      `impl_blocks` → `collected_derives` merges into `derive_traits`; the
      structural §3.6 implicit-include also auto-derives field-supported
      types. The ONLY surviving structural arm is `.new` (all-fields
      constructor — genuine floor, not a derive). Pins: `208_implicit_default`,
      `216_derive_default_include` (+ existing `2xx_implicit_*`). Commits
      `019a60a`(clone)/`8c78170`(to_s)/`5a271d1`(default).
      `Enum.weight` ASSESSMENT: NOT a derive, NOT a floor — a dead
      golden-only arm with no real user that shadowed
      `lookup_class_method_return` (a `def weight -> Float` would mis-type as
      `Int`). REMOVED (`a6f73ef`); `numeric::resolvers()` now empty. Pin
      `217_enum_declared_method.rx`. (Uncovered a pre-existing, unrelated
      cranelift width bug in `Float`-returning enum-method codegen — out of
      Feature D scope, noted for a future codegen phase.)
- [x] Feature E: String reconcile (`remove`/`push`/… C-vs-surface). MIGRATED
      to `string.rx` via the bridge: `&str.to_lower`/`to_upper` (the C symbols
      `malloc` an owned buffer, so `-> String` is the CORRECT surface — the old
      `-> str` arm wrongly claimed a borrowed slice) and `&str.parse_uint`
      (newly declared `def parse_uint -> Result[USize, Error]` on `class
      String`, sharing the `&str` C symbol). Their `strings.rs` `Ty::Str` arms
      were deleted. GENUINE IRREDUCIBLE residuals kept in `strings.rs` (surface
      type ≠ derivable ABI width — no single `.rx` decl expresses both):
      `remove` (Char/I32 surface vs C 16-byte struct-pointer/I64 holding the
      removed codepoint + rewritten buffer) and the mutation methods
      `push`/`push_str`/`insert`/`insert_str` (surface `Unit` — the MIR
      special-case does the `&mut String` deref/store dance and returns no
      value, pin `48_borrow_var` — vs C `char*` the dance must capture, so
      `.rx` declares `-> String`). Pins: `630_string_insert_char`,
      `631_string_push_str`, `632_str_to_lower_upper`, `633_str_parse_uint`
      (+ existing `311`/`313`/`315`/`45`/`113`). `strings.rs` residual count:
      5 `String` arms (`remove`+4 mutation), 0 `Str` arms.
- [~] Feature B: trait-bound enforcement. DONE: generalized
      `check_concurrency_bounds` → `check_declared_bounds` +
      `check_generic_param_bounds` (harvested-generic seam) +
      `check_constructor_generic_bounds` (construction seam), all dispatching
      through `MixinResolver::check_satisfaction`. New general code **E1015**
      ("type does not satisfy declared mixin bound") for non-Send/Sync bounds;
      Send/Sync keep E1011/E1012. Preserved-code bridge (`ops.rs::
      bound_diagnostic_code`) maps owner+bound → code. **Moved to `.rx`
      bounds:** `class Mutex[T: Send]` (→ E1101) and `class SharedSync[T: Send]`
      (→ E1102) — the `concurrency.rs` E1101/E1102 arms were DELETED; enforced
      at the construction seam reading the `.rx` bound. Zero-regression rule
      (only bounded params fire; abstract/unresolved skipped) holds — full
      suite stayed 561/0 + e2e 333/0 through the inert landing. Red/green pin
      `tests/trait_bound_enforcement.rs` (`needs[T: Greet]` → E1015 / clean).
      GENUINE FLOOR: `Thread.spawn`'s **E1100** capture-Send (closure captures,
      not a param/generic bound). **DEFERRED(de-primitivize), NOT floor:**
      BufReader/BufWriter **E0714** — post-#06.95 the `.rx` is a module+mixin
      closed-set with no generic `BufReader[R]` param to bind `[R: Read]` to;
      migrating it would revert that reshape (regression). De-prim phase must
      re-examine whether per-variant constructors already enforce the inner
      type. **`sum`/E0700 fork — CLOSED (Task OP Step 2):** the
      receiver-element bound seam now exists. A `where T: Bound` on a class
      method whose `T` is the receiver class's own generic is threaded into the
      `FnSignature.generic_params` as a synthetic bounded param —
      `resolve/funcs.rs` for regular `def`s, `resolve/ffi_registration.rs::
      ffi_receiver_element_bounds` for FFI `lib`-block decls (which now parse an
      optional `where`, `parser/ffi.rs`). `collect.rs::bridge_builtin_method`
      binds `{T → element}` (`receiver_generic_bindings`) and runs
      `check_generic_param_bounds` → E0700. `class Array[T]`'s
      `def sum -> Int where T: Add` declares it; `mixin Add` (core) is a marker
      satisfied only nominally (`check_satisfaction` zero-method-mixin rule);
      `Int`/`Float`/`USize` `include Add`. `collections.rs` `sum` arm +
      `is_iter_sum_compatible` DELETED.
- [~] Operator wave (Task OP). **Step 1 DONE:** operator-symbol method-NAME
      parsing — `def +`/`-`/`*`/`/`/`%`/`&`/`|`/`^`/`<<`/`>>`/`[]`/`[]=`/`-@`/
      `+@`/`!` + the call form `a.+(b)`/`a.[](i)`/etc. (inert; `ExprKind` nodes
      unchanged). `**` omitted (no operator exists). Pin
      `631_operator_overload_explicit`. **Step 2 DONE:** receiver-element bound
      seam + `sum`/E0700 closure (above). **Remaining:** desugar (post-typeck)
      arithmetic/bitwise/index/unary → `def OP`, machine-primitive floor stays
      direct, delete migrated binops/ops arms, Duration `.rx def +`/`-`; then the
      SEPARATE comparison/equality + `Comparable` increment (deferred, `<=>` token
      vs reuse-`cmp` TBD).
- [ ] Literals→primitives + de-primitivize `Ty::String`/`Ty::Array`.
- [ ] Generic small-method inliner (perf).

## Known gaps / findings to respect
- Method-level closure-return generic inference: `Ty::Fn` arm in
  `bind_type_params_from_args` + bridge via `harvest_and_subst_generics` (done,
  `5a69cf2`). Verify it composes through mixin default methods (the Enumerable
  spike).
- `Array.get` surfaces a raw element not `Option[&T]` (Option-construct gap);
  index with `xs[i]` in `.rx` bodies.
- `Map` displays as `Hash[K,V]`; method-home key is `"Hash"`.
- [FIXED] Closure-param seeding for mixin-default combinators: a `kv.1`
  tuple-field access on a closure param of a mixin combinator (`reduce`/`map`/…
  from `include Enumerable[T]`) lowered to an unresolved `?T::1` method call
  because `MixinResolver::lookup_method_by_name` only consulted own
  `type_methods`, not trait/mixin default signatures — so the closure param
  stayed `Infer`. Fixed by falling through to `trait_method_sigs` for any
  implemented trait. Pin: `tests/release-e2e/cases/604_closure_tuple_param_field.rx`.
- [FIXED] `h.each { }` on a `Ty::Map`/`Ty::Set` receiver was mis-inlined by
  `try_inline_closure_method`'s `is_collection_method` branch — it treated the
  builtin collection as a user-defined Vec-wrapping class and dereferenced a
  non-existent backing Vec at field 0, so the closure was never called over the
  real entries (`Hash#each` "yields nothing"). Now builtin `Map`/`Set`
  receivers fall through to a real call to the migrated opaque `.rx` body
  (`Hash_each`), which materializes entries via the FFI-on-self `self.to_a` and
  forwards each tuple. Pin: `tests/release-e2e/cases/605_generic_self_ffi_method.rx`.
  `class Hash[K, V]` now carries a `def each` `.rx` body over `self.to_a`.
- [FIXED] Closure call inside a match arm of an opaque generic body
  (`Option#map`/`Result#map`/`Result#map_err` migrated to `.rx` bodies). Three
  faults, all now fixed: (1) `match self` in the opaque body sees `self` typed
  `Ty::Class { name: "Option"|"Result" }`, not structural `Ty::Option`/
  `Ty::Result`, so payload-field typing fell back to the variant's declared
  TypeParam → wrong field type → segfault. Fixed with
  `normalize_option_result_class` in `mir/lower/match_arms.rs`. (2) The call
  site mis-inlined `.map` on an Option/Result via the `is_collection_method`
  field-0 vec path (same class as the Map/Set bug) → fall through to the real
  `Option_map`/`Result_map` body. (3) typeck: `Result#map` dropped the err type
  (returned `Result[U, Error]` not `Result[U, E]`) and `map_err` had ok/err
  swapped; the `map_err` closure-return harvest was missing in
  `infer_combinator_block`. Pins: `99_map_option` / `115_option_map_chain` /
  `118_option_map_unwrap_or` (now over `.rx` Option.map) +
  `tests/release-e2e/cases/606_result_map_chain.rx`.
