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

20 of 26 fixed (Q23–Q26 surfaced 2026-06-08 building the GUI stack). Outstanding:

- [ ] **Q16 · S4 — dependency symbols invisible to library/`check`/`test` builds.**
      `ruxen test` and `ruxen check` can't see dependency symbols; only binary
      builds merge dependency sources. This is why `rondo`/`quiver` can't unit-test
      against their own public API and rely on sibling binary crates. **Dedicated effort.**
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
- [ ] **Q23 · S4 — `ruxen fmt` is destructive** (strips `##` doc comments; can't
      parse `Tester.describe` test files). High-friction: the project convention +
      the app Stop hooks tell contributors to run it. Format by hand until fixed.
- [ ] **Q24 · S4 — stale incremental cache replays false move/borrow diagnostics**
      with bogus line numbers across dirs (`ruxen build`/`test`; `check` is correct).
      Workaround `rm -rf target/ruxen/{incremental,test-build}`. This amplified the
      Q18 stale-toolchain confusion. Both detailed in `dev/gui-stack-v1-issues.md`.
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
- [ ] **Q16 — dependency symbols in library/`check`/`test` builds.**
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
