# ADR: One string type — remove `&str`, collapse to `String` + `&String`

Status: Accepted (2026-06-11)
Branch: `feat/drop-elaboration`
Scope: `compiler/ruxen_core/src/{hir/types.rs, resolve/{exprs.rs,types.rs}, typeck/{unify.rs,coerce.rs,mixins.rs,infer/*}, mir/lower/{emit.rs,expr/*,drops.rs,mod.rs,type_helpers.rs}, codegen/{layout.rs,cranelift/helpers.rs,llvm/types.rs}, borrow_check/lifetimes.rs}`, `src/ruxen_repl/src/display.rs`, `library/std/string/src/string.rx`, `docs/errors/E0726.md`, `docs/tutorial/29_*`, `docs/specs/stdlib/string.spec.md`

## TL;DR

Ruxen has exactly one string pair: **`String`** (owned, heap `char*`, freed at
scope exit via `ruxen_string_free`) and **`&String`** (borrowed reference, no
drop flows through it). The separate `Ty::Str` (`&str`) primitive is **removed
outright**. It served no user-visible purpose: at the C ABI a `String` and a
`&str` are the *same representation* (a bare null-terminated `char*` — there is
no length/capacity header in the runtime), the unifier already bridged
`(Ref(String), Str)` as equal, and `method_home_key` already routed `Ty::Str`'s
whole method surface to `class String`. `&str` was a parallel spelling of the
same wire value that only created drop-safety hazards and overload landmines.

The one genuinely load-bearing role `Ty::Str` played — typing the *raw,
never-freed `.rodata`/FFI `char*` temporaries* that the literal-heap-copy path
and FFI argument marshalling produce — moves to the **existing** `Ty::RawPtr(Char)`
(`*Char`, C's `const char*`), which is already `Copy`, never dropped, and
displays as `*Char`. No "secret" pre-coercion type is kept alive.

## The audit (measured, not assumed)

`Ty::Str` footprint at HEAD `7a35b0a`: 93 Rust references across ~36 files; the
load-bearing seams:

| Seam | File | Role |
|---|---|---|
| Variant decl | `hir/types.rs:280` | `Ty::Str` enum variant |
| Literal birth | `resolve/exprs.rs:39` | `StringLiteral` HIR node born `Ty::Str` |
| Surface spelling | `resolve/types.rs:669` | `str` type name → `Ty::Str` |
| Unifier bridge | `unify.rs:380` | `(Ref(String), Str)` treated equal |
| Method home | `mixins.rs:1164` | `Ty::Str → "String"` (surface method routing) |
| Display name | `mixins.rs:1129`, `hir/types.rs:1036` | prints `"&str"` |
| Coercion | `coerce.rs` | `&str → String` directional coerce |
| Drop callee | `drops.rs:451` | **only `Ty::String` is freed**; `Ty::Str` never is |
| Raw temp | `emit.rs:163`, `expr/mod.rs:109/114` | raw `.rodata`/FFI `char*` temp |
| Layout | `layout.rs:413` (16B) vs `:415` String (24B) | **stale/unused** — see below |

**Drop-safety oracle (`drops.rs:451`):** the drop elaborator frees a local
*iff* its type is `Ty::String` (→ `ruxen_string_free`) AND
`compute_dealloc_safe_locals` proves it owns a fresh allocation. A `Ty::Str`
local is **never** a drop callee. This is exactly the Q38 leak: an owned literal
that stayed `Ty::Str` heap-copied via `ruxen_string_from` but was never freed.

**Representation (the reason the collapse is free):** the runtime has **no**
`RuxenString` struct with a length header. `ruxen_string_from` (`string.c:284`)
returns a plain `malloc`'d null-terminated `char*`; every C entrypoint in
`string.c` takes `const char *` and returns ptr-or-i64-sized. A `String` value
and a `&str` value are *both* a single i64-wide `char*` at the ABI. The
`layout.rs` 16-vs-24-byte divergence is dead data — string values flow as i64
pointers in both backends (`cranelift/emit.rs` `StringLiteral` emits a
`global_value(I64, …)`; there is no 24-byte struct move anywhere). Collapsing is
representationally free.

**App / stdlib impact** (`grep -rn '&str|: str\b' --include='*.rx'`):
- `library/std/string/src/string.rx`: **3 hits, all comments** documenting the
  old `method_home_key: Ty::Str → "String"` mechanism. **Zero** `&str` param
  annotations. Swept here.
- `canvas`: **4 sites** — real `&str` param annotations
  (`examples/demo.rx:3` `what: &str`; `src/canvas.rx:662` & `:681`;
  `src/path.rx:101`). The coordinator sweeps these to `&String` post-install.
- `quiver`: **1 site** — a *comment* (`src/dsl.rx:25` documenting the
  `&str`-vs-closure overload miscompile landmine). The landmine dies; the
  comment can be deleted by the coordinator.
- `rondo`: **0 sites.**

## Decisions

### 1. Literal provenance — born `String`, heap-copy into owned, copy-everywhere

A string literal is born **`Ty::String`** in `resolve/exprs.rs` (extends the Q38
promotion, which only covered un-annotated `let`, to *every* position). Drop
safety:

- **Owned position** (`let s = "x"`, `String` param/field/return/tuple/`Err`):
  materialized via the existing `emit_owned_string_literal`
  (`ruxen_string_from` heap-copy), which is already drop-safe and already what
  Q38/Q39 do. The raw `.rodata` pointer it wraps is a `*Char`, NEVER a drop
  callee (dropping it would `free()` static memory — `emit.rs:159`).
- **Borrow position** (`&String` param): the literal also heap-copies today.
  A zero-copy static-`&String` borrow is sound (no drop flows through a borrow)
  but needs provenance plumbing that is out of scope. **Copy-everywhere is
  CORRECT** — it never leaks and never frees static memory. The zero-copy
  optimization is filed as a follow-up in `docs/TASKS.md`, NOT done here.

This is the load-bearing safety property the drop_fixtures + leak counters pin.

### 2. `Ty::Str` fate — ELIMINATED outright

The `Ty::Str` variant is deleted from `hir/types.rs`. Every arm folds into:
- `Ty::String` — wherever the value is an owned/surface string.
- `Ty::Ref(Box::new(Ty::String))` — wherever a borrow was meant (`&str` → `&String`).
- `Ty::RawPtr(Box::new(Ty::Char))` (`*Char`) — the raw, never-freed `.rodata`/FFI
  `char*` temporaries (`emit_owned_string_literal`'s `raw` temp, the FFI
  pattern/flag temps in `expr/mod.rs`). `RawPtr` is `Copy`, never dropped, and
  already has full layout/codegen/Display support. This is the correct type for
  "a C string pointer I do not own."

No compiler-internal `Ty::Str` survives. No diagnostic, REPL display, LSP hover,
or fmt surface can print `str` after this change (the `mixins.rs:1129` and
`hir/types.rs:1036` `"&str"` Display arms are deleted with the variant).

### 3. Surface spelling `str` / `&str` — HARD REMOVAL (option a)

`str` as a type spelling **errors** with new code **E0726** and the hint:
"`str` is not a type in Ruxen — use `String` for an owned string or `&String`
for a borrowed reference." Rationale: app impact is 4 real sites (all in
`canvas`, swept post-install) + 0 in rondo + 1 comment in quiver; stdlib has 0
real sites. A deprecation alias would add a warning path and a second removal
task for no benefit at this blast radius. The USER said remove; the audit says
removal is cheap. `resolve/types.rs:669` changes from `return Ty::Str` to
emitting E0726 and recovering with `Ty::Error`.

`&"literal"` (previously `&&str`): with one string ref type, `&"x"` is a borrow
of an owned-`String` literal → a clean **`&String`**. The bare-literal idiom
(`"x"` coercing to a `&String` param without the `&`) remains the recommended
form; `&"x"` is accepted as an explicit `&String` borrow, not an error.

### 4. What dies

- **The `&str`-vs-closure overload heap-corruption landmine** (quiver
  `dsl.rx:25`). With one string ref type there is no `&str` arm to collide with
  a closure arm in overload selection. Pinned: a regression test asserts an
  overload set over `(&String, …)` vs `(closure, …)` resolves soundly.
- **The `&"literal"`-is-`&&str` oddity.** Resolved to a clean `&String` borrow
  (decision 3).
- **`method_home_key`'s `Ty::Str` arm**, the `unify.rs:380` `(Ref(String), Str)`
  bridge, the `coerce.rs` `&str → String` arm, the `"&str"` Display arms — all
  deleted, folded into the `String`/`Ref(String)` paths.

### 5. Stdlib sweep

`library/std/string/src/string.rx` has only comments referencing `&str`. They
are updated to say `String` / `&String` and to describe `method_home_key` as
routing the `String` surface (no `Ty::Str`). Bootstrap/embedded lockstep
(`stdlib_embedded.rs`) is regenerated if any embedded copy changes. ABI is
unchanged — both were already `const char*`.

## Staging (commits)

1. **Literals born `String`** (all positions) + leak/double-free pins
   (extend `922_string_literal_coercion_all_positions`; drop_fixtures matrix).
2. **Eliminate `Ty::Str`** — fold all arms to `String`/`Ref(String)`/`RawPtr(Char)`;
   delete the variant, the unify bridge, the coerce arm, the Display/home arms.
3. **Stdlib `&str` comment sweep** + embedded lockstep.
4. **Surface `str` → E0726** + `docs/errors/E0726.md` + registry entry.
5. **Diagnostics/REPL/LSP/fmt never say `str`** + tutorial 29 + string.spec.md.

The staged spine that may NOT slip (honest-scope clause): (1) literals born
`String` with leak/double-free-free pins, (2) stdlib swept, (3) no user-visible
surface prints `str`. Surface-spelling E0726 and full variant elimination ship
together here unless the gate reveals a blast radius requiring a precise
TASKS.md filing — never half-wired, never a silent behavior change.

## Filed follow-ups

- **Zero-copy static `&String` borrow of a literal** — skip the
  `ruxen_string_from` heap-copy when a literal feeds a `&String` param, using
  provenance to keep the `.rodata` pointer and suppress the drop. Out of scope;
  copy-everywhere is correct meanwhile.
- **Layout-table cleanup** — `layout.rs` String=24 is stale (runtime is a bare
  `char*`); reconcile to 8 (pointer) once verified no path reads the 24.
