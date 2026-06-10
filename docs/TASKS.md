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

28 of 33 fixed (Q23–Q26 surfaced 2026-06-08; Q16 fixed 2026-06-08 on
feat/drop-elaboration; Q29 audited 2026-06-09 — already sound, pinned; Q28, Q30,
Q31 fixed 2026-06-09). **Q31 was an enum UNDER-ALLOCATION** (not a drop
double-free): `alloc_size` sized enums to packed layout while codegen addresses
payloads on a fixed 8-byte slot stride, so `Move(Float32,Float32)` stored field 1
four bytes past the alloc → heap corruption → crash on the next float `malloc`.
Fixed by slot-rounding enum allocations (`8 + widest_variant_field_count*8`).
The canvas `Int`→`Float32` event-coord revert it unblocked has now LANDED
(canvas 143 green, sub-pixel pinned, live windowed loop verified). **Q32 + Q33
NEW 2026-06-09** (filed during that revert). Outstanding:

- [ ] **Q32 · S3 — Q16 flat-merge of an FFI dependency breaks `ruxen test` at
      link.** A consumer's test EXECUTABLE flat-merges the FFI dep's `src/**.rx`
      (incl. `lib "C"`-calling bodies) but neither compiles its `runtime/**.c`
      nor propagates its `[system_libs]` → every `ruxen_canvas_*` symbol
      undefined. quiver dodged it by dropping its (genuinely unused) canvas dep;
      any consumer that really `use`s an FFI dep will hit it. Preferred fix:
      when flat-merging a dep, also link its C runtime + system libs (what
      binary builds already get when the dep is declared directly). Details in
      `dev/gui-stack-v1-issues.md` §Q32.
- [ ] **Q33 · S2 — `Float32 == <negative Int literal>` evaluates false.** The
      value is correct; positive literals, `<`/`>`, as-Int compares, and
      f32==f32 all fine — only equality against a NEGATIVE Int literal
      miscompares (unary-minus literal lowering is the prime suspect). Repro:
      `tmp/test-cache/q33-negative-literal-f32-compare-repro.rx`; canvas works
      around in `tests/scroll_resize.rx`. Details §Q33.

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
- [ ] **Q17 · S4 — cross-package generic monomorphization fails for consumer types.**
      A dependency's generic can't be monomorphized for a type defined in the
      consuming package (forces quiver's single-implementor `PaintSurface` shape).
      **Dedicated effort.** (Q16 + Q17 together unblock real multi-package testing.)
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
- [ ] **Q17 — cross-package generic monomorphization.** Forces quiver's
      single-`PaintSurface` shape. **Unblocks:** multiple paint backends + clean
      L1/L2 generic seams.
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
