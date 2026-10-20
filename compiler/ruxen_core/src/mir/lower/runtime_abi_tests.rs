//! Unit tests for `runtime_abi.rs`: callee ownership classes and the FFI
//! by-value parameter rule.

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
fn ffi_value_params_move_and_borrowed_params_do_not() {
    use crate::hir::types::Ty;
    let elem = Ty::TypeParam {
        name: "T".to_string(),
        bounds: Vec::new(),
    };
    // `Array#push(x: T)`: the receiver is borrowed, `x` moves into C.
    assert!(!ffi_param_takes_value(&elem, true));
    assert!(ffi_param_takes_value(&elem, false));
    assert!(ffi_param_takes_value(&Ty::String, false));
    // A C function that only reads declares `&T`; scalars are copies.
    assert!(!ffi_param_takes_value(
        &Ty::Ref(Box::new(Ty::String)),
        false
    ));
    assert!(!ffi_param_takes_value(
        &Ty::RefMut(Box::new(Ty::String)),
        false
    ));
    assert!(!ffi_param_takes_value(&Ty::Int, false));
}

#[test]
fn move_by_ffi_does_not_borrow_args() {
    let o = callee_ownership("ruxen_executor_spawn");
    assert!(!o.args_are_borrowed); // default-taint path runs → spawned future moved
}

#[test]
fn is_move_by_ffi_matches_table_and_rejects_dotted_and_user() {
    // Every member of MOVE_BY_FFI is recognized — this is the single
    // predicate drops/'s `moved_to_ffi` collection routes through, so
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
