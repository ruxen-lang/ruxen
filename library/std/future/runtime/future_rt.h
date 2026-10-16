/* future/runtime — package-internal header shared by the future runtime's
 * translation units (scheduler.c, schedule.c, stream.c, …). Include it
 * after runtime.h. Nothing outside library/std/future/runtime/ includes it.
 *
 *   - The two compiler-synthesised dispatch helpers the C side calls.
 *   - The Poll[T] tag values.
 *   - `ruxen_future_drop_and_free`, the one "run a future's `def drop`,
 *     then free its heap block" path (stream adapters, retry children,
 *     race losers).
 */

#ifndef RUXEN_FUTURE_RT_H
#define RUXEN_FUTURE_RT_H

/* Compiler-synthesised dynamic poll (docs/specs/types/mixin_vtables.spec.md
 * §B5; the MIR pass `synthesize_dynamic_dispatch_helpers` emits it when any
 * class includes `Future dispatch runtime`). i64 ABI: the future's heap
 * pointer and the Context's heap pointer in, the `Poll[T]` heap pointer
 * out. */
int64_t Future_dynamic_poll(int64_t self, int64_t ctx);

/* Compiler-synthesised `Drop_dynamic_drop(self)` (MIR
 * `build_drop_dynamic_helper`): routes a future's or stream's `def drop`
 * through the Drop vtable slot of its class_info, or a synthesised no-op
 * when the class declares no `def drop`. Declared weak so a program with
 * no async-protocol class at all (the helper is then never emitted) still
 * links; callers guard the NULL symbol. NULL means "no future in the
 * program", not "this future has no drop". */
__attribute__((weak)) void Drop_dynamic_drop(int64_t self);

/* Poll[T] heap layout (pinned by `poll_tag_layout_stability` in
 * compiler/ruxen_core/tests/async_surface.rs): a 16-byte block,
 * offset 0 the i32 tag, offset 8 the i64 payload (Ready only). */
#define RUXEN_POLL_READY_TAG   0
#define RUXEN_POLL_PENDING_TAG 1

/* Run the `def drop` of the future/stream at `ptr`, then free its heap
 * block. 0 (never set, or already consumed) is a no-op. The caller owns
 * `ptr` and must have cleared every other reference to it first: the
 * `def drop` is arbitrary user code and may re-enter the runtime. */
static inline void ruxen_future_drop_and_free(int64_t ptr) {
    if (ptr == 0) {
        return;
    }
    if (Drop_dynamic_drop) {
        Drop_dynamic_drop(ptr);
    }
    ruxen_dealloc((void *)(uintptr_t)ptr);
}

#endif /* RUXEN_FUTURE_RT_H */
