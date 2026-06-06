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
   `a[i]` → `a.[](i)`, `a[i]=v` → `a.[]=(i,v)`. Desugar in parser→HIR so nothing
   downstream special-cases operators. Delete `mir/lower/expr/binops.rs` +
   `typeck/infer/ops.rs` operator arms.
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
      `each`; `class Array include Enumerable[T]` (bodies deleted from Array).
      Mixin generic params now scope into method sigs (`resolve_trait`); MIR
      emits the included defaults as opaque `Array_<m>` bodies and registers
      them in `lib_body_methods` (`collect_lib_body_methods` walks includes).
      Array-specific `sort_by`/`zip`/`to_h`/`sum`/`select!`/`each` stay on the
      class (positional / arg-typed / FFI). **NEXT: `include Enumerable[T]` in
      Set + Hash (provide their `each`); migrate Option/Result closures.**
- [ ] Feature D: derive Clone/Debug/Default (retire `resolver.rs` structural).
- [ ] Feature E: String reconcile (`remove`/`push`/… C-vs-surface).
- [ ] Feature B: trait-bound enforcement (move Send/E0714/sum-Add to `.rx`
      bounds; generalize `check_concurrency_bounds`). Genuine floor: Thread.spawn
      capture-Send.
- [ ] Operator wave: `def +`/`[]`/`<=>` parser support; desugar; delete
      binops/ops arms; `Comparable` mixin.
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
