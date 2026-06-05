# Ruxen GUI — Cross-Platform UI as a First-Class Use Case (Design)

**Status:** Design approved (brainstorming complete). Top-level architecture for a multi-subsystem program; only the **first vertical slice** proceeds to an implementation plan next.

**Goal:** Make cross-platform GUI applications a first-class use case for ruxen via a **package** (not the language), delivering a *consistent* UI framework that exploits ruxen's two genuine edges over the incumbents: **compiled-native (no Electron/VM)** and **memory-safe without a GC (no GC jank → smooth UI)**.

**Non-goal:** Putting any GUI into the ruxen language itself. The language stays general-purpose; everything below L0 is packages.

---

## Why ruxen is positioned for this

Grounded in the current codebase:
- **Compiled-native** — Cranelift (dev/JIT) + LLVM (release), cross-compilation already working. No browser engine to ship.
- **Memory-safe, no GC** — ownership + borrow checking + drop elaboration. The differentiator vs Flutter (Dart GC) and React Native (JS): deterministic, allocation-light frames.
- **First-class C FFI** — the entire stdlib is `lib "C"` bindings; this is the bridge to existing native rendering.
- **Async runtime** — a unified executor/reactor; event loops are exactly what GUIs need.
- **Ruby-flavored syntax + blocks** — ideal for a declarative block DSL.
- **Package system** — `Ruxen.toml` manifests + a dependency resolver (`manifest.rs`, `resolve_deps.rs`) already support third-party packages depending on lower packages.

The incumbents persist because the *language* was never the hard part — **rendering consistency, text/accessibility, and platform integration** are. So the strategy binds proven C libraries for those and puts ruxen's value-add in a type-safe, no-GC, reactive UI API.

---

## Architecture — four layers (1:1 with Dart/Flutter)

| Layer | Ruxen | Flutter analogy | `unsafe`/FFI? |
|---|---|---|---|
| **L0 — Language** | ruxen core (unchanged) | Dart | — |
| **L1 — Engine package** | windowing + input + 2D canvas, via FFI to existing C libs | `dart:ui` / the engine | **Yes — the only such layer** |
| **L2 — Framework package** | signals + Ruby-block DSL + layout + widgets + paint/diff | Flutter framework | **No — 100% safe ruxen** |
| **L3 — Apps** | user app | a Flutter app | No |

**Invariant:** L1 is the *only* layer permitted to be `unsafe`/FFI/platform-specific. L2 is 100% safe, platform-agnostic ruxen. This keeps the language unbloated, makes the framework portable, and is where the no-GC ownership model pays off (deterministic widget/node teardown, no GC jank).

**Consistency model:** Flutter-style **draw-everything** — L2 draws *all* widgets onto L1's canvas (no native controls), for pixel-identical output across platforms.

---

## L1 — Engine package

**Binds existing C packages (builds no renderer):**
- **SDL2/SDL3** — windowing, input, event pump, GL/Metal context. Cross-platform incl. mobile.
- **Skia** — the 2D canvas. *Skia from day one* (it is literally Flutter's renderer: GPU, world-class text shaping/i18n via bundled HarfBuzz/FreeType, pixel-perfect consistency).

**Incremental-FFI discipline (load-bearing constraint):**
The **full Skia C package is vendored in-repo**, but only a **curated subset of methods is exposed via FFI**, grown method-by-method. Nothing is missing from the vendored library; only the *binding surface* is incremental.

Adding a capability is a mechanical 4-step:
1. Wrap the `SkCanvas`/`SkParagraph`/… call in the C shim (`skia_shim.{c,cpp}`).
2. Declare it in L1's `lib "C"` block.
3. Expose it as an L1 `Canvas` method.
4. Use it from L2.

L2's `Canvas` API is designed against the **full intended** surface from the start; L1 fills in coverage over time. **Every newly-bound Skia method gets a pin test** (consistent with the project's test discipline). The build/link strategy (prebuilt Skia binaries per platform vs build-from-source) is an L1-spec detail (see Open Details).

**L1 exposes upward, only:**
- `Window` / `Surface` — created over SDL's context, backed by a Skia surface.
- An **event stream** — input (pointer/keyboard), resize, lifecycle.
- A `Canvas` — `begin_frame`/`end_frame`, `clear`, `draw_rect`, `draw_path`, `draw_text`, `draw_image`, transforms/clips, … (the growing subset).

---

## L2 — Framework package

100% safe ruxen, platform-agnostic. Four concerns: **reactive model**, **DSL**, **layout**, **paint/diff + event dispatch**.

### Reactive model — fine-grained signals (not rebuild-diff)

Chosen over Flutter's rebuild-the-subtree-then-diff because that style leans on a GC; fine-grained signals (SolidJS/Leptos/SwiftUI-style) update **only the exact bound nodes**, with minimal per-frame allocation — the no-GC smoothness story made real.

**Signals vs. ownership (the crux):** fine-grained reactivity needs shared, observable state with subscriptions, which a borrow checker resists. Solution (the pattern Leptos proved in Rust):
- An **arena-scoped signal runtime** owned by the UI root.
- The app holds cheap **`Copy` handles** (indices into the arena), never references — so no aliasing/borrow fight.
- Teardown is **deterministic via the runtime's drop** — no GC, no leaks.

This is the headline L1/L2-boundary design constraint and must be specified precisely in the L2 core spec.

### DSL — Ruby-block, `{}` = reactive / value = static

The builder block runs **once** to construct the node tree. Reactivity granularity comes from a single rule:
- **Plain value** → static content, built once, never re-evaluated.
- **Block / closure `{ ... }`** → a *tracking scope*: the framework subscribes to the signals read inside it and re-runs **only that node** when they change.

```ruxen
fn counter_view(count: Signal[Int]) -> Widget
  column do
    text "Counter"                    # static: built once
    text { "count: #{count.get}" }    # reactive: re-runs ONLY this node when count changes
    button "tap" do                   # static label; trailing block = click handler
      count.update { |c| c + 1 }
    end
  end
end
```

This gives Solid/Leptos-grade granularity using only ruxen's existing blocks/closures — **no `view!`/JSX macro and no language change required.** (JSX was rejected: it would require a custom-syntax macro facility; the Ruby-block form is more native to a Ruby-flavored language.)

### Layout
A constraint/flex layout pass over the widget tree, producing geometry consumed by the paint pass. (Engine choice — own simple flex vs binding a C layout lib like Yoga — is an L2-spec detail.)

### Paint / diff + event dispatch
- The widget tree paints onto L1's `Canvas`.
- Signal changes invalidate only their tracking scopes → targeted repaint (no full-tree diff).
- L1's event stream is dispatched to the appropriate node's handlers (e.g. the `button do … end` click block).

---

## First vertical slice — desktop counter app

The minimum that proves the **entire** stack end-to-end and de-risks the *language-level* API (the actual risk):

1. SDL window → Skia surface on its GL/Metal context.
2. L1 **minimal** canvas FFI: `begin_frame`/`end_frame`, `clear(color)`, `draw_rect`, `draw_text` (one font).
3. L2 **minimal**: the signal arena runtime + `Signal[T]`; the block DSL for `column`/`text`/`button`; one simple layout pass; paint; click → `count.update` → targeted repaint.
4. Demo app: a counter — tap a button, the number increments. The "hello world" of reactive GUI.

Proves: L1 FFI ↔ Skia ↔ window/events ↔ L2 signals ↔ block DSL ↔ targeted repaint, on **one** desktop platform, with a *minimal* Skia FFI surface.

**Explicitly out of the first slice:** mobile/web, the full widget library, accessibility, text i18n/shaping beyond basic, packaging.

---

## Decomposition (each its own spec → plan → build)

1. **L1 engine** — Skia vendoring + build/link + the C shim + SDL windowing/input + the minimal canvas FFI + the incremental-FFI pattern.
2. **L2 core** — signal arena runtime + `Copy` handles + the Ruby-block DSL + layout + paint/diff + event dispatch.
3. **Widget library** — buttons, text, lists, inputs, containers, …
4. **Text / i18n / accessibility** — Skia paragraph/HarfBuzz/ICU; platform accessibility trees.
5. **Platform matrix** — macOS/Windows/Linux → Android/iOS → web (WASM + canvas).
6. **Packaging / distribution** — `.app`/`.apk`/`.ipa`/`.msi`, permissions, lifecycle.

**The first implementation plan covers a thin vertical through #1 + #2 only** (the counter app). The other five are future cycles.

---

## Open details (spec-level, NOT architectural forks)

These are resolved *inside* the relevant sub-project spec, not here:
- **Signal API shape** — `count.get`/`count.set` vs call-style `count()`.
- **Skia build/link** — vendor prebuilt binaries per platform vs build-from-source.
- **Platform order** after desktop — web/WASM next vs mobile.
- **Layout engine** — own simple flex vs bind Yoga (C).
- **First-slice widget set** — bounded by the counter app (`column`, `text`, `button`).

---

## Risks

- **Skia build/link complexity** — C++ + large binary; the L1 build story is the main engineering risk. Mitigated by vendoring + the incremental shim (bind only what's used) and by deciding prebuilt-vs-source in the L1 spec.
- **Signals + ownership ergonomics** — the arena/`Copy`-handle pattern must feel natural in ruxen; prototype in the L2 core slice before committing the full API.
- **Block-DSL reactivity boundary** — the `{}`-reactive / value-static rule must be unambiguous and well-documented; pin it with tests in the L2 core slice.
- **Scope** — this is a multi-year program; discipline is to ship the desktop slice first and expand the platform matrix only after the language-level API is proven.

---

## Out of scope / non-goals
- No GUI in the ruxen language.
- No bespoke renderer (bind Skia).
- No native-widget wrapping (draw-everything for consistency).
- First slice is desktop-only; no mobile/web/packaging until later cycles.
