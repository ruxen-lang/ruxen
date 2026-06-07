# Generic Compiler Migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL — use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans`. Steps use checkbox (`- [ ]`) syntax. **Architecture source-of-truth:** `docs/specs/system/generic-compiler.spec.md` — read it first.

**Goal:** Make the Ruxen compiler fully generic — zero hardcoded stdlib type/method names in Rust. The stdlib defines every type, method, and operator in `.rx` over the compiler's generic mechanisms; adding a stdlib class/method requires zero compiler edits.

**Architecture:** Compiler keeps only generic mechanisms (classes, generics, mixins/traits, FFI+repr, derive, closures) + machine primitives (i64/f64/bool/ptr/slices) + syntactic protocols (literal→primitive, operator→method-by-symbol). The stdlib lives in `.rx`. Method resolution goes through `builtin_bridge` + `declared_method_resolvers`; shared methods live in `mixin Enumerable[T]`/`Comparable`; operators are overridable methods; `Ty::String`/`Array` dissolve into primitives + `.rx` classes. No `#[lang]`/registry — generic features only.

**Tech stack:** Rust compiler (`compiler/ruxen_core`), Cranelift backend, `.rx` stdlib (`library/std/*`), release-e2e fixture harness.

---

## Operating rules (READ BEFORE ANY TASK)

- **Gate (every commit):** `cargo build -p ruxen_repl` (refreshes the fast-path `libruxenrt.a`) → `cargo test -p ruxen_core --test release_e2e_smoke -- --ignored --nocapture` shows `[fast-path] linking via …` + **all fixtures 0-fail** → golden re-record (`RECORD_GOLDEN=1 cargo test -p ruxen_core --lib golden_parity`) + `git diff` the snapshot to confirm **only intended lines** changed → `cargo test -p ruxen_core --lib` (**≥561/0**) → `cargo fmt --all`.
- **Do NOT gate on `--features llvm`** — `llvm-config` is absent in this env. All changes here are backend-agnostic (typeck/resolve/`.rx`/cranelift-independent).
- **Commits:** descriptive message, **NO `Co-Authored-By` trailer**, **NO `git push`**, never `git reset --hard` (fix forward), branch `stdlib-rust-cleanup`. **Frequent small gated commits** — the API has been dropping mid-run; every green commit is a recovery checkpoint.
- **One feature agent on the branch at a time.** Two concurrently-committing agents corrupt each other (learned the hard way). Read-only/doc fan-outs are fine alongside; committing agents are not.
- **Never stage `src/ruxenc/target/`** (gitignored build artifacts).
- **Pins are `.rx` release-e2e fixtures** (`tests/release-e2e/cases/<name>.rx` + `expected/<name>.out`). **NEVER** embed a compile-and-run program as an inline Rust `r#"..."#` string. Remove all scratch/debug files before committing.
- **After each feature, update the relevant `CLAUDE.md`** (per-module context, local/gitignored) so future agents don't re-deep-dive. Commit the spec-doc phase checklist update with the feature.
- **The LSP is unreliable during this refactor** — the binary lags the source. Trust `cargo` + the e2e gate, not the IDE.
- **Known gaps (spec doc §Known gaps):** index with `xs[i]` not `.get` in `.rx`; closures invoked `f.(x)` (= Ruby `.call`); `var`/`let`; `Map` method-home key is `"Hash"`; Hash element is the `(K,V)` tuple.

---

## Status snapshot (as of 2026-06-07, HEAD `92294d0`)

**Done & committed (green):**
| Phase | SHAs |
|---|---|
| ABI derivation from `.rx` | `076dd7c` |
| Class-typed resolvers → `.rx` | `f4b76be` |
| `*Iter` machinery teardown | `64dafb2` |
| String/Array/Set/Hash/Int delegation (`builtin_bridge`) | `c4bc2f7`,`f88d96c`,`f2a07d8`,`993858a` |
| Feature A — width-correct FFI receiver (Float→F64) | `676d79f` |
| 3 codegen bugs (closure-in-match-arm / FFI-on-self / closure-param seeding) | `446ef33`,`617cfb5`,`48c51aa` |
| Feature C — Array/Set/Hash `mixin Enumerable[T]`; Option/Result `map`/`map_err`/`unwrap_or_else` | …,`50825e6`,`79866af` |
| **Feature D — `Clone`/`Debug`/`Default` derive** (retired structural arms; `Enum.weight` dead-code removed; `numeric.rs` empty) | `019a60a`,`8c78170`,`5a271d1`,`a6f73ef`,`344a427` |
| **Feature E — String reconcile** (`&str.to_lower`/`to_upper`/`parse_uint`→`.rx`; `remove`+mutation methods = irreducible floor) | `218b2ce` |
| **Feature B — trait-bound enforcement** (general `check_declared_bounds`; `Mutex`/`Arc`/`SharedSync[T: Send]`→E1101/E1102; new E1015; `Thread.spawn` E1100=floor; BufReader E0714=deferred) | `bd9007b`,`dfbcf3c`,`7828685` |
| **Merge `origin/master`** (struct/enum inline-method codegen + `FloatToBits`) — clears the deferred Float-enum bug, pinned by `218` | `03f28f2`,`92294d0` |
| CI fast-path; build-artifact untrack; plan/spec docs | `8714d97`,`a626769`,`7145f5d`,`881dcb5` |

**~64% by effort.** Done through Feature B + master merge. Lib 561/0, e2e **338/0**.

**Residual / debt (tracked):**
- `collections.rs` ≈ 7 arms (Array `each`/`to_h`/`sum`/`select!`; Option/Result `try_op`/`map`/`map_err`/`ok_or`) — closure/operator/diagnostic floor + the `sum`/E0700 fork.
- **`sum`/E0700 fork (from Feature B):** needs a *receiver-element method-call bound seam* (sum's `Add` bound is on the Array class generic `T`, argless — can't flow through the harvest seam; the `where` merge in `resolve/funcs.rs` only targets a method's own generics). **Same seam the operator wave needs** (`a + b`→`a.+(b)`) → folded into Task OP.
- **BufReader/BufWriter E0714** — DEFERRED to Task DEPRIM (closed-set `module BufReader` has no generic param to bind; re-examine whether per-variant constructor param-types already enforce the inner type). NOT permanent floor.
- **Genuine permanent floor:** `Thread.spawn` E1100 capture-Send; `String.remove`/`push`/`push_str`/`insert`/`insert_str` ABI-divergence; the `.new` aggregate constructor.
- **Workspace-health (NEW, must clear before merge to master):** `cargo test --workspace` has 6 PRE-EXISTING failing targets unrelated to the migration's gated work — `ruxen_ide --lib`, `ruxen_lsp --test server_integration`, `ruxen_repl --lib`, `ruxen_core --test std_use_resolution`, `ruxen_cli --test installed_pkg_manager`, `ruxenc --test installed_binary`. These drifted because the gate is narrow (rule 42). Triage + fix as **Task WS** before this branch merges to master.

Remaining tasks below.

---

## Task D: derive `Clone` / `Debug` / `Default`  *(IN PROGRESS — agent `ab6e1d4a`)*

Retire the user-type structural auto-derives (`to_s`/`clone`/`new`/`default`) from `typeck/method_resolvers/resolver.rs::structural_fallback_resolvers` into a real derive mechanism mirroring the Send/Sync auto-derive (`typeck/mixins.rs::register_derived_impls`).

**Files:** `typeck/mixins.rs` (derive machinery), `typeck/method_resolvers/resolver.rs` (delete structural arms), `library/std/core/src/lib.rx` (wire `Clone`/`Debug`/`Default` mixins).

- [ ] **Step 1 — failing pins:** add `tests/release-e2e/cases/620_derive_clone.rx` (a `class`/`struct` with `include Clone`, calls `.clone()`, prints field), `621_derive_debug.rx` (`include Debug`/`Displayable`, `to_s`), `622_derive_default.rx` (`include Default`, `Default.new`/zero-construct) + matching `expected/*.out`. Run e2e → expect FAIL (derives not wired).
- [ ] **Step 2 — wire derives:** in `register_derived_impls`, on `include Clone` synthesize field-wise `clone`; `include Debug`/`Displayable` → structural `to_s`; `include Default` → `default`/`new`. Same machinery as Send/Sync, but generating method bodies (not just markers).
- [ ] **Step 3 — delete structural arms:** remove the `to_s`/`clone`/`new`/`default` arms from `resolver::structural_fallback_resolvers`.
- [ ] **Step 4 — assess `Enum.weight`** (`numeric.rs`, 1 arm, compiler accessor, no runtime symbol): fits the derive model, or genuine floor? Document the decision; do not force.
- [ ] **Step 5 — gate** (full gate above; golden diff = only the intended structural lines removed).
- [ ] **Step 6 — update context:** `resolve`/`typeck`/`method_resolvers` + `library/std/core` `CLAUDE.md`; spec doc phase checklist.
- [ ] **Step 7 — commit** (`feat(derive): Clone/Debug/Default auto-derive; retire structural resolver arms`).

---

## Task E: String reconcile (the ABI-divergent residuals)

Migrate the last `strings.rs` arms whose `.rx`/C types diverge: `String.remove`, `push`, `push_str`, `insert`, `insert_str`, and `&str` `to_lower`/`to_upper`/`parse_uint`.

**Files:** `library/std/string/runtime/string.c` (ground-truth C signatures), `library/std/string/src/string.rx`, `typeck/method_resolvers/strings.rs`.

- [ ] **Step 1 — read C ground truth** for each symbol in `string.c`. Record actual return/param widths (e.g. `remove` → C `void*`/I64 vs surface `Char`/I32; `push` → C `char*` vs surface `Unit`).
- [ ] **Step 2 — per arm, decide the true surface type** and apply the STANDING RULE (spec doc): if the derived ABI width matches C → migrate the `.rx` decl + delete the arm; if it would mismatch → reconcile (fix the `.rx` type **and**, only if strictly necessary, the C return) so they agree, then migrate; if genuinely irreducible → keep as documented residual.
- [ ] **Step 3 — pins:** `.rx` fixtures exercising each migrated method (`s.remove(i)`, `s.push(?x)`, `s.insert(...)`, `&str.to_lower`).
- [ ] **Step 4 — gate + golden diff + update `library/std/string` + `strings.rs` `CLAUDE.md` + commit** per arm (or small groups).

---

## Task B: trait-bound enforcement  *(DESIGN-FIRST — high blast radius)*

**Goal:** general `where T: Bound` / `[T: Bound]` enforcement on call sites, so the construction-site checks move to `.rx` bounds and the resolver arms delete. Affects **every generic call site** → design + full-suite gate before deleting anything.

**Seams (pinned):** `typeck/infer/ops.rs::check_concurrency_bounds` (already enforces Send/Sync on a bounded-`TypeParam` arg → E1011/E1012 — the generalization target); `typeck/mixins.rs::check_satisfaction` (special-cases Send/Sync); the hardcoded checks to retire — `concurrency.rs` (Mutex.new→E1101, Arc/SharedSync→E1102), `io.rs` (BufReader/Writer→E0714), `collections.rs` (`sum`→E0700).

- [ ] **Step 1 — DESIGN, stop for review.** Produce: (a) generalize `check_concurrency_bounds`→`check_declared_bounds(param_ty, arg_ty)` over any `MixinRef` bound via `check_satisfaction`; (b) ALSO enforce bounds on harvested function/class generic params after `{T→concrete}` binding; (c) only-bounded-params-checked rule (unbounded generic calls untouched → zero regression); (d) diagnostic-code plan (preserve E1011/12, E1100/01/02, E0700, E0714, or assign new codes deliberately); (e) which checks become `.rx` bounds: `class Mutex[T: Send]`, `class BufReader[R: Read]`, `def sum -> T where T: Add`; (f) the genuine floor — `Thread.spawn`'s E1100 capture-Send (closure captures, not a param bound) — confirm it stays compiler-side. **Do not implement until reviewed.**
- [ ] **Step 2 — implement the general enforcement pass** (red/green pin: `fn needs[T: Display](x:T)` called with non-Display → clean E-coded error; with Display → clean). Gate on **full `cargo test -p ruxen_core`** + new bound fixtures, not just e2e.
- [ ] **Step 3 — declare the bounds in `.rx`** (`Mutex[T: Send]`, `BufReader[R: Read]`, `sum where T: Add`) and **delete** the `concurrency.rs`/`io.rs`/`sum` arms. Gate per deletion.
- [ ] **Step 4 — update `CLAUDE.md` (concurrency/io/method_resolvers) + spec checklist + commit.**

---

## Task OP: operator + Ruby block-syntax wave

**Goal:** operators become overridable `.rx` methods; closures use Ruby block convention. Delete operator special-casing.

**Files:** `parser/expr/operators.rs`, `parser/methods.rs` (parse `def +`/`[]`/`[]=`/`<=>`/unary + `&block` params), `parser/expr/calls.rs` (desugar), `mir/lower/expr/binops.rs` + `typeck/infer/ops.rs` (DELETE operator arms), `lexer` (wire `yield` — currently a placeholder), `library/std/core/src/lib.rx` (`mixin Comparable` over `<=>`), scalar/Duration/etc. `.rx` (`def +`/`-`/`[]`).

- [ ] **Step 1 — DESIGN, stop for review.** Decide: operator-symbol-as-method-name (`a + b` → `a.+(b)`), desugar in parser→HIR so downstream is operator-free; `Comparable` derives `<`/`<=`/`>`/`>=` from `<=>` in `.rx`; unary naming (`-@`/`+@`/`!`); `[]`/`[]=` index/index-assign desugar; `&block` typed capture (`&block: Fn[T->U]`) + `yield`/`block.call`/`.()` invocation, **keeping the typed block signature** (untyped `yield` would break generic `U` inference); precedence stays compiler-syntax. Confirm `f.(x)` is already Ruby (`.call`) — no change needed there.
- [ ] **Step 2 — parser support** (pins: `.rx` fixtures with `def +(o)`, `def [](i)`, `def []=(i,v)`, `def <=>(o)`, `def -@`, a `&block` method). Gate.
- [ ] **Step 3 — desugar `a OP b` → `a.OP(b)` and `a[i]`→`a.[](i)`** in parser→HIR; delete `binops.rs` + `infer/ops.rs` operator arms; add `mixin Comparable`; give scalars/Duration their operator `.rx` methods. Gate (Duration arithmetic + comparison fixtures must stay green).
- [ ] **Step 4 — convert combinator decls** `(f: any Fn[...])` → `(&block: Fn[T->U])` + `yield`/`block.call`; gate (combinator fixtures green).
- [ ] **Step 5 — update `CLAUDE.md` (parser/mir/typeck) + spec + commit** per increment.

> **PERF note:** operators-as-methods make primitive arithmetic a method call. This is acceptable *only* with Task INLINE landing (generic inliner). Measure `Int` arithmetic before/after; if catastrophic without the inliner, sequence INLINE before/with this.

---

## Task DEPRIM: de-primitivize `Ty::String`/`Array`/`Set`/`Map` + literals  *(DESIGN-FIRST — the long pole, effort 15)*

**Goal:** dissolve the primitive type heads so the compiler holds no `"String"`/`"Array"`/`"Set"`/`"Hash"` name and no `Ty::String`/`Array` variants — only machine primitives (byte-slice, array-slice, i64, …) with the stdlib `.rx` classes layered over them; literals lower to primitives. This kills the remaining type-classification name-matches (e.g. `matches!(base, "Set"|"Hash"|"Map")` in `closure_inline`/`util.rs`).

**Seams (pinned, ~289 name-references across `typeck` 122 / `mir` 79 / `resolve` 54 / `codegen`+`parser`+`hir` 46):** `hir/types.rs` (`Ty::String`/`Str`/`Array`/`Set`/`Map` variants); `typeck/infer/collect.rs:603` (`StringLiteral => Ty::String`); `typeck/method_resolvers/method_home_key` + `substitute_generics_in_return`; `mir/lower/util.rs` (collection-type classification); `mir/lower/closure_inline/mod.rs` (`is_builtin_non_vec_collection`); `mir/lower/expr/method_call.rs` (`Array`→`Vec` runtime mapping, static-ctor dispatch); literal lowering in codegen.

- [ ] **Step 1 — DESIGN, stop for review.** This is a foundational rewrite — produce a design covering: how literals (`"…"`, `[…]`, `42`, `true`, `?c`) lower to primitives without naming stdlib classes; whether `Ty::String`/`Array` become a single primitive `Ty` (byte-slice/array-slice) with the `.rx` class as the method-home, or are removed entirely; how `class Int`/`Float`/etc. declare their machine `repr` generically (FFI/`repr` attribute) so codegen knows widths without hardcoding names; the migration order (probably: literals→primitive first, then dissolve each head, then delete the name-matches); the blast radius + rollback points. **Expect this to surface its own cascade of bugs.** Do NOT implement until reviewed — likely split into sub-plans.
- [ ] **Step 2+ — staged implementation** per the approved design, each stage a gated commit, each removing a slice of the ~289 name-references. (Steps enumerated in the sub-plan produced by Step 1.)
- [ ] **Final — acceptance grep:** `grep -rn -E '"(String|Str|Array|Vec|Set|HashSet|Map|HashMap|Hash)"' compiler/ruxen_core/src/` returns empty (modulo machine primitives + the operator-desugar protocol). `method_resolvers/mod.rs` registers only `declared_method_resolvers` + `builtin_bridge`.

---

## Task INLINE: generic small-method inliner (perf)

**Goal:** restore perf lost to (a) closure-call-per-element combinators and (b) operators-as-methods, via a **generic** codegen pass that inlines trivial leaf methods (e.g. `Int.+`, identity passthroughs) — hardcoding no name.

**Files:** `codegen/cranelift/*` (the inliner), `mir/lower/*` (candidate marking).

- [ ] **Step 1 — DESIGN, stop for review.** Define "inlinable leaf method" generically (small body, no recursion, resolved callee); where in MIR→Cranelift it runs; how it stays name-agnostic.
- [ ] **Step 2 — implement + bench.** Micro-bench `map`/`select` on ~1M elements and `Int` arithmetic loops before/after; report deltas (target: within ~2× of the pre-migration inlined path).
- [ ] **Step 3 — gate + update `codegen` `CLAUDE.md` + commit.**

---

## Task H: method-level generic harvesting through the bridge  *(highest-leverage residual retirement)*

**Goal:** the bridge resolves a `.rx` method's RETURN type when it uses a **method-level generic** bound from a closure-arg's return (`def map[U](f: any Fn[Fn(T)->U]) -> Option[U]`) or a direct arg (`ok_or(err: E) -> Result[T, E]`) — retiring the `collections.rs` Option/Result fresh-var arms and resolving those signatures from `.rx`. This is the single Rust change that retires the most remaining method-dispatch arms, and it **completes the "add a generic combinator with zero Rust" goal** (any future `flat_map`/`and_then`/`filter_map` then resolves from `.rx`).

**Why it's needed (not pure-`.rx`):** `substitute_generics_in_return` (`infer/collect.rs:854`) binds RECEIVER generics only; `harvest_and_subst_generics` (`:1253`) only partially handles method-level/closure generics. So `collections.rs` mints fresh type vars per-method: `(Option,"map")=>Option[fresh]`, `(Result,"map_err")=>Result[ok,fresh]`, `(Option,"ok_or")=>Result[inner,Error]`. The `.rx` declares the true signatures but the bridge can't bind the method generic from the args.

**The change (`infer/collect.rs` — `bridge_builtin_method` + `harvest_and_subst_generics`):**
- [ ] Step 1: after receiver-generic subst, for each UNBOUND method generic `P` on the `.rx` method, find `P` in the declared param types and structurally unify with the actual arg's inferred type: closure-return slot `Fn(T)->U` → `U` = closure arg's inferred return; direct-arg slot `err: E` → `E` = arg type; else mint a fresh var (later unified — same as `collections.rs` does now, but generic). Substitute full σ into the return.
- [ ] Step 2: ordering — harvest AFTER inferring the closure/arg exprs (closure-param seeding from Feature C seeds the closure params; its body yields the return). Pin: `xs.map{…}.map{…}` chains type correctly through `.rx`.
- [ ] Step 3: delete `collections.rs` arms — Option `map`/`ok_or`, Result `map`/`map_err` (keep `try_op`/`?` — operator protocol). Declare the real signatures in `option_result/lib.rx`.
- [ ] Step 4: gate — Option/Result fixtures (99/115/118/606) green; golden re-record (the `map`/`map_err`/`ok_or` rows move resolver→bridge, like `sum`); **full `cargo test -p ruxen_core`** (touches the generic-subst chain EVERY generic call uses — full-suite gate mandatory). Update `infer`/`method_resolvers` CLAUDE.md.

**Does NOT retire (still floor/deferred):** String ABI-divergence (`remove`/`push`/`insert` family), `Thread.spawn` capture-Send (E1100), `.new`/`each`/operators, `BufReader` E0714 (→ DEPRIM), concurrency high-level API (needs `.rx` methods written + construction-time harvesting for `JoinHandle[T].join`).

---

## Self-review

- **Spec coverage:** every phase in `generic-compiler.spec.md`'s checklist maps to a Task here (D, E, B, OP, DEPRIM, INLINE) ✅. The done phases are recorded in the Status snapshot with SHAs ✅.
- **Placeholders:** D/E are concrete; B/OP/DEPRIM/INLINE are explicitly **design-first** (Step 1 = produce design, stop for review) — this is intentional and honest for a compiler rearchitecture, not a hidden TODO. The design steps name the exact seams/files to investigate.
- **Type/name consistency:** gate commands, file paths, SHAs, and diagnostic codes are consistent with the spec doc and the committed history.
- **Floor acknowledged:** `Thread.spawn` capture-Send, the operator-desugar protocol, and machine primitives + literal/iteration syntax are the legitimate compiler-only residue — not stdlib-name hardcoding.
