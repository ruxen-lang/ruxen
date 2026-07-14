# ADR: Q17/Q17b — generic callable / mixin-bound monomorphization

Status: Accepted (2026-06-10)
Updated: Q17b method completion (2026-07-14)
Branch: `feat/drop-elaboration`
Scope: `compiler/ruxen_core/src/mir/lower/{monomorphize.rs, mod.rs, expr/fn_call.rs, expr/method_call.rs, expr/field_access.rs}`

## TL;DR

Q17 was filed as "cross-PACKAGE generic monomorphization". **After Q16 landed
(dep `src/**.rx` flat-merged into the consuming compilation unit), it is no
longer a cross-package problem at all** — it reproduces in a SINGLE FILE with no
packages. The real defect is narrower and entirely inside MIR lowering: a
**generic free function (or method) bound by a mixin** is lowered exactly ONCE
with the type parameter abstract, and a call to a bound method inside its body
(`s.fill_rect(...)` where `s: &var T, T: Paintable`) is mangled to the literal
bound-placeholder symbol `T: Paintable_fill_rect`, which link-fails. A
single-implementor fast path masks this by devirtualizing; with ≥2 implementors
there is no unique impl and the placeholder leaks to the linker — exactly the
"bound-placeholder garbage" the ledger warned about.

## Empirical failure modes (measured, not assumed)

Repro (single file, `ruxen compile q17.rx`):

```ruxen
mixin Paintable
  def fill_rect(w: Int, h: Int) -> Int
end
def paint_all[T: Paintable](s: &var T, w: Int, h: Int) -> Int
  s.fill_rect(w, h)
end
class RecordingSurface include Paintable; … def var fill_rect(...) … end
class TallySurface    include Paintable; … def var fill_rect(...) … end
def main
  let a = paint_all(&var RecordingSurface.new, 4, 5)
  let b = paint_all(&var TallySurface.new, 4, 5)   # 2nd implementor
end
```

```
Undefined symbols for architecture arm64:
  "_T: Paintable_fill_rect", referenced from:
      _paint_all in q17.o
```

The emitted MIR for `paint_all` is a single opaque body:

```
=== MIR function: paint_all ===
    Call { dest: Some(3), callee: "T: Paintable_fill_rect", args: [Use(0), Use(1), Use(2)] }
```

Findings:

1. **Not cross-package.** Q16's flat-merge already puts the dependency's generic
   and the consumer's implementor in ONE compilation unit. The two-package
   `ruxen build` repro and the single-file `ruxen compile` repro fail
   identically. The compilation model is settled (flat-merge); we work within it.
2. **Where the placeholder is born.** `method_call.rs` lines ~851-871: for a
   receiver of `Ty::TypeParam { bounds } | SomeMixin | AnyMixin` (after peeling
   refs), the resolved class is `self.unique_bound_impl(bounds)
   .unwrap_or_else(|| type_name.clone())`. `type_name` here is
   `type_name_from_ty(&TypeParam{name:"T", bounds:[Paintable]})` =
   `"T: Paintable"`, so the mangled callee becomes `T: Paintable_fill_rect`.
3. **The single-impl fast path is the mask.** `unique_bound_impl` (`mod.rs:240`)
   returns `Some(only_impl)` when a bound has exactly one implementor, so
   `paint_all` over a single-implementor mixin devirtualizes to
   `RecordingSurface_fill_rect` and links. This is precisely why quiver is
   capped at one `PaintSurface` implementor. With 2+ impls `unique_bound_impl`
   returns `None`, the placeholder leaks, and the link fails.
4. **Generic FREE functions are never monomorphized today.** `fn_call.rs`
   emits a single `Call { callee: callee_name }` — no generic-arg suffix, no
   per-instantiation body. The only monomorphization that exists
   (`monomorphize.rs`) is for generic CLASSES keyed on the *receiver*
   instantiation, and it explicitly does NOT cover free functions or
   mixin-bound type params.

## Decision

Generalize the existing generic-CLASS monomorphization machinery to cover
**generic free functions and generic methods whose type params are mixin-bound**,
keying on the **concrete type argument inferred at each call site**. Define
mixin-generic dispatch as **monomorphize-per-instantiation**, with
devirtualization (`unique_bound_impl`) retained ONLY as the single-implementor
fast path so quiver's current shape emits byte-identical code.

Concretely, three coordinated changes, all in MIR lowering (shared by both
backends — they consume `MirProgram`, so no backend-specific work):

1. **Collect free-fn/method instantiations (demand-driven).** The
   `collect_generic_fn_instances` and `collect_generic_method_instances` passes
   walk `HirExprKind::FnCall` / resolved `MethodCall` nodes. For a callee that is
   generic (≥1
   `generic_param`) with at least one **mixin-bounded** type param, it unifies
   the callee's declared param types against the call's actual (concrete) arg
   `.ty`s to recover `{ T → ConcreteTy }`. Only **fully-concrete** bindings
   (reusing `is_fully_concrete`) for which **every** generic param is resolved
   are recorded. The instantiation set is deduped on the emitted mangled base.
   This is demand-driven: only `(callee, type-args)` pairs that actually appear
   in a call are recorded, so there is no combinatorial blowup.

2. **Emit one specialized body per instantiation.**
   `emit_generic_fn_instances` / `emit_generic_method_instances` clone the
   callee `HirFuncDef`, run the existing
   `trait_default::subst_type_params_in_func` with the recovered substitution
   (`T → TallySurface`), and lowers it under a mangled name
   `paint_all__mono__TallySurface` or
   `Frame_measure_scaled__mono__Wide`. Method copies lower through
   `lower_method`, retaining fixed `self` as their first parameter. After
   substitution the bounded parameter's
   `.ty` is `&var TallySurface` (concrete), so the body's `s.fill_rect(...)`
   lowers through the ordinary `Ty::Class` dispatch arm to
   `TallySurface_fill_rect` — no new dispatch logic, the existing concrete-class
   path does the work. This is exactly how class-mono already specializes
   method bodies; we reuse `MONO_SEP` and the `mono_base`/`strip_mono_suffix`
   helpers unchanged.

3. **Redirect call sites.** `fn_call.rs` and `method_call.rs` consult their
   emitted-instantiation maps.
   When the call's recovered type-arg vector matches an emitted instantiation,
   the callee is rewritten to the mangled name. Eligible opaque bodies are
   suppressed because they can emit placeholder callees; a call with no
   concrete emitted match receives a clear lowering error.

### Why suppressing the abstract fallback is sound

Every concrete call targets a body in which all method-level bounded parameters
have been substituted. Eligible opaque free-function and method bodies are never
emitted, so correctness does not depend on linker dead-stripping. If collection
cannot reach a concrete leaf (including a generic-calling-generic method
boundary), call lowering errors before emitting a dangling or placeholder
callee. `unique_bound_impl` remains available for ordinary concrete lowering,
but is no longer the safety mechanism for these generic bodies.

## What is OUT of scope (sound partial boundary)

- **True separate compilation / rlib generics.** Q16's flat-merge is the
  compilation model — every consuming unit re-merges all transitive dep sources,
  so a dep generic and a consumer type are always co-compiled. We do NOT add
  cross-rlib generic instantiation; there is no such boundary to cross.
- **Higher-ranked / nested generic instantiation through an intermediate generic
  caller.** If `paint_all[T]` is itself called only from another generic
  `wrap[U: Paintable](x: &var U) { paint_all(x) }` with no concrete `U` at any
  leaf call, the type arg stays abstract and cannot be monomorphized. This is
  rejected with a clear diagnostic, NOT miscompiled (see Risks). The common
  shape — a concrete implementor passed to a dep generic — is fully covered.
- **Namespacing** (Q14) — unchanged; flat symbol space.

## Risks and how the design avoids them

| Risk | Mitigation |
|---|---|
| **Bound-placeholder garbage** (`T: Paintable_fill_rect`) reaching the linker — the exact Q17 bug | Every concrete call site is redirected to a monomorphic copy whose body dispatches on a concrete receiver. Placeholder-prone opaque free-function and method bodies are suppressed unconditionally. |
| **An unmonomorphizable call** (type arg stays abstract — generic caller, no concrete leaf) | With no emitted concrete match, call lowering returns a clear `Err`; it never falls back to the suppressed opaque body or emits a placeholder/missing symbol. Free-function generic chains reach a fixpoint; cross-method generic chains remain the explicit residual. |
| **Compile-time blowup** | Collection is demand-driven: only `(callee, concrete-args)` pairs that literally appear at a call site are recorded, deduped on the mangled base. N call sites with K distinct concrete types → K copies, not K^params. Matches class-mono's cost model. |
| **Regressing quiver's single-implementor shape** | `unique_bound_impl` is untouched and still runs first for the 1-impl case; a generic fn with a single-implementor bound records ONE instantiation that lowers to the same concrete callee the devirtualize path already produced. An existing-case e2e build is diffed before/after to confirm byte-stability of the single-impl path. |
| **Different field layouts between implementors** | Each monomorphic copy is a fully concrete body; field slot indices are resolved per concrete class by the normal `Ty::Class` field-access path. Layouts never mix. |

## Acceptance bar (non-negotiable — quiver's blocker)

A consumer binary defines a SECOND mixin implementor and calls a dependency's
generic against BOTH the dep's own implementor and the consumer's, in one
program, printing distinct correct values from each — proving real
monomorphization, not devirtualize-to-one. Pinned by staged-install integration
tests (the `dep_visibility.rs` pattern) that compile + run + assert stdout, plus
single-unit release-e2e cases (655+).

## Staged remainder status (updated by Q17b)

The generic-METHOD remainder is complete for methods declared directly inside
class, struct, and enum receivers. Pin `474_generic_method_mixin_mono.rx` runs
two concrete implementors through distinct specialized class-method bodies; the
focused compiler suite additionally runs generic methods owned by a struct and
an enum. A generic-calling-generic chain that crosses method boundaries is still
outside the worklist fixpoint; it is rejected clearly at lowering with no opaque
fallback, placeholder symbol, or silent miscompile. True separate-compilation
generics remain out of scope under Q16's source flat-merge model.
