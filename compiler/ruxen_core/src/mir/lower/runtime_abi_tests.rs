//! Unit tests for `runtime_abi.rs`: callee ownership classes and arg masks.

use super::*;

#[test]
fn fresh_alloc_constructors_classify_as_fresh() {
    assert_eq!(callee_ownership("Vec_new").result, ResultOwnership::Fresh);
    assert_eq!(
        callee_ownership("ruxen_hash_new").result,
        ResultOwnership::Fresh
    );
    assert_eq!(
        callee_ownership("Command_new").result,
        ResultOwnership::Fresh
    );
    assert_eq!(callee_ownership("File_open").result, ResultOwnership::Fresh);
    assert_eq!(
        callee_ownership("Mutex_lock_raw").result,
        ResultOwnership::Fresh
    );
    assert_eq!(
        callee_ownership("ruxen_vec_skip").result,
        ResultOwnership::Fresh
    );
}

#[test]
fn free_family_consumes_first_arg_and_returns_nothing() {
    let o = callee_ownership("ruxen_string_free");
    assert_eq!(o.result, ResultOwnership::None);
    // free helpers BORROW arg0 in the drop-pass sense (the arg loop
    // `continue`s on arg0 so it is NOT tainted) — encoded as the
    // borrows_first_arg flag.
    assert!(o.borrows_first_arg);
}

#[test]
fn drop_frees_storage_classifies_ffi_ownership_primitives_only() {
    assert!(drop_frees_storage("ruxen_mutex_drop"));
    assert!(drop_frees_storage("ruxen_sharedsync_drop"));
    assert!(drop_frees_storage("ruxen_channel_sender_drop"));
    assert!(!drop_frees_storage("ruxen_mutex_guard_drop"));
    assert!(!drop_frees_storage("ruxen_file_drop"));
    assert!(!drop_frees_storage("Holder_drop"));
}

#[test]
fn vec_push_transfers_arg1() {
    assert_eq!(
        callee_ownership("ruxen_vec_push").arg_transfer,
        ArgMask::single(1)
    );
    assert_eq!(
        callee_ownership("Vec_push").arg_transfer,
        ArgMask::single(1)
    );
}

#[test]
fn hashmap_insert_transfers_arg1_and_arg2() {
    assert_eq!(
        callee_ownership("ruxen_hash_insert").arg_transfer,
        ArgMask::pair(1, 2)
    );
    assert_eq!(
        callee_ownership("HashMap_insert").arg_transfer,
        ArgMask::pair(1, 2)
    );
}

#[test]
fn move_by_ffi_does_not_borrow_args() {
    let o = callee_ownership("ruxen_executor_spawn");
    assert!(!o.args_are_borrowed); // default-taint path runs → spawned future moved
}

#[test]
fn is_move_by_ffi_matches_table_and_rejects_dotted_and_user() {
    // Every member of MOVE_BY_FFI is recognized — this is the single
    // predicate drops.rs's `moved_to_ffi` collection routes through, so
    // the two can no longer diverge.
    for &(symbol, spellings) in MOVE_BY_FFI {
        for &c in std::iter::once(&symbol).chain(spellings) {
            assert!(is_move_by_ffi(c), "MOVE_BY_FFI member {c:?} not recognized");
        }
    }
    // The dotted `Task.spawn_raw` form is DEAD: MIR call callees are
    // always underscore-mangled (`Type_method`), so the dotted spelling
    // never reaches a `MirInst::Call.callee`. It is intentionally NOT in
    // the table (and absent from the parity SYMBOLS list).
    assert!(!is_move_by_ffi("Task.spawn_raw"));
    // User callees never move by FFI.
    assert!(!is_move_by_ffi("MyClass_method"));
}

#[test]
fn user_callee_is_fully_conservative() {
    let o = callee_ownership("MyClass_method");
    assert_eq!(o.result, ResultOwnership::None); // dest tainted
    assert!(!o.args_are_borrowed); // every Use(arg) tainted
    assert!(!o.borrows_first_arg);
}

#[test]
fn arg_mask_contains_is_safe_beyond_u8_width() {
    // A many-arg FFI (e.g. `ruxen_canvas_draw_rect`, 9 args) queries
    // `contains()` for arg indices >= 8; the u8 mask must NOT overflow-shift
    // (regression: `1u8 << 8` panicked in debug). Bits beyond the u8 are
    // unset by definition.
    let m = ArgMask::single(1);
    assert!(m.contains(1));
    assert!(!m.contains(8));
    assert!(!m.contains(64));
    assert!(!ArgMask::none().contains(8));
}
