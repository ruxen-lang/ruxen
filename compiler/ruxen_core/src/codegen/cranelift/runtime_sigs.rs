//! Compiler-internal runtime function signatures (the ABI residual).
//!
//! After `docs/specs/system/zero_rust_stdlib_classes.spec.md`, the C-ABI
//! of every `ruxen_*` symbol that a stdlib `.rx` `lib` block declares is
//! DERIVED from that declaration (resolver records the declared `Ty`s;
//! `ty_to_cranelift` lowers them; codegen Pass-0 declares the
//! `Linkage::Import` from that). The hand-rolled per-symbol table that
//! used to mirror those widths is gone.
//!
//! What remains here is the IRREDUCIBLE residual: symbols emitted
//! DIRECTLY by codegen / MIR lowering, declared by NO `.rx` lib block, so
//! there is no declared `Ty` to derive their ABI from. These are language
//! features (allocation, equality/ordering lowering, panic, the formatter
//! synthesis surface, pointer-indirection helpers, the no-op sentinel)
//! plus a handful of conversion/accessor helpers the compiler calls
//! implicitly. The parity guard
//! `compiler/ruxen_core/tests/runtime_abi_derivation.rs` proves this set
//! is exactly the table-minus-derived difference at the migration
//! baseline (74 symbols, incl. the non-`ruxen_` I/O aliases).
//!
//! Consumers:
//!   * `cranelift/emit.rs::coerce_call_args` — consulted only as a
//!     FALLBACK, after the derived `user_fn_param_tys` (codegen Pass-0
//!     populated that from the lib-decl `Ty`s, so derived wins for every
//!     `.rx`-declared symbol). This table answers for the residual.
//!   * `cranelift/translation_env.rs::get_or_declare_func` — the path for
//!     a callee never declared in Pass-0/1, i.e. a residual symbol emitted
//!     mid-body.
//!
//! Adding a stdlib runtime function NO LONGER touches this file: declare
//! it in the package's `.rx` + `.c` and its ABI is derived. Only a NEW
//! compiler-emitted-but-undeclared symbol belongs here.

use cranelift_codegen::ir::types::{self, Type};

/// Compiler-internal runtime signatures (the residual that cannot be
/// derived from a `.rx` lib declaration). Returns `(param_types,
/// optional_return_type)`.
///
/// `pub` (re-exported from `codegen::cranelift` as `runtime_signature` for
/// source compatibility) so any external consumer keeps resolving; the
/// REPL JIT derives FFI widths from `HirFfiFunc::param_types` itself and
/// does not call this.
pub fn compiler_internal_signature(name: &str) -> Option<(Vec<Type>, Option<Type>)> {
    match name {
        // ── Implicit I/O entry points ────────────────────────────────
        // The bare verbs (`puts`, `print`, …) and their `ruxen_*` forms
        // are emitted by `print`/`puts` statement lowering and by the
        // string-interpolation path, not declared in a `.rx` lib block.
        "puts" | "ruxen_puts" => Some((vec![types::I64], None)),
        "eputs" | "ruxen_eputs" => Some((vec![types::I64], None)),
        "print" | "ruxen_print" => Some((vec![types::I64], None)),
        "println" => Some((vec![types::I64], None)),
        "eprintln" => Some((vec![types::I64], None)),
        "read_line" | "ruxen_read_line" => Some((vec![], Some(types::I64))),
        "stdin" | "ruxen_stdin" => Some((vec![], Some(types::I64))),
        "stdout" | "ruxen_stdout" => Some((vec![], Some(types::I64))),
        "stderr" | "ruxen_stderr" => Some((vec![], Some(types::I64))),
        "ruxen_print_int" => Some((vec![types::I64], None)),
        // `main` prologue calls this directly (see cranelift/mod.rs
        // entry-block setup); never `.rx`-declared.
        "ruxen_env_init" => Some((vec![types::I32, types::I64], None)),
        "ruxen_env_args_count" => Some((vec![], Some(types::I64))),
        "ruxen_env_args_at" => Some((vec![types::I64], Some(types::I64))),
        "args" | "ruxen_env_args" => Some((vec![], Some(types::I64))),

        // ── Implicit conversions (interpolation / `to_s` synthesis) ──
        "ruxen_int_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_float_to_string" => Some((vec![types::F64], Some(types::I64))),
        "ruxen_bool_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_char_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_int_to_f" => Some((vec![types::I64], Some(types::F64))),
        "ruxen_float_to_i" => Some((vec![types::F64], Some(types::I64))),

        // ── Formatter synthesis surface (Debug/Display derive) ───────
        // Emitted by `_fmt` helper synthesis (Phase 2 #06.D4); the spec
        // helpers take an internal spec struct the surface `.rx` never
        // names.
        "ruxen_float_to_string_prec" => Some((vec![types::F64, types::I64], Some(types::I64))),
        "ruxen_string_truncate_chars" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_new_with_spec" => Some((
            vec![types::I64, types::I64, types::I64, types::I64],
            Some(types::I64),
        )),
        "ruxen_fmt_formatter_precision" => Some((vec![types::I64], Some(types::I64))),

        // ── Equality / ordering / hashing lowering (`==`, `<=>`) ─────
        // `cranelift/emit` and `llvm/emit/instructions` emit these for
        // the `Compare` instruction on string/collection operands.
        "ruxen_string_concat" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_cmp" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_hash" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_from_iter" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_str_split" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_str_parse_uint" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_eq" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "ruxen_hash_index" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_set_eq" => Some((vec![types::I64, types::I64], Some(types::I8))),

        // ── Indexing / element access emitted by `a[i]` lowering ─────
        // `Array#get` is `.rx`-declared as `ruxen_vec_get_opt`; the bare
        // `ruxen_vec_get` (+ mut variants) are the panic-on-OOB forms the
        // index operator lowers to directly.
        "ruxen_vec_get" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_get_mut" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_get_mut_opt" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_set" => Some((vec![types::I64, types::I64, types::I64], None)),
        "ruxen_vec_from_iter" => Some((vec![types::I64], Some(types::I64))),

        // ── Pointer indirection helpers (`&mut T` mutation lowering) ──
        "ruxen_deref_ptr" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_store_ptr" => Some((vec![types::I64, types::I64], None)),

        // ── Allocation + panic (core language runtime) ───────────────
        "ruxen_alloc" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_dealloc" => Some((vec![types::I64], None)),
        "ruxen_realloc" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_panic" => Some((vec![types::I64], None)),

        // ── Drop-glue selectors emitted by the MIR drop pass ─────────
        // Per-element drop helpers the drop elaboration chooses by the
        // local's element type; no `.rx` decl names them.
        "ruxen_string_free" => Some((vec![types::I64], None)),
        "ruxen_vec_free" => Some((vec![types::I64], None)),
        "ruxen_hash_free" => Some((vec![types::I64], None)),
        "ruxen_set_free" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_string_v" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_v_string" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_string_string" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_v_vec" => Some((vec![types::I64], None)),
        "ruxen_set_drop_string" => Some((vec![types::I64], None)),
        "ruxen_vec_drop_string" => Some((vec![types::I64], None)),
        "ruxen_vec_drop_vec" => Some((vec![types::I64], None)),

        // ── Result `?`-operator + closure-fallback helpers ───────────
        "ruxen_result_try_op" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_result_unwrap_or_else" => Some((vec![types::I64, types::I64], Some(types::I64))),

        // ── Implicit accessor/conversion helpers without a `.rx` decl ─
        // FOLLOW-UP (coverage gap, not a language primitive): the
        // Duration `as_*` accessors, fs `Metadata` accessors, regex
        // `compile_const`/`drop`/`error_drop`, `match_drop`, the
        // ExitStatus accessors, and the thread-sleep helper below are
        // stdlib runtime functions the compiler currently emits
        // implicitly rather than routing through an `.rx`-declared
        // method. They are residual only because no `.rx` lib block
        // declares the linked symbol; a follow-up should declare them so
        // they too become derived. See the migration report's residual
        // bucketing.
        "ruxen_duration_as_secs"
        | "ruxen_duration_as_millis"
        | "ruxen_duration_as_micros"
        | "ruxen_duration_as_nanos" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_thread_sleep_ns" => Some((vec![types::I64], None)),
        "ruxen_metadata_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_modified" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_is_file" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_is_dir" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_is_symlink" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_exit_status_code" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_exit_status_success" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_regex_compile_const" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_regex_drop" => Some((vec![types::I64], None)),
        "ruxen_regex_error_drop" => Some((vec![types::I64], None)),
        "ruxen_match_drop" => Some((vec![types::I64], None)),

        // ── No-op sentinel (declared via call-site inference) ────────
        "ruxen_noop" => Some((vec![], None)),
        _ => None,
    }
}
