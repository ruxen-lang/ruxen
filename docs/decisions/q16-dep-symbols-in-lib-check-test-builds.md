# ADR: Q16 — dependency symbols visible to library / `check` / `test` builds

Status: Accepted (2026-06-08)
Branch: `feat/drop-elaboration`
Scope: `src/ruxen_cli/src/build.rs`, `src/ruxenc/src/test_runner.rs`

## Context

Ruxen's v1 build driver merges a dependency package's source into the
consuming package by **flat-merging** the dep's `src/**.rx` ahead of the
user source in a single compilation unit (no `use <pkg>.X` namespacing
yet — see B12 / Q14). Today this flat-merge happens in exactly ONE place:

- `build.rs::compile_project` — the **binary** build path. It receives
  `dep_source_dirs`, prepends each dep's gathered source, and (critically)
  **skips extern-rlib linking** while doing so (lines ~663–668), because
  the dep's object would otherwise duplicate every symbol the merged user
  object now also emits.

Three other build kinds get NO dependency sources:

1. `compile_project`'s sibling `compile_piece` — the **library** build
   path (`build_type() == "library"`). It calls `gather_sources` for the
   dep/lib's own `src/**.rx` only; `_extern_libs` is received but unused.
2. `build.rs::check` — `ruxen check`. It gathers only the project's own
   `ModuleTree` sources and type-checks them.
3. `test_runner.rs::gather_project_lib_sources` — the `ruxen test`
   wrapper. Its doc comment is explicit: *"ruxenc is a single-file driver
   (it does not resolve project deps)"*. It merges only the project's own
   `src/**.rx`.

**Consequence:** a library (e.g. quiver) that path-depends on another
package (canvas) cannot reference any dependency symbol from `src/lib.rx`,
from `ruxen check`, or from a `tests/**.rx` test file. quiver must put its
canvas adapter in a separate example **binary** package and test its
public API through that binary — the workaround Q16 exists to remove.

## Decision

Reuse the binary path's already-proven **flat-merge** strategy for the
other three build kinds. Flat-merge is the right primitive precisely
because it already solves the double-link / duplicate-symbol problem: the
dep is compiled *into* the consuming compilation unit, so there is exactly
one object and exactly one definition of every symbol. We do not introduce
a second linking mechanism (extern rlibs) for lib/test, which would
reintroduce the duplicate-symbol risk the binary path deliberately avoids.

Concretely:

1. **Extract** the inline dep-source flat-merge in `compile_project`
   (lines ~577–589) into a free helper:

   ```rust
   fn gather_dep_sources(dep_source_dirs: &[PathBuf]) -> Result<String, String>
   ```

   `compile_project` calls it and is otherwise unchanged — binary builds
   behave bit-for-bit as before (same ordering, same skip-extern-link
   when `dep_source_dirs` is non-empty).

2. **Library path (`compile_piece`).** Thread resolved dep source dirs in.
   When non-empty, prepend `gather_dep_sources(deps)` ahead of the lib's
   own gathered source, exactly as the binary path does. The rlib that
   results contains the dep symbols inlined — which is acceptable for v1
   because:
   - A library rlib is only consumed by being flat-merged again into a
     downstream binary's compilation unit (the binary path re-gathers ALL
     transitive dep sources itself from `resolve_result.deps`, which is a
     full topological closure — see `build()` Step 1/2). So the inlined
     copy in the intermediate rlib is never the copy that links the final
     binary; the binary path's own flat-merge + skip-extern-link governs
     the final link. No duplicate symbol reaches a linker.
   - The rlib's purpose at the library-build stage is to **prove the
     library type-checks and lowers against its deps** (the acceptance
     criterion) and to surface dep symbols to the library's own `src`.
   Skip-extern-link is preserved: `compile_piece` does not link extern
   rlibs (it never did — `_extern_libs` was already unused), so there is
   no double-link to introduce.

3. **`check`.** Resolve deps (same path as `build()` Step 1, guarded by
   `!manifest.dependencies.is_empty()`), gather their sources via
   `gather_dep_sources`, and prepend to the combined project source before
   `check_single_file`. `check` does no codegen/link, so the only concern
   is symbol *visibility* during typeck — which flat-merge provides.

4. **`ruxen test`.** Give `gather_project_lib_sources` a sibling that
   resolves the project's path deps and gathers their sources, and prepend
   those (ahead of the project's own lib source) into the synthesised
   wrapper. The wrapper already compiles as a single-file unit through
   `ruxenc`, so flat-merge is the only mechanism that can work here. The
   dep sources go ABOVE the project's own lib source, which goes above the
   synthesised `def main` — preserving Q18's hoist ordering.

## Ordering & determinism

Merge order is fixed and total: **deps (topologically sorted) → project
own source → user/test body**. Within deps we reuse the resolver's
topological order (`resolve_result.deps`) for `build`/`check`, and a
manifest-declared path-dep order for the test runner. This matches the
binary path's existing ordering so a library and a binary in the same
workspace see identical declaration order.

## Avoiding duplicate symbols / double link — the safety argument

| Build kind | Dep entry | Extern-rlib link? | Duplicate-symbol risk |
|---|---|---|---|
| binary (today) | flat-merge | skipped when deps merged | none (1 object) |
| library (Q16) | flat-merge | never linked extern | none (1 object per rlib; final link is the binary's own re-merge) |
| check (Q16) | flat-merge | n/a (no codegen) | n/a |
| test (Q16) | flat-merge | n/a (single-file unit) | none (1 object) |

The invariant we rely on: **a dependency's symbols enter a consuming
compilation unit by source flat-merge, never by extern-rlib link, whenever
that unit also contains user code that could re-emit the dep's helper
symbols.** Every build kind above honors it.

## What this does NOT cover (sound partial boundary)

- **Namespacing.** Still flat (no `use <pkg>.X` scoping) — a dep symbol
  collides with a same-named user symbol. This is Q14's job; unchanged.
- **Transitive dep closure for `check`/`test`.** `check`/`test` resolve
  the project's *direct* deps. The binary path already gathers the full
  topological closure for codegen; for `check`/`test` (visibility only)
  direct deps cover the acceptance fixture. Deep transitive `use`-in-test
  is a follow-up, noted in the issue entry, not silently wrong: a missing
  transitive symbol surfaces as a normal "unknown symbol" typeck error,
  not a miscompile.
- **Q17** (cross-package generic monomorphization for a *consumer-defined*
  type) is a separate, codegen-deep item. Q16 makes the dep's symbols
  *visible*; Q17 is about *instantiating* a dep generic with a consumer
  type. Tracked separately.

## Acceptance

A two-package fixture: lib `dep-color` exporting `struct Color`; lib
`consumer` with `dep-color = { path = "../dep-color" }` that `use`s
`Color` in `src/lib.rx` and in a `tests/**.rx` file. After this change,
`ruxen build`, `ruxen check`, and `ruxen test` in `consumer` all succeed.
Pinned by `src/ruxen_cli/tests/dep_visibility.rs` (modeled on
`package_manager.rs`) plus a `test_runner` unit pin for the dep-source
gatherer.
