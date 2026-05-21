//! Vec, HashMap, HashSet, allocator, Option/Result, and panic runtime symbols.

// Vec / Array surface.
pub(in crate::codegen::runtime) const VEC: &[&str] = &[
    "riven_vec_pop",
    "riven_vec_sum",
    "riven_vec_count",
    "riven_vec_reverse",
    "riven_vec_first",
    "riven_vec_last",
    "riven_vec_clone",
    // Phase 2 stdlib (#05 batch 2): lazy iterator combinators.
    "riven_vec_take",
    "riven_vec_skip",
    // Phase 2 stdlib (#05 batch 3): chain / zip materialise into a fresh Vec.
    "riven_vec_chain",
    "riven_vec_zip",
    "riven_vec_contains_int",
    "riven_vec_sort",
    "riven_vec_join",
    // Vec[T] surface — Phase 2 stdlib batch 1 (#03).
    "riven_vec_with_capacity",
    "riven_vec_capacity",
    "riven_vec_clear",
    "riven_vec_truncate",
    "riven_vec_swap",
    "riven_vec_insert",
    "riven_vec_remove",
    "riven_vec_extend",
    "riven_vec_get_or_panic",
    "riven_vec_eq",
    "riven_vec_drop_string",
    "riven_vec_drop_vec",
    // Phase 2 stdlib batch 2 (#03): from_iter, dedup, set.
    "riven_vec_from_iter",
    "riven_vec_dedup",
    "riven_vec_set",
];

// HashMap / HashSet surface.
pub(in crate::codegen::runtime) const HASH_SET: &[&str] = &[
    "riven_hash_new",
    "riven_hash_from_iter",
    "riven_hash_insert",
    "riven_hash_get",
    "riven_hash_contains_key",
    "riven_hash_len",
    "riven_hash_is_empty",
    "riven_set_new",
    "riven_set_from_iter",
    "riven_set_insert",
    "riven_set_contains",
    "riven_set_len",
    "riven_set_is_empty",
    // Phase 2 stdlib (#04): HashMap[K,V] full surface.
    "riven_hash_with_capacity",
    "riven_hash_remove",
    "riven_hash_clear",
    "riven_hash_keys",
    "riven_hash_values",
    "riven_hash_iter",
    "riven_hash_eq",
    "riven_hash_index",
    // Phase 2 stdlib (#04): HashSet[T] full surface.
    "riven_set_with_capacity",
    "riven_set_remove",
    "riven_set_clear",
    "riven_set_iter",
    "riven_set_eq",
    "riven_set_union",
    "riven_set_intersection",
    "riven_set_difference",
];

// Allocator + drop selectors + Option/Result + panic.
pub(in crate::codegen::runtime) const MEMORY_OPTION_RESULT: &[&str] = &[
    "riven_alloc",
    "riven_dealloc",
    "riven_realloc",
    "riven_string_free",
    "riven_vec_free",
    "riven_hash_free",
    // Phase 2 stdlib (#04 batch 2): set spine + per-element drop selectors.
    "riven_set_free",
    "riven_hash_drop_string_v",
    "riven_hash_drop_v_string",
    "riven_hash_drop_string_string",
    "riven_hash_drop_v_vec",
    "riven_set_drop_string",
    "riven_panic",
    "riven_option_expect",
    "riven_option_unwrap",
    "riven_option_is_some",
    "riven_option_is_none",
    "riven_result_expect",
    "riven_result_unwrap",
    "riven_result_is_ok",
    "riven_result_is_err",
    "riven_result_ok",
    "riven_result_err",
];
