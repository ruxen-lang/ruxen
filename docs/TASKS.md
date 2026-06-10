# Ruxen — Task Index

**Canonical task list for the Ruxen compiler/toolchain.** This is a roll-up
over the authoritative backlogs below — it does not replace them, it tells you
where each kind of work lives and what is open *right now*. Keep it current
(see `../CLAUDE.md` → "Task tracking & keeping context current").

## Where work is tracked

| Source | What it holds |
|--------|---------------|
| [`requirements/ROADMAP.md`](requirements/ROADMAP.md) | The full tier-1→5 feature backlog (38 spec docs) **and** the P0 pre-flight bug list. The authoritative long-horizon plan. |
| [`dev/gui-stack-v1-issues.md`](dev/gui-stack-v1-issues.md) | Compiler/toolchain bugs surfaced by the sibling apps (Q1–Q22), each with a repro + severity + fix status. The app↔language ledger. |
| [`prompts/v1/25_v1_release_checklist.md`](prompts/v1/25_v1_release_checklist.md) | The v1.0.0 release gate checklist. |
| [`../CHANGELOG.md`](../CHANGELOG.md) | What's **done** — every landed fix (Keep a Changelog, `[Unreleased]`). |
| `tests/release-e2e/cases/` | The pin test backing each landed fix. |

## Open now — GUI-stack ledger (`dev/gui-stack-v1-issues.md`)

33 of 37 fixed (Q23–Q26 surfaced 2026-06-08; Q16 fixed 2026-06-08 on
feat/drop-elaboration; Q29 audited 2026-06-09 — already sound, pinned; Q28, Q30,
Q31 fixed 2026-06-09; Q32, Q33 fixed 2026-06-10; Q17 fixed for generic free fns 2026-06-10; **Q34 fixed 2026-06-10 via the syntax-parity harness; Q37 (S1, root-caused: a yielding/`&block` method poisoned an unrelated same-named free fn via a bare-name yield map) fixed 2026-06-10 — quiver examples build again; Q35, Q36 NEW 2026-06-10** — Q36 from the quiver Ruby-block DSL migration: two-`&var`-arg `yield` miscompile, left filed-open). The
canvas `Int`→`Float32` event-coord revert (unblocked by Q28/Q31) has LANDED
(canvas 143 green, sub-pixel pinned, live windowed loop verified).

### Toolchain / tests

- [x] **Syntax-parity harness — one syntax across compiler/fmt/repl/lsp/ide
      (DONE 2026-06-10, `feat/drop-elaboration`).** ADR
      `docs/decisions/syntax-parity-harness.md`. Two axes: per-surface
      conformance over the cases + stdlib + canvas/quiver/rondo corpus (491
      files), and structural pins (a compile-time exhaustiveness guard over
      `TopLevelItem`/`MixinItem`/`ImplItem`; an intentional-divergence
      allowlist for the E0728/E0607 accepted-but-compile-rejected class). The
      fmt axis (reparse-identity + idempotence) caught **Q34** (dropped
      grouping parens) plus four more fmt-destructiveness bugs — zero-arg
      `MethodCall`→field-access, method-visibility-section drop, `async`
      modifier drop — all fixed. Pins: `tests/syntax_parity.rs`,
      `src/ruxen_ide/tests/syntax_parity_ide.rs`,
      `tests/q34_fmt_grouping_parens.rs`, and a new `parity` phase in
      `tests/release-e2e/run.sh` (in `PHASES=all`).
- [x] **Q37 · S1 — yield/`&block`-method name-collides with a same-named free
      fn (FIXED 2026-06-10, `feat/drop-elaboration`).** A yielding/block-taking
      METHOD registered its bare name in the name-keyed `yield_fns` map, so an
      unrelated same-named generic free fn inherited a phantom `__block` →
      `could not infer type for parameter __block`. This broke every quiver
      example binary once canvas gained its `frame` methods (colliding with
      quiver's `frame` free fn in the flat-merged build). Fixed by deciding the
      synthetic `__block` LOCALLY from each function's own body
      (`resolve/funcs.rs`); the buggy name map + populator deleted. Pin:
      release-e2e 920. Verified: quiver counter example builds again. Ledger
      §Q37.

### Language features

- [x] **Ruby-block semantics — explicit `&block` / `yield` / `block_defined?`
      (DONE 2026-06-10, `feat/drop-elaboration`).** ADR
      `docs/decisions/ruby-block-semantics.md`. Explicit optional
      `&block: Fn[(T…) -> R]` (canonical square-bracket spelling; paren form kept
      for back-compat; `fmt` preserves both), `yield`/`yield(args)` with the
      block's value typed `R`, `block_defined?` + `block_given?` alias, optional
      blocks with a clean runtime panic on blockless `yield` (E-free, exit 101),
      single `do…end`/`{ }` attachment rule for free fns and methods, new
      diagnostic **E1119** (`&block` not last). Block slot = 8-byte
      closure-pair-pointer + null sentinel, independent of Q2. Pins: release-e2e
      908–912, `tests/ruby_block_semantics.rs`, drop-leak soundness pin.
- [x] **Ruby `alias` keyword — `alias new_name old_name` (DONE 2026-06-10,
      `feat/drop-elaboration`).** ADR `docs/decisions/alias-keyword.md`. A pure
      resolver synonym (one body, two names; zero duplicated codegen), valid in
      class/struct/enum/mixin/extension bodies (method synonym) and at
      top-level/module scope (free-fn synonym). Contextual keyword (not
      reserved). Plain + `?`/`!` names; operator aliases staged (E1123). New
      diagnostics **E1120/E1121/E1122/E1123** (registered + `docs/errors/`).
      Accepted on every surface (compiler/fmt/repl/lsp/ide). stdlib sweep:
      `Array#to_a` → `alias to_a clone`. Pins: release-e2e 913–918,
      `tests/alias_keyword.rs`.
- [ ] **Alias follow-up · S4 — operator aliases (`alias << push`, `alias [] get`).**
      Staged from Tier 1 (E1123). Operator names route through the
      post-typeck operator-desugar path (`mir/lower/expr/binops.rs` +
      `typeck/infer/ops.rs`), distinct from ordinary method-name mangling; wiring
      the synonym map into that path is the remaining work. ADR D6.
- [ ] **Alias follow-up · S4 — aliasing a mixin DEFAULT-method target.** An alias
      whose target is a method the type gets ONLY from a mixin default body (not
      redefined in the type) is rejected with E1120 today. The default's signature
      lives in `typeck::trait_method_sigs`, not the type's `type_methods`, so the
      typeck-side synonym registration can't yet bind it. Aliasing a type's OWN
      method (incl. one satisfying a mixin requirement) works. ADR D7.
- [ ] **Block follow-up · S3 — closure/block captures are not freed at
      closure-drop.** A closure (`{ }` OR `do…end`) that captures a heap value
      leaks that capture: `allocs=3, frees=0` for both forms (verified — this is
      a PRE-EXISTING closure-capture-drop gap, NOT a block regression; the block
      surface reuses the same machinery). Drop elaboration over a closure's
      captures struct (free each owned capture, then the captures block and the
      pair) is unimplemented. No double-free, no segfault — purely a leak.
      Repro: `tests/fixtures/ruxen/block_capture_heap_no_leak.rx` (soundness
      pinned; leak-freedom is this follow-up).
- [x] **Block follow-up · S3 — paren-less blockless call to an optional-block
      METHOD doesn't fill the block slot (FIXED 2026-06-10,
      `feat/drop-elaboration`).** `w.frame` (no parens, no block) on a method
      declaring `&block` parsed as a `FieldAccess`; that no-arg method path
      (`mir/lower/expr/field_access.rs`) did not append the `nil` block default,
      so MIR emitted one too few args and CRASHED the arity verifier
      (`__closure_*: got 1, expected 2`). FREE functions had no gap (the
      blockless `render` in pin 909 works). Fixed via the "fill defaults at MIR"
      option: the no-arg method route now appends the resolved method's trailing
      default sentinels via `Lowerer::method_trailing_default_sentinels`
      (`mir/lower/mod.rs`) — the MIR mirror of typeck's
      `append_method_default_args` the parens path already runs — so `w.frame`
      and `w.frame()` lower identically (consistent with the regular-default
      fix `autocall_uses_real_default_not_null`). Pins: release-e2e
      `921_block_optional_method_parenless` (RUN+stdout; revert CRASHES, not
      just asserts), `ruby_block_semantics.rs`
      (`parenless_blockless_method_call_fills_block_slot`,
      `explicit_block_param_on_method` extended). ADR known-limitations updated.
- [ ] **Block follow-up · Tier-2 (staged, per ADR).** `&` block-forwarding
      (`g(&block)` / anonymous `&`), `next` as block-value, `&:symbol` to-proc
      sugar, numbered params / `it`. Rejected (see ADR): non-local `return` /
      Ruby `break`-exits-yielder, `redo`, lenient arity, `instance_eval`
      self-rebinding. Nested `yield` inside a closure body errors cleanly in
      Tier 1.
- [x] **Interpolating a closure prints silent pointer garbage → now E0729
      (FIXED 2026-06-10, `feat/drop-elaboration`).** A bare `do … end` is a
      closure literal, NEVER an expression block (parser `atoms.rs`: "do…end is
      always a closure"), so `let v = do … end; puts "#{v}"` bound the
      un-invoked closure and MIR interpolation's "unknown type → Int_fmt
      (pointer-as-int)" fallback printed a raw pointer — silent garbage, and the
      `docs/tutorial/05-control-flow.md` "Blocks as expressions" section taught
      exactly this broken form. Fixed: typeck (`infer/expr.rs` Interpolation arm)
      now rejects a `Fn`/`FnMut`/`FnOnce`-typed interpolated part with **E0729**
      (`docs/errors/E0729.md`); an invoked closure's result (`#{f.()}`) and all
      ordinary values are unaffected. Tutorial section rewritten (helper-fn /
      invoke-and-format alternatives; the match-arm `-> do … end` value form is
      real and kept). Pins: `ruby_block_semantics.rs::interpolating_a_closure_
      is_e0729` + `..._invoked_closure_result_is_ok`.
- [ ] **PARKED design question — Ruby-style expression blocks.** If a block that
      is itself a *value* (multi-statement expression producing its last value)
      is ever wanted, the unambiguous spelling is **`begin … end`**, NOT
      `do … end` — `do … end` now firmly means "a block attached to a call"
      (enforced by E0729 on the silent-pointer path). Today the value-producing
      multi-statement need is met by a helper function or an `if`/`match`
      expression. No work scheduled; recorded so the spelling decision is not
      re-litigated ad hoc.

- [x] **Q34 · S2 — `ruxen fmt` drops grouping parentheses (FIXED 2026-06-10,
      `feat/drop-elaboration`).** `(rel*span + track_w/2)/track_w` →
      `rel*span + track_w/2/track_w` broke quiver's slider math. Fixed by
      re-parenthesizing operands by precedence (`formatter/prec.rs`, mirroring
      `parser::expr::infix_binding_power`); the syntax-parity harness's fmt axis
      (reparse-identity + idempotence over the whole stdlib + sibling corpus)
      also caught four sibling fmt-destructiveness bugs fixed alongside. The
      "do NOT bulk-run `ruxen fmt`" caution is LIFTED — fmt is reparse-faithful
      over all 492 corpus files. Pins: `tests/q34_fmt_grouping_parens.rs`,
      `tests/syntax_parity.rs`, release-e2e 919, ADR
      `docs/decisions/syntax-parity-harness.md`.

- [ ] **Q35 · S3 — a STRUCT's `include <Mixin>` does not satisfy a generic's
      mixin bound (E1015).** Even a single struct implementor is rejected by a
      mixin-bounded generic; the identical class works (Q17's 655 fixture runs
      on the installed CLI). Orthogonal to Q17 — typeck's bound-satisfaction
      registry records `include` only for classes. Clean diagnostic, nothing in
      the GUI stack blocked (PaintSurface implementors are classes). Repro:
      `tmp/test-cache/q35-struct-include-bound-repro.rx`; details §Q35.
- [ ] **Q36 · S2 — `yield` with two `&var` reference args miscompiles (NEW
      2026-06-10).** `yield(&var app.ui, &var app.root)` (two `&var` refs in one
      yield) runs the block against an EMPTY target; single-`&var` yield and the
      `f.(&var a, &var b)` closure-call form both work. Found migrating quiver's
      DSL to `&block`/`yield`; quiver works around it (`App.build` stays a closure
      param). Sub-gap: a `&block` param's type doesn't infer through the yield
      seam (untyped block param ⇒ `?T`). Likely from `8a783f9` (block semantics).
      Repro: `quiver/tmp/test-cache/ruxen-two-var-yield.md`; details §Q36.
- [ ] **Q37 · S2 — generic `frame[S: Mixin]` in a library gets a bogus `__block`
      param when consumed by a binary (NEW 2026-06-10).** All three quiver
      example binaries fail `ruxen build` with `could not infer type for
      parameter __block in function frame` on quiver's yield-free generic
      `frame[S: PaintSurface]` (`quiver/src/run.rx`) — only in binary-consumes-
      library builds (the quiver lib + 157-test suite are green; reproduces at
      pristine quiver HEAD). Likely a `yield_scan` over-attribution across the
      flat-merged lib+binary source after `8a783f9`. Details §Q37.
- [x] **Q32 · S3 — Q16 flat-merge of an FFI dependency broke `ruxen test` at
      link (FIXED 2026-06-10).** A consumer's test EXECUTABLE flat-merged the
      FFI dep's `src/**.rx` (incl. `lib "C"`-calling bodies) but neither
      compiled its `runtime/**.c` nor propagated its `[system_libs]` → every
      `ruxen_*` symbol undefined at link. Fix (option (b)): the test runner now
      gathers each flat-merged dep's `runtime/**.c`
      (`codegen::find_runtime_sources_in_dir`) and forwards each dep's
      `[system_libs]` (`codegen::parse_system_libs`) as `--link-arg=-l<lib>`
      (new repeatable `ruxenc` flag), mirroring exactly what `compile_project`
      gathers for a directly-declared dep; `compile_project` also gained the
      dep `[system_libs]` propagation it was missing. `src/ruxenc/test_runner.rs`,
      `src/ruxenc/compile.rs`, `src/ruxen_cli/build.rs`. Pins (staged install,
      RUN + assert): `src/ruxen_cli/tests/ffi_dep_link.rs` (FFI-dep test links +
      passes; binary declaring the dep directly has no dup-symbol; non-FFI dep
      still links). Details in `dev/gui-stack-v1-issues.md` §Q32.
- [x] **Q33 · S2 — `Float32 == <negative Int literal>` evaluated false (FIXED
      2026-06-10).** The `Compare` MIR instruction is width-blind; codegen
      coerced the rhs to the lhs's SSA type with the signedness-BLIND
      `coerce_value` (`fcvt_from_uint`), so a signed `Int(-1)` (i64 `0xFFFF…`)
      became `1.8e19` and `f == -1` was false. The ordering ops (`>=`/`<`) and
      the literal-on-the-left shape broke too; positive literals and f32==f32
      were accidentally fine. Fix: re-materialize a mismatched numeric operand
      pair to a common float width via a target-typed `Assign` before the
      `Compare` (`coerce_compare_operands` in `mir/lower/expr/binops.rs`),
      invoking codegen's Q5 signedness-aware int→float path. Backend-agnostic
      (shared MIR); mirrors Q28's `coerce_to_field_ty`. Pins (RUN + assert
      stdout): `tests/release-e2e/cases/653_f32_negative_int_literal_compare`,
      `654_enum_f32_payload_negative_compare` +
      `compiler/ruxen_core/tests/q33_negative_literal_float_compare.rs`.

- [x] **Q28 · S1 — `Float32` field/payload store-via-local → 0 / crash (FIXED 2026-06-09).**
      The struct/enum/tuple constructor lowering stored each field value
      width-blind (at the value's SSA width) into a fixed 8-byte slot; an f64
      value into an f32 field stored 8 bytes, and the f32 `GetField` read 4 →
      0. Fix: coerce each constructor arg to the FIELD's declared width via a
      target-typed `Assign` (`coerce_to_field_ty` → shared `coerce_value`
      fdemote/fpromote path) BEFORE the `SetField`, in
      `mir/lower/expr/constructors.rs` (Construct/EnumVariant/Tuple) and the
      struct auto-ctor in `method_call.rs`. Backend-agnostic (shared MIR). All
      four shapes now print 204.75; the e2e pins RUN + assert stdout. Pins:
      `tests/release-e2e/cases/650_f32_field_store_via_local`,
      `651_enum_f32_payload_via_local` (+ 647/648) and
      `compiler/ruxen_core/tests/q28_enum_float_payload.rs`.
- [x] **Q30 · S4 — `ruxen fmt` rewrote builder-closure call shapes (FIXED 2026-06-09).**
      `fmt` dropped a no-arg closure header (`{ || … }` → `{ … }`, a crash
      shape) and stripped `()` off a zero-arg call (`row_height()` →
      `row_height`, a call→identifier semantic change). Fix: a zero-param
      `ClosureExpr` always formats with an explicit `||`, and a `Call` node
      always emits its parens (it only exists when the source wrote `()`).
      `formatter/format_expr.rs`. The brace block-arg is already preserved as
      braces. Round-trip pins in `q23_fmt_nondestructive.rs`.
- [x] **Q31 · S1 — repeated `Float`-payload enum construction crashed: enum
      UNDER-ALLOCATION (FIXED 2026-06-09).** `alloc_size` sized enums to packed
      `layout.size` while codegen addresses payloads on a fixed 8-byte slot
      stride (`GetPayload` = base+8, field N at N*8), so `Move(Float32,Float32)`
      stored field 1 four bytes past the 16-byte alloc → heap-metadata
      corruption → crash on the next float `malloc` (hence ≥2 constructions; Int
      payloads on 8-byte slots survived). Fix: slot-round enum allocations to
      `8 + widest_variant_field_count*8` in `mir/lower/emit.rs` (no drops/codegen
      change; enum dealloc was already sound — 3 allocs/3 frees). Pins run+assert
      stdout: `tests/release-e2e/cases/652_enum_float_payload_double_construct`,
      `compiler/ruxen_core/tests/q31_float_enum_payload_drop.rs`,
      `drop_fixtures.rs::q31_…_no_leak`. Unblocks canvas `Int`→`Float32` coords.
- [x] **Q16 · S4 — dependency symbols invisible to library/`check`/`test` builds
      (FIXED).** Library (`compile_piece`), `check`, and `ruxen test` now
      flat-merge dependency `src/**.rx` via the shared `build::gather_dep_sources`
      + `build::resolve_dep_source_dirs`, the same mechanism binary builds use.
      `rondo`/`quiver` can now unit-test against their own public API directly.
      Pin: `src/ruxen_cli/tests/dep_visibility.rs`.
- [x] **Q17 · S4 — generic-free-fn / mixin-bound monomorphization for consumer types.**
      ✅ FIXED for generic FREE FUNCTIONS (feat/drop-elaboration, 2026-06-10).
      Re-scoped empirically: post-Q16 this is a single-unit MIR-lowering gap, not
      cross-package. A generic free fn over a mixin now monomorphizes per concrete
      implementor (incl. generic-calling-generic via a worklist fixpoint), so a
      consumer binary can define a SECOND `PaintSurface` implementor and call the
      dep's generic against both — quiver's framework cap is lifted. Design:
      `docs/decisions/q17-cross-package-monomorphization.md`. **STAGED REMAINDER:**
      generic METHODS over a mixin (a generic `def` inside a class) are not yet
      monomorphized — now a clear lowering error, never a placeholder symbol (see
      below). Also out of scope: true rlib/separate-compilation generics (Q16's
      flat-merge is the model).
- [ ] **Q17b · S4 — generic METHOD over a mixin (staged remainder).** A generic
      `def measure[T: Sized](item: &var T)` INSIDE a class still resolves the bound
      method to a placeholder; lowering rejects it with a clear diagnostic
      ("move the generic into a free function"). Extend the Q17 free-fn
      monomorphization to class/struct/enum methods: collect generic-method
      instantiations, emit specialized method bodies (`Frame_measure__mono__Wide`),
      redirect the method/field-access call sites. Not on quiver's critical path
      (its paint pass is all free functions).
- [ ] **Q2 · S1 — `Option[any Fn[...]]` class field returns garbage.** ⏸ Deferred
      pending the closure representation redesign; apps work around with index-into-pool arrays.
- [ ] **Q22 — closure captures are pointer-copies.** ✅ Audited 2026-06-08 post-P0.2:
      **SOUND, not a UAF** (S4, not the feared S1). Now that `Drop` runs, a captured
      class local is permanently tainted out of the drop set by the capture's
      `SetField` (`drops.rs:728` / `closure.rs:136-150`), so it is **not freed** and the
      stored closure's pointer does not dangle — it **leaks** instead (lives to process
      exit). Does NOT gate the next Drop increment. Open item is the owning-capture /
      keep-alive design (Q4 prerequisite) so escaped handles are freed deterministically
      instead of leaked. Verdict + mechanism in `dev/gui-stack-v1-issues.md` §Q22.
- [x] **Q23 · S4 — `ruxen fmt` is destructive** ✅ FIXED 2026-06-08. (a) Nested
      class/struct/enum/impl/mixin method `##` docs are now emitted
      (`format_func_with_leading_comments`). (b) The shared parser accepts a clean
      top-level expression statement (`TopLevelItem::Expr`) so `ruxen fmt`
      round-trips `Tester.describe` test files; the direct compile path rejects it
      with the new E0728 (`ruxen test` wraps in `def main` and is unaffected).
      Pin: `compiler/ruxen_core/tests/q23_fmt_nondestructive.rs`; doc
      `docs/errors/E0728.md`.
- [x] **Q24 · S4 — stale incremental cache replays false move/borrow diagnostics**
      ✅ FIXED 2026-06-08. The cache key's toolchain component was just
      `CARGO_PKG_VERSION`, invariant across a `--from-source` rebuild, so stale
      objects (with the old compiler's borrow behaviour) were replayed. `compile.rs`
      now folds a `toolchain` fingerprint (exe path + size + mtime) into the cache
      flags, and `CacheKey` gained a `flags` component so the per-object key
      reflects it. Pin: `cache_key_differs_on_flags`.
- [x] **Q25 · S1 — `Hash.key?`/`get` on an EMPTY hash SEGFAULTS**; `&Hash`/`&Set`
      params unsound. ✅ FIXED 2026-06-08. (a) The `string_keys` tristate (-1 unset)
      was C-truthy, so empty-table lookups `strcmp`'d an int key as a `char*` —
      `hash.c` now tests `> 0`. (b) The `&Hash`/`&Set` "unsoundness" was a free-fn
      false-positive E1118 on `&Hash[K,V]` (the `Hash → Hashable` alias); now both
      free fns and methods accept the sound `&Hash[K,V]` collection ref (like
      `&Array[Int]`), and the bare `&Hash` mixin ref is still rejected. Pins:
      `tests/release-e2e/cases/617_*`, `618_*`, `619_*` +
      `compiler/ruxen_core/tests/q25_hash_set_soundness.rs`.
- [x] **Q26 · S1 — capturing closure stored under a `&var *self` reborrow loses its
      captures** ✅ FIXED 2026-06-08. Root cause: a closure nested inside another
      closure's body never re-captured a variable the OUTER block captured
      (`capture_map` was excluded from the visible-defs set). Closure lowering now
      treats `def_to_local ∪ capture_map` as visible and fills re-capture slots from
      the enclosing captures pointer. Unblocks reactive `dyn_text`/`button` children
      in quiver containers. Pins: `tests/release-e2e/cases/615_*`, `616_*` +
      `compiler/ruxen_core/tests/q26_nested_closure_capture.rs`.
- [x] **Q28 · S1 — enum variant `Float`/`Float32` payloads (claimed miscompile)**
      ✅ FIXED / already sound 2026-06-09 (feat/drop-elaboration). The
      `canvas/src/event.rx` `Int`-coordinate TODO ("return to Float32 payloads once
      enum float payloads work") is STALE — like Q22, the note outlived the defect.
      Enum `Float`/`Float32` payloads round-trip exactly (named + positional, single +
      double, mixed with `Int` variants, through fns + Arrays, sub-pixel values). The
      typed MIR `SetField`/`GetField` slot path stores/loads each float at its own
      width; an f32 literal is `coerce_value`-narrowed to f32 in `Assign` before the
      constructor. Fixed as a side effect of Q5 + the case-218 / `1b6ced0` float-codegen
      work. No code change; pinned as a regression guard. Pins:
      `tests/release-e2e/cases/647_enum_float32_payload`, `648_enum_float_mixed_payload`
      + `compiler/ruxen_core/tests/q28_enum_float_payload.rs`. Affected site
      `canvas/src/event.rx` can revert to `Float32` (canvas owner).
- [x] **Q29 · S1 — borrowed `&String` into a `lib "C"` FFI call (claimed wrong
      pointer)** ✅ FIXED / NOT-A-BUG 2026-06-09. A borrowed `&String` forwards the
      correct data pointer + recoverable length today: a Ruxen `String` IS a bare
      NUL-terminated `char*` (no length header), and `MirInst::Ref` is by-value, so the
      `char*` passes through unchanged; C recovers length via `strlen`. The old "char
      count / wrong pointer" claim described the legacy `measure_text_n_raw(n: Int)`
      workaround, not `&String`. Evidence: a borrowed `&String` through `include?`/
      `find`/`replace`/`starts_with` returns exact byte-offset/length-sensitive results.
      Pins: `tests/release-e2e/cases/649_ffi_borrowed_string_arg` +
      `compiler/ruxen_core/tests/q29_ffi_borrowed_string.rs`. Canvas deviation note +
      redundant `measure_text_n_raw` fallback can be reverted (canvas owner).

## Open now — pre-flight P0s (`requirements/ROADMAP.md`)

The P0 list blocks tiers downstream. **It predates recent compiler work and its
file paths reference the old `crates/` layout — re-audit against the current tree
before picking one up** (several may be partially landed; check `CHANGELOG.md`).
Highest-leverage, still-relevant:

- [x] **P0.2 — real `Drop` elaboration (RESOLVED; premise stale).** `MirInst::Drop`
      is an inert marker; `insert_drops` (`mir/lower/drops.rs`) emits per-type free +
      user-destructor calls at scope exit, gated by `compute_dealloc_safe_locals` so a
      moved / returned / FFI-moved value is never dropped. Proven: `drop_fixtures.rs` +
      `user_drop_runs.rs` 19/19, `outstanding == 0`. ADR: `docs/decisions/drop-elaboration.md`.
      Remaining tier1_04 work (drop flags for conditional moves, drop-on-unwind,
      Copy/Clone) is separate and still open.
- [ ] **P0.5 — unresolved generic method calls fall back to `ruxen_noop_passthrough`**
      (silent no-op). Violates the "no silent no-op" rule.
- [ ] **P0.15 — variance rules encoded as comments only; no fixtures prove them.**
- [ ] Remainder (P0.1, P0.3–P0.14): triage against current tree.

## GUI-stack critical path (what `canvas`/`quiver` are waiting on)

The GUI stack (`../canvas`, `../quiver`) has shipped its first vertical slice
but its next cycles are gated here. Prioritized by blast radius:

- [x] **P0.2 — real `Drop` elaboration (RESOLVED).** Heap-owning locals
      (Class/Struct/Enum + String/Array/Map/Set) are freed and user `def drop`
      destructors run at scope exit, in reverse declaration order, exactly once;
      moved / returned / FFI-moved values are never dropped (no double-free /
      use-after-free). The no-GC deterministic-teardown value prop is realized for
      the normal-scope-exit case. Both backends honour it (the free is a MIR `Call`,
      so Cranelift and LLVM behave identically; `MirInst::Drop` itself is an inert
      marker). Leak-audit pin: `drop_fixtures.rs` asserts `outstanding == 0` across
      19 cases. ADR: `docs/decisions/drop-elaboration.md`. **Still open:** drop flags
      for conditionally-moved locals, drop-on-unwind (panic=abort only today),
      Copy/Clone mixins.
- [x] **Q17 — generic-free-fn / mixin-bound monomorphization (FIXED for free
      fns, feat/drop-elaboration).** A generic free function over a mixin now
      monomorphizes per consumer-defined implementor, so quiver's
      single-`PaintSurface` cap is lifted — multiple paint backends now compile +
      run. Generic-calling-generic handled via a worklist fixpoint. **Unblocks:**
      multiple paint backends + clean L1/L2 generic seams. Generic METHODS over a
      mixin remain staged (Q17b above; clear diagnostic, no placeholder symbol).
- [x] **Q16 — dependency symbols in library/`check`/`test` builds (FIXED,
      feat/drop-elaboration).** Dep `src/**.rx` is now flat-merged into library
      (`compile_piece`), `check`, and `ruxen test` builds via the shared
      `build::gather_dep_sources` + `build::resolve_dep_source_dirs` — not just
      binary builds. Symbols enter by source flat-merge (one object, no
      extern-rlib link), so binary builds are unchanged and there is no
      duplicate-symbol risk. Pins: `src/ruxen_cli/tests/dep_visibility.rs`,
      `test_runner` synth-order unit test. ADR:
      `docs/decisions/q16-dep-symbols-in-lib-check-test-builds.md`.
      **Unblocks:** quiver/rondo unit-testing their own public API instead of
      through a sibling binary.
- [x] **Q26 — closure capture lost under `&var *self` reborrow.** ✅ FIXED
      2026-06-08. **Unblocks:** reactive `dyn_text`/`button` children inside quiver
      `Row`/`Col` containers (the widget library's core; previously only static
      `text` children worked in containers). S1.
- [ ] **Enum float payloads + FFI `&String` pointer bugs.** canvas works around
      both (pointer coords forced to `Int`; `measure_text` forwards a char count
      not the string). File/repro as `Q##` if not yet tracked, then canvas
      reverts its deviations. **Unblocks:** correct float event coords + real
      text advance widths.

## Long-horizon

Tiers 1–5 in [`requirements/ROADMAP.md`](requirements/ROADMAP.md): stdlib
completeness & concurrency/async/drop (T1) → type-system features (T2) →
tooling/DX incl. LSP, debugger, docgen, incremental compile (T3) → ecosystem,
package manager, cross-compile, WASM, stable ABI (T4) → spec & stability,
language reference, edition/deprecation, error-code registry (T5). Gate is
`prompts/v1/25_v1_release_checklist.md`.

## How to update this file

- Land a fix → check its box here (or remove it), add the `CHANGELOG.md` entry,
  pin the `tests/release-e2e/cases/` case, and flip the status in the source doc
  (`gui-stack-v1-issues.md` / `requirements/ROADMAP.md`).
- A sibling app hits a new compiler quirk → add a `Q##` entry to
  `dev/gui-stack-v1-issues.md` with a repro, then surface it here under "Open now".
