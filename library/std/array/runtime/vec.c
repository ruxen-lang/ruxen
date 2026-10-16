#include "../../core/runtime/runtime.h"

/* Vec (Array) runtime: the growable int64-slot array. String helpers
   live in string/runtime/string.c. A standalone translation unit. */

/* ── Vec Operations ───────────────────────────────────────────────── */

/* Vec struct body is hoisted near the top of the file (immediately
 * after the typedef forward decl) so earlier code — including the
 * Phase 2 #06 std.process.Command helpers that walk a Vec[String]
 * passed as `args` — can access `->len` / `->data[i]` directly. The
 * field layout `{ int64_t *data; uint64_t len; uint64_t cap; }`
 * matches the hoisted definition exactly. */

RuxenVec *ruxen_vec_new(void) {
    RuxenVec *v = (RuxenVec *)ruxen_xmalloc(sizeof(RuxenVec));
    v->data = NULL;
    v->len = 0;
    v->cap = 0;
    return v;
}

/* Vec spine free — see the "Heap-owned built-in drops" comment in the
   memory-management section above for the dual-name pattern. Releases
   only the data buffer and the outer struct; `ruxen_vec_drop` below
   frees the element heap first and then calls this. */
/* asm label declared at the top of the file alongside the matching
 * forward decl — see the `RUXEN_ASM_LABEL` block there. */
void ruxen_vec_ORIG_FREE(RuxenVec *v) {
    if (!v) return;
    if (v->data) free(v->data);
    free(v);
}

/* Internal: grow a Vec's capacity to at least `needed` slots.
   Kept available for future Vec operations even if `ruxen_vec_push`
   currently inlines its own grow logic. */
__attribute__((unused))
static void ruxen_vec_grow(RuxenVec *v, uint64_t needed) {
    uint64_t new_cap = v->cap == 0 ? 4 : v->cap * 2;
    while (new_cap < needed) {
        uint64_t doubled = new_cap * 2;
        if (doubled < new_cap) {
            ruxen_panic("vector capacity overflow");
        }
        new_cap = doubled;
    }
    size_t alloc_size;
    if (__builtin_mul_overflow(new_cap, sizeof(int64_t), &alloc_size)) {
        ruxen_panic("vector allocation size overflow");
    }
    int64_t *new_data = (int64_t *)realloc(v->data, alloc_size);
    if (!new_data) {
        ruxen_panic("out of memory");
    }
    v->data = new_data;
    v->cap = new_cap;
}

void ruxen_vec_push(RuxenVec *v, int64_t item) {
    if (!v) return;
    if (v->len >= v->cap) {
        uint64_t new_cap = v->cap == 0 ? 4 : v->cap * 2;
        /* Overflow check on capacity doubling */
        if (new_cap < v->cap) {
            ruxen_panic("vector capacity overflow");
        }
        /* Overflow check on allocation size */
        size_t alloc_size;
        if (__builtin_mul_overflow(new_cap, sizeof(int64_t), &alloc_size)) {
            ruxen_panic("vector allocation size overflow");
        }
        /* Preserve original pointer in case realloc fails */
        int64_t *new_data = (int64_t *)realloc(v->data, alloc_size);
        if (!new_data) {
            ruxen_panic("out of memory");
        }
        v->data = new_data;
        v->cap = new_cap;
    }
    v->data[v->len++] = item;
}

/* Pop the last element off a Vec, returning an Option tagged union:
   [tag:i32 pad:i32 payload:i64]. tag=0 → None, tag=1 → Some(value). */
void *ruxen_vec_pop(RuxenVec *v) {
    if (!v || v->len == 0) return ruxen_option_none();
    v->len -= 1;
    return ruxen_option_some(v->data[v->len]);
}

uint64_t ruxen_vec_len(RuxenVec *v) {
    return v ? v->len : 0;
}

int64_t ruxen_vec_get(RuxenVec *v, uint64_t index) {
    if (!v || index >= v->len) {
        ruxen_panic("index out of bounds");
    }
    return v->data[index];
}

/* get_opt: returns a proper Option tagged union (16 bytes):
   [tag: i32] [pad: i32] [payload: i64]
   tag=0 → None, tag=1 → Some(value) */
void *ruxen_vec_get_opt(RuxenVec *v, uint64_t index) {
    if (!v || index >= v->len) return ruxen_option_none();
    return ruxen_option_some(v->data[index]);
}

int8_t ruxen_vec_is_empty(RuxenVec *v) {
    return (!v || v->len == 0) ? 1 : 0;
}

void ruxen_vec_each(RuxenVec *v, void (*callback)(int64_t)) {
    /* In the v1 runtime, closures/blocks are not yet supported.
       Just iterate and call the callback if non-null. */
    if (!v || !callback) return;
    for (uint64_t i = 0; i < v->len; i++) {
        callback(v->data[i]);
    }
}

/* `Vec::count` — alias for length. Kept as a separate symbol so the
   codegen can dispatch without a special case. */
int64_t ruxen_vec_count(RuxenVec *v) {
    return v ? (int64_t)v->len : 0;
}

/* Reverse a Vec in place. Returns the same pointer for fluent chaining. */
RuxenVec *ruxen_vec_reverse(RuxenVec *v) {
    if (!v || v->len < 2) return v;
    uint64_t i = 0, j = v->len - 1;
    while (i < j) {
        int64_t tmp = v->data[i];
        v->data[i] = v->data[j];
        v->data[j] = tmp;
        i++;
        j--;
    }
    return v;
}

/* Vec::first / Vec::last — return Option-tagged 16-byte pair.
   Some(v) is tag=1, payload=v; None is tag=0. */
void *ruxen_vec_first(RuxenVec *v) {
    if (!v || v->len == 0) return ruxen_option_none();
    return ruxen_option_some(v->data[0]);
}

void *ruxen_vec_last(RuxenVec *v) {
    if (!v || v->len == 0) return ruxen_option_none();
    return ruxen_option_some(v->data[v->len - 1]);
}

/* Vec::clone — shallow copy: a new Vec with the same elements.
   Element types that own heap data (String, Class) keep aliasing
   that storage; v1 only guarantees structural duplication. */
RuxenVec *ruxen_vec_clone(RuxenVec *v) {
    RuxenVec *out = ruxen_vec_new();
    if (!v || v->len == 0) return out;
    for (uint64_t i = 0; i < v->len; i++) {
        ruxen_vec_push(out, v->data[i]);
    }
    return out;
}

/* `Vec::take(n)` — eager-materialise the first `n` elements as a
   fresh `RuxenVec*`. Phase 2 stdlib (#05 batch 2): we do NOT yet
   model lazy iterators; `vec.iter.take(n)` returns a copy, which
   keeps the chain trivially composable with downstream eager
   terminators (`sum`, `count`, etc.) without a per-call iterator
   struct. Bounds: `n` is clamped to `[0, len]`; a negative `n`
   yields an empty Vec. Element ownership is shallow-copied (same
   contract as `ruxen_vec_clone`). */
RuxenVec *ruxen_vec_take(RuxenVec *v, int64_t n) {
    RuxenVec *out = ruxen_vec_new();
    if (!v || n <= 0) return out;
    uint64_t bound = (uint64_t)n;
    if (bound > v->len) bound = v->len;
    for (uint64_t i = 0; i < bound; i++) {
        ruxen_vec_push(out, v->data[i]);
    }
    return out;
}

/* The descriptor stream that follows the subtree starting at `desc`'s
   low nibble. Used to find a HASH value's descriptor after its key's. */
static int64_t ruxen_elem_desc_skip(int64_t desc) {
    int64_t rest = RUXEN_ELEM_REST(desc);
    switch (RUXEN_ELEM_TAG(desc)) {
        case RUXEN_ELEM_VEC:
        case RUXEN_ELEM_SET:
            return ruxen_elem_desc_skip(rest);
        case RUXEN_ELEM_HASH:
            return ruxen_elem_desc_skip(ruxen_elem_desc_skip(rest));
        case RUXEN_ELEM_OBJ:
            /* The glue address is the rest of the word: nothing follows. */
            return 0;
        default:
            return rest;
    }
}

/* True when a slot described by `desc` owns heap, so a container drop
   has to walk its slots at all. */
static int ruxen_elem_owns_heap(int64_t desc) {
    switch (RUXEN_ELEM_TAG(desc)) {
        case RUXEN_ELEM_CSTR:
        case RUXEN_ELEM_VEC:
        case RUXEN_ELEM_HASH:
        case RUXEN_ELEM_SET:
        case RUXEN_ELEM_OBJ:
            return 1;
        default:
            return 0;
    }
}

/* See the OWNERSHIP CONTRACT on the declaration in
   core/runtime/runtime.h. Defined here because this TU owns RuxenVec. */
int64_t ruxen_elem_clone(int64_t slot, int64_t desc) {
    if (!slot) return 0;
    int64_t rest = RUXEN_ELEM_REST(desc);
    switch (RUXEN_ELEM_TAG(desc)) {
        case RUXEN_ELEM_CSTR:
            return (int64_t)(uintptr_t)ruxen_string_from((const char *)slot);
        case RUXEN_ELEM_VEC: {
            RuxenVec *src = (RuxenVec *)slot;
            RuxenVec *out = ruxen_vec_new();
            for (uint64_t i = 0; i < src->len; i++) {
                ruxen_vec_push(out, ruxen_elem_clone(src->data[i], rest));
            }
            return (int64_t)(uintptr_t)out;
        }
        case RUXEN_ELEM_HASH:
            return (int64_t)(uintptr_t)ruxen_hash_deep_clone(
                (RuxenHash *)slot, rest, ruxen_elem_desc_skip(rest));
        case RUXEN_ELEM_SET:
            /* A set is a bare RuxenHash header whose values are unused. */
            return (int64_t)(uintptr_t)ruxen_hash_deep_clone(
                (RuxenHash *)slot, rest, RUXEN_ELEM_POD);
        case RUXEN_ELEM_OBJ:
            return ((RuxenElemGlue)(uintptr_t)rest)(RUXEN_ELEM_GLUE_CLONE, slot);
        default:
            /* RUXEN_ELEM_POD and any pass-through key kind (e.g.
               RUXEN_KEY_CUSTOM): the slot is a value, copying the word
               IS the copy. Nothing is allocated and nothing is owned. */
            return slot;
    }
}

/* See the declaration in core/runtime/runtime.h. Frees through the
   `_ORIG_FREE` sentinels so the drop_fixtures leak harness counts every
   string and spine this releases. */
void ruxen_elem_drop(int64_t slot, int64_t desc) {
    if (!slot) return;
    int64_t rest = RUXEN_ELEM_REST(desc);
    switch (RUXEN_ELEM_TAG(desc)) {
        case RUXEN_ELEM_CSTR:
            ruxen_string_ORIG_FREE((char *)slot);
            return;
        case RUXEN_ELEM_VEC:
            ruxen_vec_drop((RuxenVec *)slot, rest);
            return;
        case RUXEN_ELEM_HASH:
            ruxen_hash_drop((RuxenHash *)slot, rest, ruxen_elem_desc_skip(rest));
            return;
        case RUXEN_ELEM_SET:
            ruxen_hash_drop((RuxenHash *)slot, rest, RUXEN_ELEM_POD);
            return;
        case RUXEN_ELEM_OBJ:
            ((RuxenElemGlue)(uintptr_t)rest)(RUXEN_ELEM_GLUE_DROP, slot);
            return;
        default:
            return;
    }
}

/* `Vec::skip(n)` — eager-materialise the tail starting from index `n`
   as a fresh `RuxenVec*`. Mirrors `ruxen_vec_take` but copies from
   `n..len`. `n >= len` yields an empty Vec; `n <= 0` yields a full
   clone.

   OWNERSHIP CONTRACT
     Allocates: the result spine + backing array, AND a fresh copy of
       every element whose `elem_kind` says the slot owns heap.
     Frees: nothing. `v` is BORROWED and is completely unchanged and
       still fully owned by its own scope when this returns.
     Caller may assume: the result is INDEPENDENT of `v`. Dropping
       either one deep-frees only its own elements, in any order, and
       the survivor stays intact. This is what lets `ruxen_vec_skip_elem`
       be a Fresh callee (see FRESH_ALLOC_CALLEES).

   The 2-argument `ruxen_vec_skip` is the POD-only spelling kept for the
   `.rx` FFI declaration's arity; MIR rewrites it to this form with the
   element kind injected from the static element type. It forwards with
   RUXEN_ELEM_POD, which is correct for a value element and is the
   pre-existing behaviour for every non-heap `T`. */
RuxenVec *ruxen_vec_skip_elem(RuxenVec *v, int64_t n, int64_t elem_kind) {
    RuxenVec *out = ruxen_vec_new();
    if (!v) return out;
    uint64_t start = n <= 0 ? 0 : (uint64_t)n;
    if (start >= v->len) return out;
    for (uint64_t i = start; i < v->len; i++) {
        ruxen_vec_push(out, ruxen_elem_clone(v->data[i], elem_kind));
    }
    return out;
}

RuxenVec *ruxen_vec_skip(RuxenVec *v, int64_t n) {
    return ruxen_vec_skip_elem(v, n, RUXEN_ELEM_POD);
}

/* `Iter::chain(other)` — eager-materialise the concatenation of two
   iterators as a fresh `RuxenVec*`. Phase 2 stdlib (#05 batch 3): we
   keep the v1 "every iter is a Vec" invariant; chain copies each
   source's slots into a new Vec so downstream terminators (`sum`,
   `count`, `fold`, …) keep using the same `RuxenVec*` shape. Element
   ownership is shallow-copied — the same contract as
   `ruxen_vec_clone` / `ruxen_vec_take`.

   OWNERSHIP CONTRACT — identical to `ruxen_vec_skip_elem`: both sources
   are BORROWED and unchanged, every heap element is copied per
   `elem_kind`, and the result is independent of both. The two source
   iters remain owned by their respective scope frames and may be
   dropped before or after the result, in any order.

   As with skip, the 2-argument spelling is the POD-only form kept for
   the `.rx` FFI declaration's arity; MIR rewrites it with the element
   kind injected from the static element type. */
RuxenVec *ruxen_vec_chain_elem(RuxenVec *a, RuxenVec *b, int64_t elem_kind) {
    RuxenVec *out = ruxen_vec_new();
    if (a) {
        for (uint64_t i = 0; i < a->len; i++) {
            ruxen_vec_push(out, ruxen_elem_clone(a->data[i], elem_kind));
        }
    }
    if (b) {
        for (uint64_t i = 0; i < b->len; i++) {
            ruxen_vec_push(out, ruxen_elem_clone(b->data[i], elem_kind));
        }
    }
    return out;
}

RuxenVec *ruxen_vec_chain(RuxenVec *a, RuxenVec *b) {
    return ruxen_vec_chain_elem(a, b, RUXEN_ELEM_POD);
}

/* `Iter::zip(other)` — eager-materialise pairs as a fresh
   `RuxenVec*`, where each slot holds a pointer to a freshly
   allocated 2-tuple `{a[i], b[i]}` (16 bytes, layout: field0 at +0,
   field1 at +8). Stops at the shorter source's length. The tuple
   payloads are shallow-copied — for `Vec[Int]`/`Vec[&str]` this is a
   plain bit copy; for `Vec[String]`/`Vec[Vec[_]]` the heap
   ownership is aliased (same shallow-copy contract as `clone`).

   v1 ships eager-materialisation rather than a lazy `ZipIter`
   struct; once trait-druxen dispatch lands (#09) the inner cell can
   become a tagged-pair iterator over two cursors. */
RuxenVec *ruxen_vec_zip(RuxenVec *a, RuxenVec *b) {
    RuxenVec *out = ruxen_vec_new();
    if (!a || !b) return out;
    uint64_t bound = a->len < b->len ? a->len : b->len;
    for (uint64_t i = 0; i < bound; i++) {
        int64_t *pair = (int64_t *)ruxen_alloc(16);
        pair[0] = a->data[i];
        pair[1] = b->data[i];
        ruxen_vec_push(out, (int64_t)pair);
    }
    return out;
}

/* ── Vec[T] surface — Phase 2 stdlib batch 1 (#03) ──────────────────
 * Constructors / inspectors / mutators that the v1 surface promises
 * but the runtime did not yet implement. All operate on the same
 * 64-bit-slot element layout; the type system enforces correctness
 * upstream so e.g. `ruxen_vec_swap` on Vec[String] is safe — the
 * slot bits are pointer-typed but bitwise-swappable.
 */

/* Vec.with_capacity(cap) — pre-allocate the backing array. */
RuxenVec *ruxen_vec_with_capacity(uint64_t cap) {
    RuxenVec *v = ruxen_vec_new();
    if (cap == 0) return v;
    size_t alloc_size;
    if (__builtin_mul_overflow(cap, sizeof(int64_t), &alloc_size)) {
        ruxen_panic("vector allocation size overflow");
    }
    int64_t *data = (int64_t *)ruxen_xmalloc(alloc_size);
    v->data = data;
    v->cap = cap;
    return v;
}

/* Vec.capacity — total slot count of the backing array. */
uint64_t ruxen_vec_capacity(RuxenVec *v) {
    return v ? v->cap : 0;
}

/* Vec.clear — reset length to zero without freeing the backing
   array. Element-owned heap (e.g. Vec[String]) is NOT walked here;
   the v1 contract is that `clear` is a *bulk forget* — callers who
   need element-drop should use `truncate(0)` once that helper learns
   per-type drop, or drain explicitly. */
void ruxen_vec_clear(RuxenVec *v) {
    if (!v) return;
    v->len = 0;
}

/* Vec.truncate(n) — drop trailing elements past index `n`. No-op if
   `n >= len`. */
void ruxen_vec_truncate(RuxenVec *v, uint64_t n) {
    if (!v) return;
    if (n < v->len) {
        v->len = n;
    }
}

/* Vec.swap(i, j) — swap two elements in place. Panics on OOB. */
void ruxen_vec_swap(RuxenVec *v, uint64_t i, uint64_t j) {
    if (!v || i >= v->len || j >= v->len) {
        ruxen_panic("vec swap: index out of bounds");
    }
    if (i == j) return;
    int64_t tmp = v->data[i];
    v->data[i] = v->data[j];
    v->data[j] = tmp;
}

/* Vec.insert(i, item) — shift elements at >= i one slot right and
   place `item` at position `i`. Panics if i > len. */
void ruxen_vec_insert(RuxenVec *v, uint64_t i, int64_t item) {
    if (!v) {
        ruxen_panic("vec insert: null receiver");
    }
    if (i > v->len) {
        ruxen_panic("vec insert: index out of bounds");
    }
    /* Reuse push's grow path by appending then rotating. */
    ruxen_vec_push(v, 0);
    for (uint64_t k = v->len - 1; k > i; k--) {
        v->data[k] = v->data[k - 1];
    }
    v->data[i] = item;
}

/* Vec.remove(i) — remove and return the element at index `i`,
   shifting subsequent elements one slot left. Panics on OOB. */
int64_t ruxen_vec_remove(RuxenVec *v, uint64_t i) {
    if (!v || i >= v->len) {
        ruxen_panic("vec remove: index out of bounds");
    }
    int64_t out = v->data[i];
    for (uint64_t k = i; k + 1 < v->len; k++) {
        v->data[k] = v->data[k + 1];
    }
    v->len -= 1;
    return out;
}

/* Vec.extend(other) — append every element of `other` to `self`.
   The element slots are copied bitwise; for Vec[String] this means
   the destination aliases the source's owned strings. The caller is
   responsible for ensuring `other` is forgotten/cleared before drop
   to avoid double-free; v1 documents this as a known sharp edge
   for heap-element types and the borrow checker treats `extend`'s
   second arg as a borrow only — full move semantics land in 05. */
void ruxen_vec_extend(RuxenVec *v, RuxenVec *other) {
    if (!v || !other) return;
    for (uint64_t k = 0; k < other->len; k++) {
        ruxen_vec_push(v, other->data[k]);
    }
}

/* Vec[i] — panicking indexed read used by IndexOp lowering for Vec
   receivers. Mirrors Rust's `v[i]`: OOB → panic with a descriptive
   message including both `i` and `len`. */
int64_t ruxen_vec_get_or_panic(RuxenVec *v, uint64_t i) {
    if (!v || i >= (v ? v->len : 0)) {
#if defined(__wasm32__)
        /* wasm has no snprintf and panic ignores the message (it traps), so a
         * static string suffices (tier 4.09). */
        ruxen_panic("Vec index out of range");
#else
        char buf[96];
        uint64_t len = v ? v->len : 0;
        snprintf(buf, sizeof(buf), "index %llu out of range, len %llu",
                 (unsigned long long)i, (unsigned long long)len);
        ruxen_panic(buf);
#endif
    }
    return v->data[i];
}

/* `Array[T]` drop glue: the one drop callee the MIR drop pass emits for
   every `Array[T]` local. `elem_desc` is the element descriptor the
   compiler built from `T` (see runtime.h); each slot is freed per that
   descriptor, recursively, before the spine.

   NOTE on free-helper call sites in this file: the drop_fixtures test
   harness textually rewrites every `free(` substring to
   `ruxen_test_free(`. To call `ruxen_string_free` / `ruxen_vec_free`
   from C without losing them to that rewrite we go through the
   `ORIG_FREE(` sentinel — see the long comment above
   `ruxen_string_ORIG_FREE`. In the production build the sentinel
   resolves directly to the canonical helper via the asm-label
   declarations; under the leak-tracking splice it is restored to
   `free(` and folded into the wrapper that bumps the per-kind
   counters. */
void ruxen_vec_drop(RuxenVec *v, int64_t elem_desc) {
    if (!v) return;
    if (ruxen_elem_owns_heap(elem_desc)) {
        for (uint64_t i = 0; i < v->len; i++) {
            ruxen_elem_drop(v->data[i], elem_desc);
            v->data[i] = 0;
        }
    }
    ruxen_vec_ORIG_FREE(v);
}

/* ── Vec[T] surface — Phase 2 stdlib batch 2 (#03) ──────────────────
 * `from_iter`, `dedup`, plus the consume-style `into_iter` whose
 * runtime side is identity (the v1 iterator representation IS a
 * RuxenVec*, so `iter` / `into_iter` / `iter_mut` are all the same
 * passthrough at the C layer). The closure-takers `sort_by` and
 * `retain` are inlined at the MIR layer and do not appear here.
 */

/* Vec.from_iter(iter) — currently identity passthrough since every
   "iterator" in the v1 runtime is already a RuxenVec*. The MIR-level
   drop-elaboration treats this as a fresh allocation (see
   FRESH_ALLOC_CALLEES in mir/lower.rs) so the destination local
   inherits the spine-free responsibility. */
RuxenVec *ruxen_vec_from_iter(RuxenVec *iter) {
    /* The source iter is consumed (its Vec spine is what the new Vec
       owns now). The drop pass already taints the source local via
       the consume-helper match. Nothing else to do. */
    return iter;
}

/* Vec slot store — used by the inlined `retain` lowering to compact
   surviving elements into the prefix. Panics on OOB so a buggy
   inliner (or a future user-callable use that escapes) doesn't
   silently corrupt out-of-bounds memory. */
void ruxen_vec_set(RuxenVec *v, uint64_t index, int64_t value) {
    if (!v || index >= v->len) {
        ruxen_panic("vec set: index out of bounds");
    }
    v->data[index] = value;
}

/* The `ruxen_iter_to_vec` identity collector was removed with the
 * orphaned iterator machinery (zero-Rust-stdlib migration, Phase B /
 * Milestone 2): nothing produced a `*Iter` wrapper, and no surface call
 * (`.iter`/`.into_iter`/`.to_vec`/`.enumerate`/`.as_slice`) reached it.
 * The Ruby collectors `ruxen_string_from_iter` / `ruxen_set_from_iter` /
 * `ruxen_hash_from_iter` are unrelated and preserved. */

