#include "../../core/runtime/runtime.h"

/* ── Hash Operations ──────────────────────────────────────────────── */

/* Simple Hash: array of bucket linked lists, keyed by uintptr_t.
   Keys and values are both pointer-sized (stored as int64_t).
   Chained collisions handled via next pointer.

   `RuxenHashEntry` and `struct RuxenHash` live in core/runtime/runtime.h
   — json.c walks the bucket chains too, and a per-TU copy of the layout
   is an ODR hazard. The key descriptor (B73) is documented there. */

#define RUXEN_HASH_INITIAL_BUCKETS 16u

static uint64_t ruxen_hash_bits(int64_t k) {
    /* splitmix64-ish finalizer for decent distribution on raw int bits. */
    uint64_t x = (uint64_t)k;
    x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
    x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
    x = x ^ (x >> 31);
    return x;
}

uint64_t ruxen_hash_str(const char *s) {
    /* FNV-1a on the byte contents for string-keyed hashes. */
    uint64_t h = 1469598103934665603ULL;
    if (!s) return h;
    while (*s) {
        h ^= (uint8_t)(*s);
        h *= 1099511628211ULL;
        s++;
    }
    return h;
}

/* The descriptor a key hashes and compares by: a borrowed key's is what
   it borrows. */
static int64_t ruxen_key_identity(int64_t desc) {
    while (desc != RUXEN_ELEM_UNSET && RUXEN_ELEM_TAG(desc) == RUXEN_ELEM_BORROW) {
        desc = RUXEN_ELEM_REST(desc);
    }
    return desc;
}

/* Key equality, by the table's key descriptor. Never dereferences a key
   unless the descriptor says the key IS a pointer. */
static int ruxen_hash_keys_equal(const RuxenHash *h, int64_t a, int64_t b) {
    /* Identical words are equal under every descriptor: same string
       pointer, same integer, same object. This also keeps a lookup on a
       table with no descriptor yet (an empty table: every path that adds
       an entry installs one) from touching memory, which is what fixed the
       empty-hash `key?`/`get` SIGSEGV (Q25). */
    if (a == b) return 1;
    if (!h) return 0;
    int64_t desc = ruxen_key_identity(h->key_desc);
    if (desc == RUXEN_ELEM_UNSET) return 0;
    switch (RUXEN_ELEM_TAG(desc)) {
    case RUXEN_ELEM_CSTR: {
        const char *sa = (const char *)a;
        const char *sb = (const char *)b;
        if (!sa || !sb) return 0;
        return strcmp(sa, sb) == 0;
    }
    case RUXEN_ELEM_OBJ:
        if (!a || !b) return 0;
        return ((RuxenElemGlue)(uintptr_t)RUXEN_ELEM_REST(desc))(
                   RUXEN_ELEM_GLUE_EQ, a, b) != 0;
    default:
        /* Every other key is its word: the `a == b` test above is the
           whole relation. */
        return 0;
    }
}

static uint64_t ruxen_hash_key_hash(const RuxenHash *h, int64_t key) {
    if (!h) return ruxen_hash_bits(key);
    int64_t desc = ruxen_key_identity(h->key_desc);
    if (desc == RUXEN_ELEM_UNSET) return ruxen_hash_bits(key);
    switch (RUXEN_ELEM_TAG(desc)) {
    case RUXEN_ELEM_CSTR:
        return ruxen_hash_str((const char *)key);
    case RUXEN_ELEM_OBJ:
        if (!key) return ruxen_hash_bits(key);
        /* Run the type's hash through the bit finalizer as well: a
           hand-written `hash_code` is free to return something with poor
           low-bit entropy (an id, a small counter), and the bucket index
           is a raw modulo. */
        return ruxen_hash_bits(((RuxenElemGlue)(uintptr_t)RUXEN_ELEM_REST(desc))(
            RUXEN_ELEM_GLUE_HASH, key, 0));
    default:
        return ruxen_hash_bits(key);
    }
}

/* Shared field init for a freshly-malloc'd table header. Used by both
   the Hash and the Set constructors (a Set is a Hash whose values are
   ignored), so the key descriptor is installed in exactly one place. */
static void ruxen_hash_init_fields(RuxenHash *h, int64_t key_desc) {
    h->bucket_count = RUXEN_HASH_INITIAL_BUCKETS;
    /* Use malloc + explicit NULL init rather than calloc so the
       drop_fixtures leak harness (which rewrites `malloc(` →
       `ruxen_test_malloc(` and `free(` → `ruxen_test_free(`) keeps
       the per-allocation counters balanced. calloc is not rewritten,
       so a calloc'd buckets array would free-count without an
       alloc-count and underflow the raw_outstanding counter. */
    h->buckets = (RuxenHashEntry **)ruxen_xmalloc(
        h->bucket_count * sizeof(RuxenHashEntry *));
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        h->buckets[i] = NULL;
    }
    h->len = 0;
    h->key_desc = key_desc;
}

static RuxenHash *ruxen_hash_alloc(int64_t key_desc) {
    RuxenHash *h = (RuxenHash *)ruxen_xmalloc(sizeof(RuxenHash));
    ruxen_hash_init_fields(h, key_desc);
    return h;
}

/* `Hash.new`: `key_desc` comes from the `.rx` declaration's `KeyDesc[K]`
   parameter, which the compiler fills from the static key type. It may be
   RUXEN_ELEM_UNSET only from C (see `ruxen_hash_install`). */
RuxenHash *ruxen_hash_new(int64_t key_desc) {
    return ruxen_hash_alloc(key_desc);
}

/* Install `key_desc` on a table that has none yet. A no-op once the table
   has one, so it can never override what construction chose. */
static void ruxen_hash_install(RuxenHash *h, int64_t key_desc) {
    if (h->key_desc == RUXEN_ELEM_UNSET) h->key_desc = key_desc;
}

/* The descriptor a table's keys are freed and copied by: its own, or the
   caller's for a table that never got one (it is empty). */
static int64_t ruxen_hash_own_desc(const RuxenHash *h, int64_t fallback) {
    return h->key_desc == RUXEN_ELEM_UNSET ? fallback : h->key_desc;
}

/* Hash drop — see the "Heap-owned built-in drops" comment in the
   memory-management section above for the dual-name pattern. The
   spine-only variant frees the bucket chains and the struct itself;
   `ruxen_hash_drop` below walks K/V slots before delegating
   here. */
void ruxen_hash_ORIG_FREE(RuxenHash *h) RUXEN_ASM_LABEL(ruxen_hash_free);
void ruxen_hash_ORIG_FREE(RuxenHash *h) {
    if (!h) return;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        RuxenHashEntry *e = h->buckets[i];
        while (e) {
            RuxenHashEntry *nx = e->next;
            free(e);
            e = nx;
        }
    }
    free(h->buckets);
    free(h);
}

/* `Hash[K, V]` drop glue: the one drop callee the MIR drop pass emits
 * for every `Hash[K, V]` local. `val_desc` is the element descriptor the
 * compiler built from V (see runtime.h); every entry's value is freed per
 * it, and every key per the table's own key descriptor (`key_desc` only
 * for a table that has none), recursively, before the spine. */
void ruxen_hash_drop(RuxenHash *h, int64_t key_desc, int64_t val_desc) {
    if (!h) return;
    key_desc = ruxen_hash_own_desc(h, key_desc);
    if (RUXEN_ELEM_TAG(key_desc) != RUXEN_ELEM_POD ||
        RUXEN_ELEM_TAG(val_desc) != RUXEN_ELEM_POD) {
        for (uint64_t i = 0; i < h->bucket_count; i++) {
            for (RuxenHashEntry *e = h->buckets[i]; e; e = e->next) {
                ruxen_elem_drop(e->key, key_desc);
                ruxen_elem_drop(e->value, val_desc);
                e->key = 0;
                e->value = 0;
            }
        }
    }
    ruxen_hash_ORIG_FREE(h);
}

/* See the declaration in core/runtime/runtime.h. The copy keeps `h`'s key
 * descriptor, installed before the first insert so the copied entries
 * land in the same buckets, and copies keys by it. */
RuxenHash *ruxen_hash_deep_clone(RuxenHash *h, int64_t key_desc,
                                 int64_t val_desc) {
    if (!h) return ruxen_hash_new(key_desc);
    key_desc = ruxen_hash_own_desc(h, key_desc);
    RuxenHash *out = ruxen_hash_alloc(h->key_desc);
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RuxenHashEntry *e = h->buckets[i]; e; e = e->next) {
            ruxen_hash_insert(out, ruxen_elem_clone(e->key, key_desc),
                              ruxen_elem_clone(e->value, val_desc), key_desc);
        }
    }
    return out;
}

/* Double the bucket array and rehash every entry into the new
   spine. Called from `ruxen_hash_insert` when an additional entry
   would push the load factor past 0.75. We splice the existing
   RuxenHashEntry nodes into the new buckets in place — no
   per-entry malloc/free, so a rehash costs one buckets-array
   alloc + one buckets-array free regardless of how many entries
   we relink. The leak harness counts only `free(` calls, and we
   add one matched malloc/free pair for the buckets spine, so
   counters stay balanced. */
static void ruxen_hash_maybe_grow(RuxenHash *h) {
    /* Load factor 0.75 — grow when len+1 would exceed
       bucket_count * 3 / 4. Integer-math equivalent below. */
    if (h->len + 1 <= (h->bucket_count * 3) / 4) return;

    uint64_t old_n = h->bucket_count;
    uint64_t new_n = old_n * 2;
    /* malloc + NULL-init for leak-harness counter parity — see the
       comment in ruxen_hash_new. */
    RuxenHashEntry **nb =
        (RuxenHashEntry **)ruxen_xmalloc(new_n * sizeof(RuxenHashEntry *));
    for (uint64_t j = 0; j < new_n; j++) {
        nb[j] = NULL;
    }
    for (uint64_t i = 0; i < old_n; i++) {
        RuxenHashEntry *e = h->buckets[i];
        while (e) {
            RuxenHashEntry *nx = e->next;
            uint64_t bj = ruxen_hash_key_hash(h, e->key) % new_n;
            e->next = nb[bj];
            nb[bj] = e;
            e = nx;
        }
    }
    free(h->buckets);
    h->buckets = nb;
    h->bucket_count = new_n;
}

/* `insert`: `key_desc` comes from the `.rx` declaration's `KeyDesc[K]`
   parameter and is installed on a table that has none. The table owns
   `key` from here on: a key already present keeps its entry, and the
   duplicate is freed by the key descriptor. */
void ruxen_hash_insert(RuxenHash *h, int64_t key, int64_t value, int64_t key_desc) {
    if (!h) return;
    ruxen_hash_install(h, key_desc);
    if (h->key_desc == RUXEN_ELEM_UNSET) {
        /* No key descriptor, so we do not know how to hash or compare this
           key. Populating the table anyway would give silently wrong
           lookups (the pre-B73 failure mode). Fail loudly and name the
           contract instead. */
        ruxen_panic("hash insert: no key descriptor for this table (B73)");
    }
    uint64_t bucket_idx = ruxen_hash_key_hash(h, key) % h->bucket_count;
    RuxenHashEntry *e = h->buckets[bucket_idx];
    while (e) {
        if (ruxen_hash_keys_equal(h, e->key, key)) {
            e->value = value;
            if (e->key != key) ruxen_elem_drop(key, h->key_desc);
            return;
        }
        e = e->next;
    }
    /* Grow before linking the new entry. Grow recomputes the bucket
       index, so we re-read after. */
    ruxen_hash_maybe_grow(h);
    bucket_idx = ruxen_hash_key_hash(h, key) % h->bucket_count;
    RuxenHashEntry *ne = (RuxenHashEntry *)ruxen_xmalloc(sizeof(RuxenHashEntry));
    ne->key = key;
    ne->value = value;
    ne->next = h->buckets[bucket_idx];
    h->buckets[bucket_idx] = ne;
    h->len += 1;
}

/* Return an Option tagged union (16 bytes): tag=1 Some(&value), tag=0 None.
   The payload carries the raw value (v1 treats &V the same as V at the
   runtime level — both are 8 bytes). */
void *ruxen_hash_get(RuxenHash *h, int64_t key) {
    if (!h) return ruxen_option_none();
    uint64_t bucket_idx = ruxen_hash_key_hash(h, key) % h->bucket_count;
    RuxenHashEntry *e = h->buckets[bucket_idx];
    while (e) {
        if (ruxen_hash_keys_equal(h, e->key, key)) {
            return ruxen_option_some(e->value);
        }
        e = e->next;
    }
    return ruxen_option_none();
}

int8_t ruxen_hash_contains_key(RuxenHash *h, int64_t key) {
    if (!h) return 0;
    uint64_t bucket_idx = ruxen_hash_key_hash(h, key) % h->bucket_count;
    RuxenHashEntry *e = h->buckets[bucket_idx];
    while (e) {
        if (ruxen_hash_keys_equal(h, e->key, key)) {
            return 1;
        }
        e = e->next;
    }
    return 0;
}

uint64_t ruxen_hash_len(RuxenHash *h) {
    return h ? h->len : 0;
}

int8_t ruxen_hash_is_empty(RuxenHash *h) {
    return (!h || h->len == 0) ? 1 : 0;
}

/* ── Set Operations ───────────────────────────────────────────────── */

/* Built on top of the Hash — values are unused (set to 1). A
   `RuxenSet` is exactly its `RuxenHash` header, so the spine work
   (allocation, clone, len, free) is the Hash routine applied to
   `&s->inner`; only element-level behaviour lives here. */

typedef struct {
    RuxenHash inner;
} RuxenSet;

_Static_assert(sizeof(RuxenSet) == sizeof(RuxenHash),
    "RuxenSet must stay a bare RuxenHash header");

static RuxenSet *ruxen_set_alloc(int64_t key_desc) {
    return (RuxenSet *)ruxen_hash_alloc(key_desc);
}

/* `Set.new`: a Set's ELEMENT is its key, so `key_desc` (the `.rx`
   declaration's `KeyDesc[T]`) describes the elements. */
RuxenSet *ruxen_set_new(int64_t key_desc) {
    return ruxen_set_alloc(key_desc);
}

/* `insert`: see `ruxen_hash_insert`. The set owns `item`; a duplicate is
   freed. The value word is 1 (unused). */
void ruxen_set_insert(RuxenSet *s, int64_t item, int64_t key_desc) {
    if (!s) return;
    ruxen_hash_insert(&s->inner, item, 1, key_desc);
}

/* Insert a COPY of `item` into `s`, deep-copied per `s`'s key descriptor.
 *
 * OWNERSHIP CONTRACT
 *   Allocates: one element copy, per `ruxen_elem_clone`, when the key
 *     descriptor says the slot owns heap. Nothing otherwise.
 *   Frees: nothing. `item` stays owned by the SOURCE collection.
 *   Caller may assume: `s` now owns its copy outright, so `s` and the
 *     source can be deep-dropped independently and in any order.
 *
 * This is the derive-side counterpart to `ruxen_set_insert`, which
 * TRANSFERS its argument (arg_transfer_mask marks slot 1) and is what a
 * user-level `s.insert(x)` lowers to. The two must not be confused: a
 * derive that used the transferring form would alias, and a user insert
 * that used this one would leak the caller's value. `s` has a key
 * descriptor: it is the result of a constructor or copies its source's. */
static void ruxen_set_insert_owned(RuxenSet *s, int64_t item) {
    if (!s || ruxen_hash_contains_key(&s->inner, item)) return;
    ruxen_hash_insert(&s->inner, ruxen_elem_clone(item, s->inner.key_desc), 1,
                      s->inner.key_desc);
}

int8_t ruxen_set_contains(RuxenSet *s, int64_t item) {
    if (!s) return 0;
    return ruxen_hash_contains_key(&s->inner, item);
}

uint64_t ruxen_set_len(RuxenSet *s) {
    return ruxen_hash_len(s ? &s->inner : NULL);
}

int8_t ruxen_set_is_empty(RuxenSet *s) {
    return ruxen_hash_is_empty(s ? &s->inner : NULL);
}

/* Set spine drop — Phase 2 stdlib (#04 batch 2). The spine is a Hash
   spine, so this is `ruxen_hash_ORIG_FREE` on the header. The element
   heap (heap-owned T) is freed by `ruxen_set_drop` below before
   delegating here. */
void ruxen_set_ORIG_FREE(RuxenSet *s) RUXEN_ASM_LABEL(ruxen_set_free);
void ruxen_set_ORIG_FREE(RuxenSet *s) {
    if (!s) return;
    ruxen_hash_ORIG_FREE(&s->inner);
}

/* `Set[T]` drop glue: the one drop callee the MIR drop pass emits for
 * every `Set[T]` local. An element is its key, freed by the set's key
 * descriptor (`elem_desc` only for a set that has none, see
 * `ruxen_hash_drop`) before the spine. */
void ruxen_set_drop(RuxenSet *s, int64_t elem_desc) {
    if (!s) return;
    elem_desc = ruxen_hash_own_desc(&s->inner, elem_desc);
    if (RUXEN_ELEM_TAG(elem_desc) != RUXEN_ELEM_POD) {
        for (uint64_t i = 0; i < s->inner.bucket_count; i++) {
            for (RuxenHashEntry *e = s->inner.buckets[i]; e; e = e->next) {
                ruxen_elem_drop(e->key, elem_desc);
                e->key = 0;
            }
        }
    }
    ruxen_set_ORIG_FREE(s);
}

/* ── HashMap[K,V] surface — Phase 2 stdlib (#04) ──────────────────── */

/* HashMap.with_capacity(Int) — capacity is an advisory hint in v1.
   The chained-bucket implementation has fixed bucket count, so this
   function returns a fresh empty map identical to `ruxen_hash_new`.
   A future open-addressing rewrite would use the hint to size the
   initial bucket array. */
RuxenHash *ruxen_hash_with_capacity(int64_t cap, int64_t key_desc) {
    (void)cap;
    return ruxen_hash_new(key_desc);
}

/* HashMap.remove(&K) — return Option[V] (16-byte tagged union).
   tag=1 Some(prior_value), tag=0 None. Walks the bucket chain and
   unlinks the matching entry; the freed RuxenHashEntry must go
   through `ruxen_test_free` under the leak harness, which is what
   plain `free(` rewrites to. The bucket-walk emits an explicit
   `free(e)` so the entry counter increments correctly. */
void *ruxen_hash_remove(RuxenHash *h, int64_t key) {
    if (!h) return ruxen_option_none();
    uint64_t bucket_idx = ruxen_hash_key_hash(h, key) % h->bucket_count;
    RuxenHashEntry *prev = NULL;
    RuxenHashEntry *e = h->buckets[bucket_idx];
    while (e) {
        if (ruxen_hash_keys_equal(h, e->key, key)) {
            int64_t value = e->value;
            if (prev) {
                prev->next = e->next;
            } else {
                h->buckets[bucket_idx] = e->next;
            }
            /* The table owned the key; the value moves to the caller. */
            ruxen_elem_drop(e->key, h->key_desc);
            free(e);
            h->len -= 1;
            return ruxen_option_some(value);
        }
        prev = e;
        e = e->next;
    }
    return ruxen_option_none();
}

/* HashMap.clear — remove every entry. Frees per-bucket entry chains
   (and the keys, which the table owns) then resets `len` to 0. The spine
   is preserved, and so is the key descriptor: emptying a `Hash[String, V]` does not make it a
   `Hash[Int, V]`. (Pre-B73 this reset the tristate flag, so the table
   re-guessed its key kind from whatever was inserted next.) */
void ruxen_hash_clear(RuxenHash *h) {
    if (!h) return;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        RuxenHashEntry *e = h->buckets[i];
        while (e) {
            RuxenHashEntry *nx = e->next;
            /* The table owned the key. */
            ruxen_elem_drop(e->key, h->key_desc);
            free(e);
            e = nx;
        }
        h->buckets[i] = NULL;
    }
    h->len = 0;
}

/* HashMap.keys -> Vec[&K]
   Returns a freshly-allocated Vec[K] containing each key. v1 does
   not distinguish &K from K at the runtime layer (both 8 bytes),
   so this is a flat slot copy. Iteration order matches bucket
   traversal order — callers must not rely on it (per prompt §44). */
RuxenVec *ruxen_hash_keys(RuxenHash *h) {
    RuxenVec *out = ruxen_vec_new();
    if (!h) return out;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RuxenHashEntry *e = h->buckets[i]; e; e = e->next) {
            ruxen_vec_push(out, e->key);
        }
    }
    return out;
}

/* HashMap.values -> Vec[&V] — symmetric to `keys`. */
RuxenVec *ruxen_hash_values(RuxenHash *h) {
    RuxenVec *out = ruxen_vec_new();
    if (!h) return out;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RuxenHashEntry *e = h->buckets[i]; e; e = e->next) {
            ruxen_vec_push(out, e->value);
        }
    }
    return out;
}

/* HashMap.to_a -> Vec[(K, V)] — Ruby's `hash.to_a` returns key/value
   pairs. Each slot is a freshly-allocated 16-byte 2-tuple {key, value}
   (field0 at +0, field1 at +8), matching `ruxen_vec_zip`'s tuple layout
   and `ruxen_hash_from_iter`'s pair decode. Payloads are shallow-copied
   (aliased heap for String values, same contract as zip). Bucket-
   traversal order; callers must not rely on it (per prompt §44). */
RuxenVec *ruxen_hash_entries(RuxenHash *h) {
    RuxenVec *out = ruxen_vec_new();
    if (!h) return out;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RuxenHashEntry *e = h->buckets[i]; e; e = e->next) {
            int64_t *pair = (int64_t *)ruxen_alloc(16);
            pair[0] = e->key;
            pair[1] = e->value;
            ruxen_vec_push(out, (int64_t)pair);
        }
    }
    return out;
}

/* HashMap[&K] — panicking indexed read used by IndexOp lowering for
   HashMap receivers. Mirrors `ruxen_vec_get_or_panic`: missing key
   triggers a ruxen panic. Returns the value slot directly (raw 8
   bytes), not an Option. */
int64_t ruxen_hash_index(RuxenHash *h, int64_t key) {
    if (!h) {
        ruxen_panic("hashmap index: missing key");
    }
    uint64_t bucket_idx = ruxen_hash_key_hash(h, key) % h->bucket_count;
    for (RuxenHashEntry *e = h->buckets[bucket_idx]; e; e = e->next) {
        if (ruxen_hash_keys_equal(h, e->key, key)) {
            return e->value;
        }
    }
    ruxen_panic("hashmap index: missing key");
    return 0; /* unreachable */
}

/* ── HashSet[T] surface — Phase 2 stdlib (#04) ──────────────────── */

/* HashSet.with_capacity(Int) — capacity hint, see HashMap.with_capacity. */
RuxenSet *ruxen_set_with_capacity(int64_t cap, int64_t key_desc) {
    (void)cap;
    return ruxen_set_new(key_desc);
}

/* HashSet.remove(&T) -> Bool — true iff the element was present.
   Reuses `ruxen_hash_remove` for the unlink work, then collapses the
   resulting Option to a Bool via the tag word. */
int8_t ruxen_set_remove(RuxenSet *s, int64_t item) {
    if (!s) return 0;
    void *opt = ruxen_hash_remove(&s->inner, item);
    int8_t was_present = (*(int32_t *)opt) == 1 ? 1 : 0;
    ruxen_dealloc(opt);
    return was_present;
}

/* HashSet.clear — release every entry, reset len, mirror
   `ruxen_hash_clear`. */
void ruxen_set_clear(RuxenSet *s) {
    if (!s) return;
    ruxen_hash_clear(&s->inner);
}

/* HashSet.iter -> Vec[&T] — v1 eager iterator, see HashMap.iter. */
RuxenVec *ruxen_set_iter(RuxenSet *s) {
    if (!s) return ruxen_vec_new();
    return ruxen_hash_keys(&s->inner);
}

/* HashMap.from_iter(iter[(K, V)]) / Array#to_h.
   Each iter slot is a heap-allocated 2-tuple with field0 at +0 and
   field1 at +8, matching the compiler's generic tuple allocation rule.
   A tuple owns its elements, so the table owns COPIES: each pair is
   cloned with `pair_desc`, the element descriptor the source's own drop
   frees a pair with (declared `ElemDesc[T]` on `Array#to_h`), and its key
   and value move out of the copy into the table, whose `key_desc`
   (declared `KeyDesc[K]`) frees the key the way the copy made it. The
   emptied copy is then dropped through `pair_desc` (its glue skips the
   emptied slots). Copy depth therefore equals drop depth, and source and
   table drop independently. The pairs are read from the last, skipping a
   key already present, so the last pair for a key wins (Ruby) and no copy
   is made for one that would be replaced. A POD descriptor (a pair the
   compiler cannot describe) makes no pair copy: the key is copied by the
   key descriptor and the value word is shared. */
RuxenHash *ruxen_hash_from_iter(RuxenVec *iter, int64_t pair_desc, int64_t key_desc) {
    RuxenHash *out = ruxen_hash_new(key_desc);
    if (!iter) return out;
    for (uint64_t i = iter->len; i > 0; i--) {
        int64_t *pair = (int64_t *)iter->data[i - 1];
        if (!pair || ruxen_hash_contains_key(out, pair[0])) continue;
        int64_t *copy = (int64_t *)ruxen_elem_clone((int64_t)pair, pair_desc);
        if (copy == pair) {
            ruxen_hash_insert(out, ruxen_elem_clone(pair[0], key_desc), pair[1], key_desc);
            continue;
        }
        int64_t key = copy[0];
        int64_t value = copy[1];
        copy[0] = 0;
        copy[1] = 0;
        ruxen_hash_insert(out, key, value, key_desc);
        ruxen_elem_drop((int64_t)copy, pair_desc);
    }
    return out;
}

/* HashSet.from_iter(iter[T]) / iter.collect[HashSet[T]].
 *
 * OWNERSHIP CONTRACT
 *   Allocates: the table, plus a copy of every element whose key
 *     descriptor says the slot owns heap.
 *   Frees: nothing. `iter` is BORROWED — it is an ordinary owned
 *     `Array[T]` local in the caller's frame (`a.to_set` leaves `a`
 *     fully alive), and its scope-exit `ruxen_vec_drop` will
 *     deep-free its own elements.
 *   Caller may assume: source and result can be dropped independently.
 *
 * The elements are copied rather than aliased BECAUSE the source's drop
 * is element-aware, as `ruxen_hash_from_iter` above copies for the same
 * reason. */
RuxenSet *ruxen_set_from_iter(RuxenVec *iter, int64_t key_desc) {
    RuxenSet *out = ruxen_set_new(key_desc);
    if (!iter) return out;
    for (uint64_t i = 0; i < iter->len; i++) {
        ruxen_set_insert_owned(out, iter->data[i]);
    }
    return out;
}

/* HashSet.union(&Self) -> HashSet[T]
   Returns a freshly-allocated HashSet containing every element of
   `a` and every element of `b`. Both inputs are borrowed (insert
   semantics dedup). The new set is registered as a fresh-alloc
   callee in mir/lower.rs::FRESH_ALLOC_CALLEES so its lifetime is
   the caller's drop frame. */
/* The result of a set-algebra operation has the same ELEMENT type as its
   operands, so it inherits their key descriptor. `ruxen_set_derived`
   picks whichever operand actually carries one — an empty/UNSET operand
   must not erase a real descriptor supplied by the other side. */
static RuxenSet *ruxen_set_derived(const RuxenSet *a, const RuxenSet *b) {
    if (a && a->inner.key_desc != RUXEN_ELEM_UNSET) return ruxen_set_alloc(a->inner.key_desc);
    if (b && b->inner.key_desc != RUXEN_ELEM_UNSET) return ruxen_set_alloc(b->inner.key_desc);
    return ruxen_set_alloc(RUXEN_ELEM_UNSET);
}

RuxenSet *ruxen_set_union(RuxenSet *a, RuxenSet *b) {
    RuxenSet *out = ruxen_set_derived(a, b);
    if (a) {
        for (uint64_t i = 0; i < a->inner.bucket_count; i++) {
            for (RuxenHashEntry *e = a->inner.buckets[i]; e; e = e->next) {
                ruxen_set_insert_owned(out, e->key);
            }
        }
    }
    if (b) {
        for (uint64_t i = 0; i < b->inner.bucket_count; i++) {
            for (RuxenHashEntry *e = b->inner.buckets[i]; e; e = e->next) {
                ruxen_set_insert_owned(out, e->key);
            }
        }
    }
    return out;
}

/* HashSet.intersection(&Self) -> HashSet[T] */
RuxenSet *ruxen_set_intersection(RuxenSet *a, RuxenSet *b) {
    RuxenSet *out = ruxen_set_derived(a, b);
    if (!a || !b) return out;
    for (uint64_t i = 0; i < a->inner.bucket_count; i++) {
        for (RuxenHashEntry *e = a->inner.buckets[i]; e; e = e->next) {
            if (ruxen_set_contains(b, e->key)) {
                ruxen_set_insert_owned(out, e->key);
            }
        }
    }
    return out;
}

/* HashSet.difference(&Self) -> HashSet[T] — elements in `a` not in `b`. */
RuxenSet *ruxen_set_difference(RuxenSet *a, RuxenSet *b) {
    RuxenSet *out = ruxen_set_derived(a, b);
    if (!a) return out;
    for (uint64_t i = 0; i < a->inner.bucket_count; i++) {
        for (RuxenHashEntry *e = a->inner.buckets[i]; e; e = e->next) {
            if (!b || !ruxen_set_contains(b, e->key)) {
                ruxen_set_insert_owned(out, e->key);
            }
        }
    }
    return out;
}

/* HashSet == HashSet — same length + every element of `a` is in `b`. */
int8_t ruxen_set_eq(RuxenSet *a, RuxenSet *b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    if (a->inner.len != b->inner.len) return 0;
    for (uint64_t i = 0; i < a->inner.bucket_count; i++) {
        for (RuxenHashEntry *e = a->inner.buckets[i]; e; e = e->next) {
            if (!ruxen_set_contains(b, e->key)) return 0;
        }
    }
    return 1;
}
