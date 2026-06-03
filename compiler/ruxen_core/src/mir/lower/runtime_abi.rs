//! The single source of truth for runtime-callee ownership in MIR lowering.
//!
//! Every runtime symbol the drop-elaboration pass (`drops.rs`) or the
//! static-constructor fast path (`expr/method_call.rs`) special-cases is
//! classified HERE, exactly once, by `callee_ownership`. This replaces the six
//! overlapping, partly-contradicting predicates that previously lived inline in
//! the `drops.rs` `Call` arm (FRESH_ALLOC_CALLEES, is_runtime_consume_helper,
//! is_runtime_borrow_helper ×3 rebinds, is_move_by_ffi_callee,
//! is_pointer_store_helper, borrows_first_arg) plus the duplicated static-ctor
//! lists in method_call.rs / util.rs.
//!
//! ABI contract: the C side's ownership semantics are documented per-symbol in
//! the stdlib specs cited inline (task_spawn.spec.md §B10, the Command builder
//! notes, mutex.c, scheduler.c:150-152). A WRONG entry here is a double-free or
//! use-after-free — every change must move a row, with a visible diff to the
//! parity oracle (tests/runtime_abi_parity.rs).

use crate::codegen::runtime::extract_method_name;

/// What the callee does with the value it RETURNS into `dest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultOwnership {
    /// Returns a fresh heap allocation owned exclusively by `dest`
    /// (drop-elaborate `dest` at scope exit). Old: FRESH_ALLOC_CALLEES.
    Fresh,
    /// No owning result to track (Unit/Int return, or a free helper).
    /// `dest` is conservatively tainted (dropped from alloc_rooted).
    None,
}

/// Which positional arguments have their ownership TRANSFERRED to the callee
/// (the arg's local must be tainted / removed from `alloc_rooted`). Small,
/// bounded set — at most arg0..arg2 ever transfer in v1, so a u8 bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArgMask(u8);

impl ArgMask {
    pub const fn none() -> Self {
        ArgMask(0)
    }
    pub const fn single(i: usize) -> Self {
        ArgMask(1 << i)
    }
    pub const fn pair(i: usize, j: usize) -> Self {
        ArgMask((1 << i) | (1 << j))
    }
    pub fn contains(self, i: usize) -> bool {
        self.0 & (1 << i) != 0
    }
}

/// The full ownership verdict for one callee. Mirrors the four decisions the
/// `drops.rs` arg loop makes, in the SAME precedence order it applied them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalleeOwnership {
    /// Result-into-`dest` ownership.
    pub result: ResultOwnership,
    /// arg0 is borrowed even though the callee otherwise transfers (the
    /// `borrows_first_arg` rule: `_init` ctors + the free/drop family). The
    /// arg loop `continue`s on arg0 when this is set. HIGHEST arg precedence.
    pub borrows_first_arg: bool,
    /// Args that are explicitly transferred regardless of `args_are_borrowed`
    /// (Vec/Hash/Set push/insert value+key slots; ruxen_store_ptr arg1). These
    /// are tainted before the borrow check. Old: transfer_indices +
    /// is_pointer_store_helper.
    pub arg_transfer: ArgMask,
    /// All remaining pointer args are BORROWED (not transferred) — the runtime
    /// helper reads/mutates in place. Old: effective is_runtime_borrow_helper.
    /// When false (user callees, move-by-FFI), every remaining Use(arg) is
    /// tainted. LOWEST arg precedence.
    pub args_are_borrowed: bool,
}

impl Default for CalleeOwnership {
    /// The conservative default for an unknown / user-defined callee: no owning
    /// result to track, every arg transferred (tainted). This is the ONE
    /// fallthrough and is intentionally pessimistic (over-tainting a primitive
    /// temp is a no-op; under-tainting is a UAF).
    fn default() -> Self {
        CalleeOwnership {
            result: ResultOwnership::None,
            borrows_first_arg: false,
            arg_transfer: ArgMask::none(),
            args_are_borrowed: false,
        }
    }
}

/// THE single ownership lookup. Ordered exactly like the old `Call` arm so the
/// parity oracle (tests/runtime_abi_parity.rs) reproduces 1:1.
pub fn callee_ownership(callee: &str) -> CalleeOwnership {
    // --- result ownership (old FRESH_ALLOC_CALLEES) ---
    let result = if FRESH_ALLOC_CALLEES.contains(&callee) {
        ResultOwnership::Fresh
    } else {
        ResultOwnership::None
    };

    // --- arg0 borrow (old borrows_first_arg) ---
    let borrows_first_arg = callee.ends_with("_init") || FREE_FAMILY.contains(&callee);

    // --- explicit per-arg transfers (old transfer_indices + pointer-store) ---
    let arg_transfer = arg_transfer_mask(callee);

    // --- remaining-arg borrow (old effective is_runtime_borrow_helper) ---
    let consumes = CONSUME_HELPERS.contains(&callee);
    let move_by_ffi = MOVE_BY_FFI.contains(&callee);
    let builder = COMMAND_BUILDER.contains(&callee);
    let prefix_borrow = has_runtime_prefix(callee);
    let terminal = COMMAND_TERMINAL_OR_ACCESSOR.contains(&callee);
    let args_are_borrowed = ((!consumes && !move_by_ffi && prefix_borrow) || terminal) && !builder;

    CalleeOwnership {
        result,
        borrows_first_arg,
        arg_transfer,
        args_are_borrowed,
    }
}

/// Is `type_name::method_name` a static (no-`self`) constructor that dispatches
/// directly to a runtime symbol? Single source for the static-vs-instance
/// decision shared by `util.rs::is_builtin_static_method` and the
/// `method_call.rs` fast-path gate. Reconciles the two formerly-diverged lists
/// (util.rs:74-132 + method_call.rs:162-214) into one declarative table.
///
/// RECONCILIATION DECISIONS (recorded per the plan's obligation):
///  1. The universal `.new` rule: `method_name == "new"` is a static ctor for
///     ANY type. This is the behaviour `method_call.rs::is_collection_ctor`
///     actually ships (line 197); `util.rs` did NOT have it. The union keeps
///     it — verified against ffi_alias_single_entry + the broad suite that it
///     does not reroute a user class with a real `init` (the method_call.rs
///     fast path still gates on `lookup_ffi_alias` / the base-type allowlist
///     downstream, and the field_access/method_call `is_static` checks OR in
///     `is_user_static_method`, so a user `.new` with an init still resolves
///     correctly).
///  2. `String.from/from_bytes/from_iter`, `Thread.*`, `Mutex/Arc/SharedSync.new`
///     came from util.rs; `File.*`, `Duration.*`, `Instant.now`, `Tcp*`,
///     `BufReader/BufWriter` came from method_call.rs. The union is the set both
///     sites now read. Widening each site to the union is behaviour-preserving
///     for the observable compilation outcome (proven by the integration suite
///     in Task 5): the extra (type, method) pairs a given site gains were
///     already classified static at the OTHER site, and both sites funnel a
///     positive answer through the same downstream FFI-alias / `_init` gates.
pub fn is_static_constructor(type_name: &str, method_name: &str) -> bool {
    // Any `.new` is a static constructor (method_call.rs is_collection_ctor rule).
    if method_name == "new" {
        return true;
    }
    let base = match type_name.find('[') {
        Some(pos) => &type_name[..pos],
        None => type_name,
    };
    STATIC_CTORS
        .iter()
        .any(|(b, methods)| *b == base && methods.contains(&method_name))
}

/// (base_type, static-constructor method names). UNION of the two formerly
/// duplicated lists (util.rs:74-132 + method_call.rs:162-214). One source.
/// `new` is handled by the universal rule in `is_static_constructor`, so it is
/// omitted here.
const STATIC_CTORS: &[(&str, &[&str])] = &[
    ("String", &["from", "with_capacity", "from_iter", "from_bytes"]),
    ("Vec", &["with_capacity", "from_iter"]),
    ("Array", &["with_capacity", "from_iter"]),
    ("Hash", &["with_capacity", "from_iter"]),
    ("HashMap", &["with_capacity", "from_iter"]),
    ("Map", &["with_capacity", "from_iter"]),
    ("Set", &["with_capacity", "from_iter"]),
    ("HashSet", &["with_capacity", "from_iter"]),
    ("Thread", &["spawn", "current", "sleep", "yield_now"]),
    // Mutex / Arc / SharedSync: only `.new`, handled by the universal rule.
    ("Duration", &["from_secs", "from_millis", "from_micros", "from_nanos"]),
    ("Instant", &["now"]),
    ("TcpListener", &["bind"]),
    ("TcpStream", &["connect"]),
    ("File", &["open", "create", "append", "open_options"]),
    ("BufReader", &["with_capacity"]),
    ("BufWriter", &["with_capacity"]),
];

/// Old `transfer_indices` + `is_pointer_store_helper`, folded into one mask.
fn arg_transfer_mask(callee: &str) -> ArgMask {
    if callee == "ruxen_store_ptr" {
        return ArgMask::single(1);
    }
    let m = extract_method_name(callee);
    let is_vec = callee.starts_with("Vec_")
        || callee.starts_with("Vec[")
        || callee.starts_with("Array_")
        || callee.starts_with("Array[");
    let is_hash = callee.starts_with("Hash_")
        || callee.starts_with("Hash[")
        || callee.starts_with("HashMap_")
        || callee.starts_with("HashMap[")
        || callee.starts_with("Map_")
        || callee.starts_with("Map[");
    let is_set = callee.starts_with("Set_")
        || callee.starts_with("Set[")
        || callee.starts_with("HashSet_")
        || callee.starts_with("HashSet[");
    match (is_vec, is_hash, is_set, m) {
        (true, _, _, "push") => ArgMask::single(1),
        (true, _, _, "insert") => ArgMask::single(2),
        (_, true, _, "insert") => ArgMask::pair(1, 2),
        (_, _, true, "insert") => ArgMask::single(1),
        _ if callee == "ruxen_vec_push" => ArgMask::single(1),
        _ if callee == "ruxen_vec_insert" => ArgMask::single(2),
        _ if callee == "ruxen_hash_insert" => ArgMask::pair(1, 2),
        _ if callee == "ruxen_set_insert" => ArgMask::single(1),
        _ => ArgMask::none(),
    }
}

/// Old prefix list inside `is_runtime_borrow_helper` (drops.rs:1091-1107).
fn has_runtime_prefix(c: &str) -> bool {
    c.starts_with("ruxen_")
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
        || c.starts_with("&str_")
}

// ---------------------------------------------------------------------------
// The declarative tables. 1:1 move of the literals formerly inline in
// drops.rs (line ranges noted). Not invented — transcribed verbatim.
// ---------------------------------------------------------------------------

/// Callees returning a fresh heap allocation owned by `dest`.
/// Old: drops.rs:694-934 `FRESH_ALLOC_CALLEES`.
const FRESH_ALLOC_CALLEES: &[&str] = &[
    "String_from",
    "Vec_new",
    "Hash_new",
    "HashMap_new",
    "ruxen_string_from",
    "ruxen_string_concat",
    "ruxen_vec_new",
    "ruxen_hash_new",
    // `String_into_bytes` / `ruxen_string_into_bytes` are in BOTH this list and
    // CONSUME_HELPERS, and this is NOT a contradiction — they are load-bearing
    // in two INDEPENDENT axes the drop pass tracks separately:
    //   * RESULT (dest): into_bytes produces a fresh Vec[U8] spine the caller
    //     owns. Without Fresh here, the dest is tainted, scope-exit drop emits
    //     no `ruxen_vec_free`, and the Vec struct + data buffer LEAK (verified:
    //     drop_fixtures::string_into_bytes_transfers_ownership goes to
    //     raw_outstanding=2, vec_frees=0 the moment this membership is removed).
    //   * ARG (arg0): the runtime frees the source `char*` internally, so the
    //     source String must be tainted (consume) to avoid a double-free.
    // The original "fresh-vs-consume contradiction" hypothesis (Phase 2 plan
    // Task 3b) was disproven by the leak backstop: the result and arg axes are
    // orthogonal, and BOTH classifications are required. Kept as-is.
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

/// The non-`_init` members of the old `borrows_first_arg` rule
/// (drops.rs:968-984). The `_init` suffix stays as an `ends_with` check in
/// `callee_ownership`.
const FREE_FAMILY: &[&str] = &[
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
];

/// Old `is_runtime_consume_helper` (drops.rs:1003-1043).
const CONSUME_HELPERS: &[&str] = &[
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
    "ruxen_string_into_bytes",
    "String_into_bytes",
    "ruxen_vec_from_iter",
    "Vec_from_iter",
];

/// Old `is_move_by_ffi_callee` (drops.rs:1068-1088).
const MOVE_BY_FFI: &[&str] = &[
    "ruxen_executor_spawn",
    "Task_spawn_raw",
    "ruxen_thread_spawn",
    "Thread_spawn",
    "Thread_spawn_raw",
];

/// Old `is_command_builder_method` (drops.rs:1158-1168).
const COMMAND_BUILDER: &[&str] = &[
    "Command_arg",
    "ruxen_command_arg",
    "Command_args",
    "ruxen_command_args",
    "Command_env",
    "ruxen_command_env",
    "Command_current_dir",
    "ruxen_command_current_dir",
];

/// Old `is_command_terminal_or_accessor` (drops.rs:1123-1139).
const COMMAND_TERMINAL_OR_ACCESSOR: &[&str] = &[
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
    "ExitStatus_code",
    "ruxen_exit_status_code",
    "ExitStatus_success",
    "ruxen_exit_status_success",
];

#[cfg(test)]
mod tests {
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
    fn vec_push_transfers_arg1() {
        assert_eq!(
            callee_ownership("ruxen_vec_push").arg_transfer,
            ArgMask::single(1)
        );
        assert_eq!(callee_ownership("Vec_push").arg_transfer, ArgMask::single(1));
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
    fn pointer_store_transfers_arg1() {
        assert_eq!(
            callee_ownership("ruxen_store_ptr").arg_transfer,
            ArgMask::single(1)
        );
    }

    #[test]
    fn user_callee_is_fully_conservative() {
        let o = callee_ownership("MyClass_method");
        assert_eq!(o.result, ResultOwnership::None); // dest tainted
        assert!(!o.args_are_borrowed); // every Use(arg) tainted
        assert!(!o.borrows_first_arg);
    }

    #[test]
    fn static_constructor_union_is_reconciled() {
        // From the method_call.rs fast-path cascade:
        assert!(is_static_constructor("File", "open"));
        assert!(is_static_constructor("Duration", "from_secs"));
        assert!(is_static_constructor("Instant", "now"));
        assert!(is_static_constructor("TcpListener", "bind"));
        assert!(is_static_constructor("TcpStream", "connect"));
        assert!(is_static_constructor("BufReader", "new"));
        assert!(is_static_constructor("BufWriter", "with_capacity"));
        // From util.rs::is_builtin_static_method (the diverged sibling):
        assert!(is_static_constructor("String", "from"));
        assert!(is_static_constructor("String", "from_bytes"));
        assert!(is_static_constructor("Thread", "spawn"));
        assert!(is_static_constructor("Mutex", "new"));
        assert!(is_static_constructor("Arc", "new"));
        assert!(is_static_constructor("SharedSync", "new"));
        // Generic-base handling (Vec[T] → Vec):
        assert!(is_static_constructor("Vec[String]", "with_capacity"));
        assert!(is_static_constructor("HashMap[K, V]", "from_iter"));
        // The universal `new` rule that method_call.rs's is_collection_ctor
        // applies to ANY type:
        assert!(is_static_constructor("AnyUserClass", "new"));
        // Negatives:
        assert!(!is_static_constructor("Vec", "len"));
        assert!(!is_static_constructor("File", "read"));
    }
}
