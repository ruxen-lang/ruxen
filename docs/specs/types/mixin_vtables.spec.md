# Spec — Mixin vtables (runtime dispatch over mixin implementors)

**Source docs:**
[docs/specs/types/mixin_default_methods.spec.md](mixin_default_methods.spec.md),
[docs/specs/stdlib/task_spawn.spec.md](../stdlib/task_spawn.spec.md) (sub-phase 5 — blocked on this),
[docs/specs/stdlib/async.spec.md](../stdlib/async.spec.md) (Future mixin),
[docs/specs/ownership/drop.spec.md](../ownership/drop.spec.md).

**Status:** new — load-bearing compiler feature. Today every mixin
method dispatch is statically resolved at the call site (full type
info known from the receiver's class). This breaks when the
dispatching side has only an opaque pointer — e.g., a C scheduler
holding a queue of heterogeneous `Future` implementors needs to
call `poll(self, ctx)` without knowing each task's concrete class.

The fix: per-mixin vtable, populated per implementor class at lowering
time, retrievable at runtime via a stable header offset on every
heap-allocated instance that includes the mixin.

This unblocks:
- **Sub-phase 5 (`Task.spawn`)** — C scheduler can dispatch `poll`
  on any queued future via its vtable.
- **`include Drop` as required contract** — task #14 B4 promotion
  deferred precisely because Drop dispatch needed runtime lookup.
- **`Display`/`Debug` in `puts "#{any_obj}"` for non-stdlib types** —
  string interpolation could dispatch through a vtable instead of
  the current hardcoded primitive-only path.
- **`Iterator` and `Iterable`** — generic `each`/`map`/`filter` over
  a heterogeneous collection.

---

## B1 — Vtable opt-in via `mixin Foo dispatch runtime`

The mixin author marks the mixin as runtime-dispatched:

```rx
mixin Future dispatch runtime
  type Output
  def var poll(cx: &var Context) -> Poll[Self.Output]
end
```

`dispatch runtime` is a NEW mixin modifier (parser addition). Mixins
without it (`mixin Comparable`, `mixin Copy`, etc.) stay statically
dispatched — no vtable overhead. Only mixins that NEED runtime
dispatch opt in.

**Why opt-in:** vtables add per-instance memory (header pointer) and
per-call indirection. For mixins that are always statically
dispatched (auto-derived markers, structural mixins), the cost isn't
warranted.

## B2 — Vtable header layout

For every class C that includes a `dispatch runtime` mixin M, the
compiler prepends an 8-byte header field to C's heap allocation:

```
+------------------+ offset 0
| vtable_ptr (i64) | → points to a static M-vtable for C
+------------------+ offset 8
| ... user fields ...
```

If C includes multiple `dispatch runtime` mixins (M1 + M2), the
header is widened to one slot per mixin:

```
+----------------+ offset 0
| M1_vtable_ptr  |
+----------------+ offset 8
| M2_vtable_ptr  |
+----------------+ offset 16
| ... fields ...
```

Order is mixin declaration order in the class body. Stable per class.

## B3 — Static vtable struct (per implementor class)

For each (mixin M, class C) where C `include M`, the compiler emits
a static C-callable vtable at codegen time:

```c
// Synthesised for each implementor.
struct __FutureVtable_TimeSleepFuture {
    int64_t (*poll)(int64_t self, int64_t ctx);
    // any other mixin methods, in declaration order
};
static const struct __FutureVtable_TimeSleepFuture
    __future_vtable_TimeSleepFuture = {
        .poll = TimeSleepFuture_poll,
        // ...
    };
```

The vtable's function pointers reference the existing Ruxen-emitted
method symbols (`TimeSleepFuture_poll` etc.) so no extra trampoline
generation is needed — the symbols are already C-callable per the
existing FFI ABI.

## B4 — Class construction populates the vtable header slot

At class construction (every `__init` method, including the
synth-generated `<Class>.new`), the codegen emits:

```rx
# Synthesised at the head of every __init:
@vtable_slot_for(Future) = &__future_vtable_<Class>
```

For C-backed classes (Mutex, etc. with no user-body `def init`),
the existing C `ruxen_<class>_new` function gains a `vtable_ptr`
write at the top of its body. This is one of the few places the
trio-leak pin's "no compiler/src/ edits" rule has a legitimate
exception — but the edit is mechanical, one line per builtin
constructor in `library/std/*/runtime/*.c`.

## B5 — Runtime dispatch helper

For each `dispatch runtime` mixin M, the compiler emits a runtime
helper:

```c
// Generated once per mixin.
int64_t Future_dynamic_poll(int64_t self, int64_t ctx) {
    // Read vtable_ptr from offset of Future in self's header.
    struct __FutureVtable *v =
        *(struct __FutureVtable **)((char*)self + FUTURE_VTABLE_OFFSET);
    return v->poll(self, ctx);
}
```

`FUTURE_VTABLE_OFFSET` is the offset of Future's slot in the header
for the class containing `self`. **Question: how does the helper
know the offset?** Two options:
- (a) **Single shared offset**: every `dispatch runtime` mixin gets
  the same slot ordering across all classes — but classes
  including different sets of mixins would have different slot
  counts and offsets. Doesn't work.
- (b) **Vtable-of-vtables**: the header's first slot is ALWAYS a
  pointer to a "class info struct" containing all mixin vtable
  pointers. `Future_dynamic_poll` reads class_info first, then
  reads the Future vtable from a known offset within class_info.
  Adds one extra dereference per call but supports arbitrary mixin
  combinations cleanly.

**Choose (b)** for v1. Single class-info pointer at offset 0.
Per-mixin vtables live inside the class-info, at offsets the
compiler computes from each mixin's declaration order in the
class body.

## B6 — Mixin method call dispatches dynamically

Today: `f.poll(ctx)` where `f: TimeSleepFuture` lowers to
`TimeSleepFuture_poll(f, ctx)` (static dispatch). This stays
unchanged for direct method calls.

The dynamic path activates when the receiver's type is the **mixin
itself** (e.g., a `dyn Future` parameter or a polymorphic slot):

```rx
def run_one(fut: &var Future) -> ()
  match fut.poll(cx)
    ...
  end
end
```

`fut: &var Future` is a `dyn`-shaped reference. The call lowers to
`Future_dynamic_poll(fut_ptr, cx_ptr)` instead of a static
`<ConcreteClass>_poll`. The vtable in fut's header resolves the
concrete poll fn.

For sub-phase 5's task queue, every enqueued future is held as
`&var Future` — the scheduler calls `Future_dynamic_poll` on each.

## B7 — `dyn Mixin` parameter / field type

New syntax: `&Mixin` / `&var Mixin` as parameter / field type
denotes a runtime-dispatched reference. The static type check
verifies the receiver's class includes the mixin; the runtime
dispatch reads the vtable.

```rx
let tasks: Array[&var Future] = Array.new()    # heterogeneous task queue
tasks.push(some_future_a)
tasks.push(some_future_b)   # different concrete type, both Future-includers
```

Implementation: `&var Future` is `Ty::DynRef { mixin: "Future", mode: Mut }`
in the type system. Carries one i64 (the heap pointer to the
class-info-headed instance).

## B8 — Vtable-of-vtables layout (class-info struct)

For each class C, the compiler emits ONE class-info struct:

```c
struct __ClassInfo_TimeSleepFuture {
    const struct __FutureVtable_TimeSleepFuture *future_vtable;
    // const struct __DropVtable_TimeSleepFuture *drop_vtable;  // if Drop also opted into runtime dispatch
    // ... one slot per `dispatch runtime` mixin C includes
};

static const struct __ClassInfo_TimeSleepFuture __classinfo_TimeSleepFuture = {
    .future_vtable = &__future_vtable_TimeSleepFuture,
};
```

The slot offsets within `__ClassInfo_<C>` follow the order in which
C's body includes runtime-dispatched mixins. Stable per class.

`Future_dynamic_poll` reads:
1. `class_info = *(ClassInfo**)(self + 0)` — pointer at offset 0.
2. `future_vtable = class_info->future_vtable` — at the offset Future
   has in this class's info struct.
3. `future_vtable->poll(self, ctx)`.

The offset for each mixin's slot in a class's info struct is
computed at codegen time and emitted into the runtime helper as a
literal: `Future_dynamic_poll` for class C uses C's specific offset.
This means the helper isn't truly generic across classes — it's
keyed by class. Each (mixin, class) pair gets its own dispatch
helper. OR a SINGLE generic helper exists that takes the offset as
a parameter, and call sites pass `offsetof_Future_in_<C>` from a
codegen-emitted const table.

**Choose**: single generic helper + per-class offset table. Less code
bloat than per-(mixin, class) helpers.

## B9 — Drop registration via runtime dispatch (UNBLOCKS task #14 B4)

Once mixin vtables ship, `include Drop` can become a contract that
the compiler enforces structurally:

- Adding `include Drop` to a class makes its drop dispatch go
  through a vtable.
- `collect_user_drop_classes` becomes redundant — Drop dispatch is
  via `Drop_dynamic_drop(self)` for any class with a Drop slot in
  its class-info.
- `def __drop` vs `def drop` name mismatch (memory note
  `project_ruxen_drop_name_mismatch.md`) becomes a compile-time
  diagnostic if `include Drop` is present but `def drop` is missing.

This is the "promote silent state to required contract" work from
task #14 B4, deferred precisely until this mechanism existed.

## B10 — Codegen budget

Each `dispatch runtime` mixin + each implementor class C adds:
- One vtable static struct (~8-32 bytes per mixin method count).
- One class-info struct slot (8 bytes per mixin).
- One init-time write to `self.class_info_ptr`.
- One indirect call at every `&Mixin`-typed dispatch site.

Per-call overhead: one extra load + one indirect call. Comparable
to Rust's `dyn Trait`. Acceptable for v1.

## B11 — Memory layout impact on existing classes

Every class that includes a `dispatch runtime` mixin gains 8 bytes
at offset 0 (the class_info pointer). This affects:
- C runtime `sizeof()` of every Mutex / SharedSync / file / etc. that
  includes Drop with runtime dispatch.
- Existing C helpers that compute heap offsets must be updated to
  account for the new header.

**Decision:** for v1, only `Future` opts into `dispatch runtime`.
Other mixins (Drop, Display, etc.) stay statically dispatched until
they need runtime dispatch. Avoids forcing a layout change on every
heap-allocated stdlib type.

The migration to `Drop dispatch runtime` etc. is a separate
follow-up — each mixin's migration is its own commit + memory-layout
review.

---

## Pin tests

| Behaviour | Test fn                                                | File                       |
|-----------|--------------------------------------------------------|----------------------------|
| B1        | `mixin_dispatch_runtime_modifier_parses`               | `tests/mixin_vtables.rs`   |
| B2        | `class_with_runtime_mixin_has_class_info_header`       | `tests/mixin_vtables.rs`   |
| B3        | `vtable_struct_emitted_per_implementor`                | `tests/mixin_vtables.rs`   |
| B4        | `class_init_populates_class_info_ptr`                  | `tests/mixin_vtables.rs`   |
| B5, B8    | `dynamic_dispatch_helper_resolves_concrete_method`     | `tests/mixin_vtables.rs`   |
| B6        | `direct_method_call_stays_static_dispatch`             | `tests/mixin_vtables.rs`   |
| B7        | `dyn_mixin_param_type_parses_and_typechecks`           | `tests/mixin_vtables.rs`   |
| B7 e2e    | `cases/770_heterogeneous_future_dispatch.rx`          | release-e2e                |
| B9 (next round) | n/a — depends on Drop migration                  | —                          |

---

## Implementation phases (sub-prompts)

This spec is big enough to stage:

1. **Phase A — surface + parser:** `dispatch runtime` modifier parses;
   `&Mixin` / `&var Mixin` types parse and typeck (B1, B7 syntax).
2. **Phase B — codegen:** vtable struct + class-info struct emission;
   class_info_ptr written at init time (B2, B3, B4, B8).
3. **Phase C — dispatch:** runtime helper + dyn-ref call site
   lowering (B5, B6).
4. **Phase D — Future opt-in:** `mixin Future` adds `dispatch runtime`;
   `Task.spawn` from sub-phase 5 now lands as runtime-only work
   (B7 e2e + sub-phase 5).

Each phase is a separate commit (or set thereof). Phase D unblocks
sub-phase 5.

## Out of scope (v2)

- **Multiple `dispatch runtime` mixins per class** (B8 handles it
  structurally but v1 ships with Future as the only one — single-
  slot class_info struct).
- **`dyn Mixin` upcasting / downcasting** — Rust's `dyn Trait as dyn
  OtherTrait` is fancy; v1 doesn't need it.
- **Mixin generics in runtime dispatch** — `dyn Iterator[T]` where T
  varies across implementors. v1 ships with `Future` whose Output is
  associated (already supported via mixin's `type Output`).
- **Drop / Display / Hash migration to runtime dispatch** — each is
  its own follow-up, gated on memory-layout review.

## Reserved error codes

- **E1117** — class includes a `dispatch runtime` mixin but doesn't
  implement all required methods.
- **E1118** — `&Mixin` / `&var Mixin` parameter type references a
  non-`dispatch runtime` mixin.
