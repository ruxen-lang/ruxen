//! The ruxen_core integration tests, one binary (one link, one process).
//!
//! Each file in this directory is a module. Run one with
//! `cargo test -p ruxen_core --test it -- <module>::`. Two suites stay
//! separate binaries: `release_e2e_smoke` (CI calls it by name) and
//! `trio_leak_pin` (it opts its whole process into the test-only stdlib).
//!
//! Tests here share one process: never mutate process-wide state
//! (environment variables, the current directory). Pass paths explicitly.

#[path = "../common/mod.rs"]
mod common;

mod alias_keyword;
mod async_lowering;
mod async_negative;
mod async_surface;
mod auto_derive_send_sync;
mod autocall_fn_references;
#[cfg(feature = "llvm")]
mod backend_parity;
mod bang_names_are_calls;
mod block_tier2;
mod blocks_capture_moves;
mod blocks_procs_lambdas;
mod bootstrap_prelude_merge;
mod borrow_check_pattern_moves;
mod borrow_check_reborrow;
mod borrowed_places;
mod codegen_unknown_method_rejected;
mod concurrency_markers;
mod concurrency_negative;
mod const_generics;
mod cross_compile_triples;
mod drop_fixtures;
mod drop_name_sweep;
mod drop_overload_return_type;
mod error_code_registry;
mod ffi_c_symbol_wired;
mod field_ownership;
mod file_class_layout_stability;
mod formatter_lib_doc_comments;
mod implicit_codes_registered;
mod implicit_diagnostics;
mod include_non_mixin;
mod inner_first_shadowing;
mod io_error_tag_stability;
mod layout_directives;
mod lexer_keywords_consistency;
mod lib_c_symbol_alias;
mod lib_in_class_body_wired;
mod linker_system_libs;
#[cfg(feature = "llvm")]
mod llvm_backend;
#[cfg(feature = "llvm")]
mod llvm_debug_info;
mod method_call_inference;
mod mir_debug_spans;
mod mir_loan_check;
mod mixin_vtables;
mod mono_map_literal_value;
mod nil_as_unit_type;
mod no_silent_stubs;
mod no_std_e1400;
mod overflow_tag_stability;
mod overload_arg_fits_none;
mod parse_def_with_semicolons;
mod parser_feature_parity;
mod phase7_ffi;
mod predicate_suffix_hint;
mod q17_generic_fn_mixin_mono;
mod q23_fmt_nondestructive;
mod q25_hash_set_soundness;
mod q34_fmt_grouping_parens;
mod qualified_class_includes_qualified_mixin;
mod qualified_method_dispatches_to_aliased_c_symbol;
mod qualified_type_registry_insertion;
mod refmut_handle_rebind;
mod regex_typeck;
mod render_fn_signature;
mod root_anchor_type_path;
mod ruby_block_semantics;
mod runtime_abi_derivation;
mod runtime_abi_parity;
mod runtime_safety;
mod send_bound_hint;
mod shutdown_tag_stability;
mod static_constructor_init_route;
mod std_use_resolution;
mod stdin_new_diagnostic;
mod stdlib_array_negatives;
mod stdlib_bootstrap;
mod stdlib_bufio;
mod stdlib_io;
mod stdlib_iterator;
mod stdlib_json;
mod stdlib_map_negatives;
mod stdlib_net;
mod stdlib_primitives;
mod stdlib_process;
mod stdlib_string_negatives;
mod stdlib_test;
mod string_literal_wrap;
mod sync_extras_typeck;
mod syntax_parity;
mod tagged_union_alloc_size;
mod task_scheduler;
mod task_spawn_ownership;
mod test_layout;
mod trait_bound_enforcement;
mod tuple_field_roundtrip;
mod type_name_collision;
mod typed_ffi_returns;
mod user_modules;
mod user_runtime_c;
mod variance;
#[cfg(feature = "llvm")]
mod wasm_codegen;
mod yield_self_block_dsl;
