//! Vec, HashMap, HashSet, allocator, Option/Result, and panic runtime symbols.

// Vec / Array surface.
pub(in crate::codegen::runtime) const VEC: &[&str] = &[
    "ruxen_vec_pop",
    "ruxen_vec_sum",
    "ruxen_vec_count",
    "ruxen_vec_reverse",
    "ruxen_vec_first",
    "ruxen_vec_last",
    "ruxen_vec_clone",
    // Phase 2 stdlib (#05 batch 2): lazy iterator combinators.
    "ruxen_vec_take",
    "ruxen_vec_skip",
    // Phase 2 stdlib (#05 batch 3): chain / zip materialise into a fresh Vec.
    "ruxen_vec_chain",
    "ruxen_vec_zip",
    "ruxen_vec_contains_int",
    "ruxen_vec_sort",
    "ruxen_vec_join",
    // Vec[T] surface — Phase 2 stdlib batch 1 (#03).
    "ruxen_vec_with_capacity",
    "ruxen_vec_capacity",
    "ruxen_vec_clear",
    "ruxen_vec_truncate",
    "ruxen_vec_swap",
    "ruxen_vec_insert",
    "ruxen_vec_remove",
    "ruxen_vec_extend",
    "ruxen_vec_get_or_panic",
    "ruxen_vec_eq",
    "ruxen_vec_drop_string",
    "ruxen_vec_drop_vec",
    // Phase 2 stdlib batch 2 (#03): from_iter, dedup, set.
    "ruxen_vec_from_iter",
    "ruxen_vec_dedup",
    "ruxen_vec_set",
];

// HashMap / HashSet surface.
pub(in crate::codegen::runtime) const HASH_SET: &[&str] = &[
    "ruxen_hash_new",
    "ruxen_hash_from_iter",
    "ruxen_hash_insert",
    "ruxen_hash_get",
    "ruxen_hash_contains_key",
    "ruxen_hash_len",
    "ruxen_hash_is_empty",
    "ruxen_set_new",
    "ruxen_set_from_iter",
    "ruxen_set_insert",
    "ruxen_set_contains",
    "ruxen_set_len",
    "ruxen_set_is_empty",
    // Phase 2 stdlib (#04): HashMap[K,V] full surface.
    "ruxen_hash_with_capacity",
    "ruxen_hash_remove",
    "ruxen_hash_clear",
    "ruxen_hash_keys",
    "ruxen_hash_values",
    "ruxen_hash_iter",
    "ruxen_hash_eq",
    "ruxen_hash_index",
    // Phase 2 stdlib (#04): HashSet[T] full surface.
    "ruxen_set_with_capacity",
    "ruxen_set_remove",
    "ruxen_set_clear",
    "ruxen_set_iter",
    "ruxen_set_eq",
    "ruxen_set_union",
    "ruxen_set_intersection",
    "ruxen_set_difference",
];

// Allocator + drop selectors + Option/Result + panic.
pub(in crate::codegen::runtime) const MEMORY_OPTION_RESULT: &[&str] = &[
    "ruxen_alloc",
    "ruxen_dealloc",
    "ruxen_realloc",
    "ruxen_string_free",
    "ruxen_vec_free",
    "ruxen_hash_free",
    // Phase 2 stdlib (#04 batch 2): set spine + per-element drop selectors.
    "ruxen_set_free",
    "ruxen_hash_drop_string_v",
    "ruxen_hash_drop_v_string",
    "ruxen_hash_drop_string_string",
    "ruxen_hash_drop_v_vec",
    "ruxen_set_drop_string",
    "ruxen_panic",
    "ruxen_option_expect",
    "ruxen_option_unwrap",
    "ruxen_option_is_some",
    "ruxen_option_is_none",
    "ruxen_result_expect",
    "ruxen_result_unwrap",
    "ruxen_result_is_ok",
    "ruxen_result_is_err",
    "ruxen_result_ok",
    "ruxen_result_err",
];
