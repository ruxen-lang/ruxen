//! Characterization / parity oracle for runtime-callee ownership classification.
//!
//! VERBATIM transcription of the six `drops.rs` `Call`-arm predicates +
//! `transfer_indices` as of commit 340f20f. These free reference functions ARE
//! the parity oracle: they reproduce the *exact* current answers (contradictions
//! included) so the upcoming `runtime_abi` table can be asserted to match them
//! 1:1 BEFORE the old predicates are deleted. Do NOT "clean these up" — a wrong
//! table entry is a double-free / use-after-free (master risk register, Phase 2).
//!
//! Source line ranges transcribed (drops.rs @ 340f20f):
//!   FRESH_ALLOC_CALLEES            694-934
//!   borrows_first_arg              967-984
//!   is_runtime_consume_helper      1003-1043
//!   is_move_by_ffi_callee          1068-1088
//!   is_command_terminal_or_accessor 1123-1139
//!   is_command_builder_method      1158-1168
//!   is_pointer_store_helper        1180
//!   is_runtime_borrow_helper       1089-1170 (effective, post-3-rebinds)
//!   transfer_indices               1215-1247

use ruxen_core::codegen::runtime::extract_method_name;

// ---------------------------------------------------------------------------
// Reference predicates (the golden oracle)
// ---------------------------------------------------------------------------

fn ref_returns_fresh_alloc(c: &str) -> bool {
    // VERBATIM from drops.rs:694-934 FRESH_ALLOC_CALLEES.
    const FRESH_ALLOC_CALLEES: &[&str] = &[
        "String_from",
        "Vec_new",
        "Hash_new",
        "HashMap_new",
        "ruxen_string_from",
        "ruxen_string_concat",
        "ruxen_vec_new",
        "ruxen_hash_new",
        "String_into_bytes",
        "ruxen_string_into_bytes",
        "String_push",
        "ruxen_string_push",
        "Vec_from_iter",
        "ruxen_vec_from_iter",
        "String_from_iter",
        "HashMap_from_iter",
        "Hash_from_iter",
        "Set_from_iter",
        "HashSet_from_iter",
        "ruxen_string_from_iter",
        "ruxen_hash_from_iter",
        "ruxen_set_from_iter",
        "Set_new",
        "HashSet_new",
        "ruxen_set_new",
        "HashMap_with_capacity",
        "Hash_with_capacity",
        "ruxen_hash_with_capacity",
        "HashMap_keys",
        "HashMap_values",
        "HashMap_iter",
        "Hash_keys",
        "Hash_values",
        "Hash_iter",
        "ruxen_hash_keys",
        "ruxen_hash_values",
        "ruxen_hash_iter",
        "HashMap_remove",
        "Hash_remove",
        "ruxen_hash_remove",
        "HashSet_with_capacity",
        "Set_with_capacity",
        "ruxen_set_with_capacity",
        "HashSet_iter",
        "Set_iter",
        "ruxen_set_iter",
        "HashSet_union",
        "HashSet_intersection",
        "HashSet_difference",
        "Set_union",
        "Set_intersection",
        "Set_difference",
        "ruxen_set_union",
        "ruxen_set_intersection",
        "ruxen_set_difference",
        "ruxen_vec_chain",
        "ruxen_vec_zip",
        "Command_new",
        "ruxen_command_new",
        "Command_arg",
        "ruxen_command_arg",
        "Command_args",
        "ruxen_command_args",
        "Command_env",
        "ruxen_command_env",
        "Command_current_dir",
        "ruxen_command_current_dir",
        "Command_status",
        "ruxen_command_status",
        "Command_output",
        "ruxen_command_output",
        "Output_status",
        "ruxen_output_status",
        "Output_stdout",
        "ruxen_output_stdout",
        "Output_stderr",
        "ruxen_output_stderr",
        "File_open",
        "ruxen_file_open",
        "File_create",
        "ruxen_file_create",
        "File_append",
        "ruxen_file_append",
        "File_open_options",
        "ruxen_file_open_options",
        "File_metadata",
        "ruxen_file_metadata",
        "File_read",
        "ruxen_file_read",
        "File_read_to_string",
        "ruxen_file_read_to_string",
        "File_read_all",
        "ruxen_file_read_all",
        "File_write",
        "ruxen_file_write",
        "File_write_all",
        "ruxen_file_write_all",
        "File_write_str",
        "ruxen_file_write_str",
        "File_flush",
        "ruxen_file_flush",
        "File_seek",
        "ruxen_file_seek",
        "File_close",
        "ruxen_file_close",
        "OpenOptions_new",
        "ruxen_open_options_new",
        "OpenOptions_read",
        "ruxen_open_options_read",
        "OpenOptions_write",
        "ruxen_open_options_write",
        "OpenOptions_append",
        "ruxen_open_options_append",
        "OpenOptions_truncate",
        "ruxen_open_options_truncate",
        "OpenOptions_create",
        "ruxen_open_options_create",
        "OpenOptions_create_new",
        "ruxen_open_options_create_new",
        "Mutex_lock_raw",
        "Mutex_try_lock_raw",
        "ruxen_mutex_lock",
        "ruxen_mutex_try_lock",
    ];
    FRESH_ALLOC_CALLEES.contains(&c)
}

fn ref_borrows_first_arg(c: &str) -> bool {
    // VERBATIM from drops.rs:967-984.
    c.ends_with("_init")
        || c == "ruxen_dealloc"
        || c == "ruxen_string_free"
        || c == "ruxen_vec_free"
        || c == "ruxen_vec_drop_string"
        || c == "ruxen_vec_drop_vec"
        || c == "ruxen_hash_free"
        || c == "ruxen_set_free"
        || c == "ruxen_hash_drop_string_v"
        || c == "ruxen_hash_drop_v_string"
        || c == "ruxen_hash_drop_string_string"
        || c == "ruxen_hash_drop_v_vec"
        || c == "ruxen_set_drop_string"
}

fn ref_is_runtime_consume_helper(c: &str) -> bool {
    // VERBATIM from drops.rs:1003-1043.
    matches!(
        c,
        "ruxen_dealloc"
            | "ruxen_string_free"
            | "ruxen_vec_free"
            | "ruxen_vec_drop_string"
            | "ruxen_vec_drop_vec"
            | "ruxen_hash_free"
            | "ruxen_set_free"
            | "ruxen_hash_drop_string_v"
            | "ruxen_hash_drop_v_string"
            | "ruxen_hash_drop_string_string"
            | "ruxen_hash_drop_v_vec"
            | "ruxen_set_drop_string"
            | "ruxen_string_into_bytes"
            | "String_into_bytes"
            | "ruxen_vec_from_iter"
            | "Vec_from_iter"
    )
}

fn ref_is_move_by_ffi_callee(c: &str) -> bool {
    // VERBATIM from drops.rs:1068-1088.
    matches!(
        c,
        "ruxen_executor_spawn"
            | "Task_spawn_raw"
            | "ruxen_thread_spawn"
            | "Thread_spawn"
            | "Thread_spawn_raw"
    )
}

fn ref_is_command_terminal_or_accessor(c: &str) -> bool {
    // VERBATIM from drops.rs:1123-1139.
    matches!(
        c,
        "Command_status"
            | "ruxen_command_status"
            | "Command_output"
            | "ruxen_command_output"
            | "Output_status"
            | "ruxen_output_status"
            | "Output_stdout"
            | "ruxen_output_stdout"
            | "Output_stderr"
            | "ruxen_output_stderr"
            | "ExitStatus_code"
            | "ruxen_exit_status_code"
            | "ExitStatus_success"
            | "ruxen_exit_status_success"
    )
}

fn ref_is_command_builder_method(c: &str) -> bool {
    // VERBATIM from drops.rs:1158-1168.
    matches!(
        c,
        "Command_arg"
            | "ruxen_command_arg"
            | "Command_args"
            | "ruxen_command_args"
            | "Command_env"
            | "ruxen_command_env"
            | "Command_current_dir"
            | "ruxen_command_current_dir"
    )
}

fn ref_is_pointer_store_helper(c: &str) -> bool {
    // VERBATIM from drops.rs:1180.
    c == "ruxen_store_ptr"
}

// Effective is_runtime_borrow_helper AFTER all three rebinds (1089/1140/1169):
//   base       = !consume && !move_by_ffi && (prefix match)   (1089-1107)
//   with_term  = base || command_terminal_or_accessor          (1140-1141)
//   effective  = with_term && !command_builder_method          (1169-1170)
fn ref_is_runtime_borrow_helper(c: &str) -> bool {
    let base = !ref_is_runtime_consume_helper(c)
        && !ref_is_move_by_ffi_callee(c)
        && (c.starts_with("ruxen_")
            || c.starts_with("Vec_")
            || c.starts_with("Vec[")
            || c.starts_with("Array_")
            || c.starts_with("Array[")
            || c.starts_with("Hash_")
            || c.starts_with("Hash[")
            || c.starts_with("HashMap_")
            || c.starts_with("HashMap[")
            || c.starts_with("Map_")
            || c.starts_with("Map[")
            || c.starts_with("Set_")
            || c.starts_with("Set[")
            || c.starts_with("HashSet_")
            || c.starts_with("HashSet[")
            || c.starts_with("String_")
            || c.starts_with("&str_"));
    let with_terminal = base || ref_is_command_terminal_or_accessor(c);
    with_terminal && !ref_is_command_builder_method(c)
}

fn ref_transfer_indices(c: &str) -> &'static [usize] {
    // VERBATIM from drops.rs:1215-1247.
    let m = extract_method_name(c);
    let is_vec = c.starts_with("Vec_")
        || c.starts_with("Vec[")
        || c.starts_with("Array_")
        || c.starts_with("Array[");
    let is_hash = c.starts_with("Hash_")
        || c.starts_with("Hash[")
        || c.starts_with("HashMap_")
        || c.starts_with("HashMap[")
        || c.starts_with("Map_")
        || c.starts_with("Map[");
    let is_set = c.starts_with("Set_")
        || c.starts_with("Set[")
        || c.starts_with("HashSet_")
        || c.starts_with("HashSet[");
    match (is_vec, is_hash, is_set, m) {
        (true, _, _, "push") => &[1],
        (true, _, _, "insert") => &[2],
        (_, true, _, "insert") => &[1, 2],
        (_, _, true, "insert") => &[1],
        _ if c == "ruxen_vec_push" => &[1],
        _ if c == "ruxen_vec_insert" => &[2],
        _ if c == "ruxen_hash_insert" => &[1, 2],
        _ if c == "ruxen_set_insert" => &[1],
        _ => &[],
    }
}

// ---------------------------------------------------------------------------
// The audit surface: union of every symbol any predicate mentions, plus
// prefix-probe witnesses for is_runtime_borrow_helper / transfer_indices and
// user-callee / _init witnesses.
// ---------------------------------------------------------------------------

pub(crate) const SYMBOLS: &[&str] = &[
    // FRESH_ALLOC_CALLEES (694-934)
    "String_from",
    "Vec_new",
    "Hash_new",
    "HashMap_new",
    "ruxen_string_from",
    "ruxen_string_concat",
    "ruxen_vec_new",
    "ruxen_hash_new",
    "String_into_bytes",
    "ruxen_string_into_bytes",
    "String_push",
    "ruxen_string_push",
    "Vec_from_iter",
    "ruxen_vec_from_iter",
    "String_from_iter",
    "HashMap_from_iter",
    "Hash_from_iter",
    "Set_from_iter",
    "HashSet_from_iter",
    "ruxen_string_from_iter",
    "ruxen_hash_from_iter",
    "ruxen_set_from_iter",
    "Set_new",
    "HashSet_new",
    "ruxen_set_new",
    "HashMap_with_capacity",
    "Hash_with_capacity",
    "ruxen_hash_with_capacity",
    "HashMap_keys",
    "HashMap_values",
    "HashMap_iter",
    "Hash_keys",
    "Hash_values",
    "Hash_iter",
    "ruxen_hash_keys",
    "ruxen_hash_values",
    "ruxen_hash_iter",
    "HashMap_remove",
    "Hash_remove",
    "ruxen_hash_remove",
    "HashSet_with_capacity",
    "Set_with_capacity",
    "ruxen_set_with_capacity",
    "HashSet_iter",
    "Set_iter",
    "ruxen_set_iter",
    "HashSet_union",
    "HashSet_intersection",
    "HashSet_difference",
    "Set_union",
    "Set_intersection",
    "Set_difference",
    "ruxen_set_union",
    "ruxen_set_intersection",
    "ruxen_set_difference",
    "ruxen_vec_chain",
    "ruxen_vec_zip",
    "Command_new",
    "ruxen_command_new",
    "Command_arg",
    "ruxen_command_arg",
    "Command_args",
    "ruxen_command_args",
    "Command_env",
    "ruxen_command_env",
    "Command_current_dir",
    "ruxen_command_current_dir",
    "Command_status",
    "ruxen_command_status",
    "Command_output",
    "ruxen_command_output",
    "Output_status",
    "ruxen_output_status",
    "Output_stdout",
    "ruxen_output_stdout",
    "Output_stderr",
    "ruxen_output_stderr",
    "File_open",
    "ruxen_file_open",
    "File_create",
    "ruxen_file_create",
    "File_append",
    "ruxen_file_append",
    "File_open_options",
    "ruxen_file_open_options",
    "File_metadata",
    "ruxen_file_metadata",
    "File_read",
    "ruxen_file_read",
    "File_read_to_string",
    "ruxen_file_read_to_string",
    "File_read_all",
    "ruxen_file_read_all",
    "File_write",
    "ruxen_file_write",
    "File_write_all",
    "ruxen_file_write_all",
    "File_write_str",
    "ruxen_file_write_str",
    "File_flush",
    "ruxen_file_flush",
    "File_seek",
    "ruxen_file_seek",
    "File_close",
    "ruxen_file_close",
    "OpenOptions_new",
    "ruxen_open_options_new",
    "OpenOptions_read",
    "ruxen_open_options_read",
    "OpenOptions_write",
    "ruxen_open_options_write",
    "OpenOptions_append",
    "ruxen_open_options_append",
    "OpenOptions_truncate",
    "ruxen_open_options_truncate",
    "OpenOptions_create",
    "ruxen_open_options_create",
    "OpenOptions_create_new",
    "ruxen_open_options_create_new",
    "Mutex_lock_raw",
    "Mutex_try_lock_raw",
    "ruxen_mutex_lock",
    "ruxen_mutex_try_lock",
    // borrows_first_arg / consume helpers (967-984, 1003-1043)
    "ruxen_dealloc",
    "ruxen_string_free",
    "ruxen_vec_free",
    "ruxen_vec_drop_string",
    "ruxen_vec_drop_vec",
    "ruxen_hash_free",
    "ruxen_set_free",
    "ruxen_hash_drop_string_v",
    "ruxen_hash_drop_v_string",
    "ruxen_hash_drop_string_string",
    "ruxen_hash_drop_v_vec",
    "ruxen_set_drop_string",
    // move-by-ffi (1068-1088)
    "ruxen_executor_spawn",
    "Task_spawn_raw",
    "ruxen_thread_spawn",
    "Thread_spawn",
    "Thread_spawn_raw",
    // command terminal/accessor not already above (1123-1139)
    "ExitStatus_code",
    "ruxen_exit_status_code",
    "ExitStatus_success",
    "ruxen_exit_status_success",
    // pointer store (1180)
    "ruxen_store_ptr",
    // transfer_indices runtime witnesses (1241-1244)
    "ruxen_vec_push",
    "ruxen_vec_insert",
    "ruxen_hash_insert",
    "ruxen_set_insert",
    // prefix-probe witnesses for is_runtime_borrow_helper / transfer_indices
    "Vec_push",
    "Vec[String]_push",
    "Vec_insert",
    "Array_len",
    "Hash_get",
    "HashMap_insert",
    "Map_get",
    "Set_contains",
    "HashSet_insert",
    "String_len",
    "&str_len",
    // user-callee + _init witnesses
    "Point_new",
    "MyClass_method",
    "Point_init",
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn into_bytes_is_dual_axis_fresh_result_and_consumed_arg() {
    // INVESTIGATED (Phase 2 Task 3b): the "fresh-vs-consume contradiction" was
    // a FALSE hypothesis. into_bytes legitimately occupies both axes the drop
    // pass tracks INDEPENDENTLY:
    //   * RESULT axis: Fresh — it produces a brand-new Vec[U8] spine the dest
    //     local owns and must `ruxen_vec_free` at scope exit.
    //   * ARG axis: consumed — the runtime frees the source char* internally,
    //     so the source String must be tainted to avoid a double-free.
    // Dropping the Fresh membership LEAKS the Vec (proven by the leak backstop
    // drop_fixtures::string_into_bytes_transfers_ownership →
    // raw_outstanding=2, vec_frees=0). Both classifications are load-bearing.
    let s = callee_ownership("ruxen_string_into_bytes");
    assert_eq!(s.result, ResultOwnership::Fresh, "fresh Vec[U8] result");
    assert!(!s.args_are_borrowed, "source char* is consumed (arg tainted)");
    let m = callee_ownership("String_into_bytes");
    assert_eq!(m.result, ResultOwnership::Fresh);
    assert!(!m.args_are_borrowed);
}

#[test]
fn oracle_sanity_spot_checks() {
    // A few hand-verified answers to catch a transcription typo early.
    assert!(ref_returns_fresh_alloc("Vec_new"));
    assert!(!ref_returns_fresh_alloc("MyClass_method"));

    assert!(ref_borrows_first_arg("Point_init"));
    assert!(ref_borrows_first_arg("ruxen_string_free"));
    assert!(!ref_borrows_first_arg("Vec_push"));

    // Vec_push: borrow helper (prefix) AND transfers arg1.
    assert!(ref_is_runtime_borrow_helper("Vec_push"));
    assert_eq!(ref_transfer_indices("Vec_push"), &[1usize][..]);
    assert_eq!(ref_transfer_indices("HashMap_insert"), &[1usize, 2][..]);
    assert_eq!(ref_transfer_indices("ruxen_set_insert"), &[1usize][..]);
    assert_eq!(ref_transfer_indices("Array_len"), &[] as &[usize]);

    // Command builder suppresses borrow; terminal restores it.
    assert!(!ref_is_runtime_borrow_helper("Command_arg"));
    assert!(ref_is_runtime_borrow_helper("Command_status"));

    // move-by-ffi suppresses borrow even with ruxen_ prefix.
    assert!(!ref_is_runtime_borrow_helper("ruxen_executor_spawn"));

    // user callee: not a borrow helper.
    assert!(!ref_is_runtime_borrow_helper("MyClass_method"));

    assert!(ref_is_pointer_store_helper("ruxen_store_ptr"));
}

// ---------------------------------------------------------------------------
// Full-union parity: the production table must reproduce every oracle answer
// for every symbol. This is the SAFETY GATE — it proves the table is
// byte-equivalent to the six old predicates BEFORE drops.rs deletes them.
// ---------------------------------------------------------------------------

use ruxen_core::mir::lower::runtime_abi::{callee_ownership, ArgMask, ResultOwnership};

/// Reconstruct the expected ArgMask from the oracle's transfer index list +
/// the pointer-store rule. In this domain at most two indices ever transfer.
fn expected_arg_mask(c: &str) -> ArgMask {
    let mut idxs: Vec<usize> = ref_transfer_indices(c).to_vec();
    if ref_is_pointer_store_helper(c) && !idxs.contains(&1) {
        idxs.push(1);
    }
    match idxs.as_slice() {
        [] => ArgMask::none(),
        [i] => ArgMask::single(*i),
        [i, j] => ArgMask::pair(*i, *j),
        _ => panic!("more than two transfer indices for {c:?}: {idxs:?}"),
    }
}

#[test]
fn table_reproduces_every_oracle_answer_for_every_symbol() {
    for &c in SYMBOLS {
        let o = callee_ownership(c);

        // result == FRESH_ALLOC_CALLEES membership
        assert_eq!(
            o.result == ResultOwnership::Fresh,
            ref_returns_fresh_alloc(c),
            "result mismatch for {c:?}"
        );

        // borrows_first_arg
        assert_eq!(
            o.borrows_first_arg,
            ref_borrows_first_arg(c),
            "borrows_first_arg mismatch for {c:?}"
        );

        // args_are_borrowed == effective is_runtime_borrow_helper
        assert_eq!(
            o.args_are_borrowed,
            ref_is_runtime_borrow_helper(c),
            "args_are_borrowed mismatch for {c:?}"
        );

        // arg_transfer == transfer_indices (plus pointer-store arg1)
        assert_eq!(
            o.arg_transfer,
            expected_arg_mask(c),
            "arg_transfer mismatch for {c:?}"
        );
    }
}
