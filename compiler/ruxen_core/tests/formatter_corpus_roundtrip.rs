// Corpus round-trip test: the formatter must be in sync with parser semantics.
// For every `.rx` file the parser currently accepts, `format()` must:
//   1. produce no errors,
//   2. produce output that STILL PARSES (semantic sync — no retired syntax),
//   3. parse to the same AST as the input, ignoring spans,
//   4. be idempotent: format(format(x)) == format(x).
use ruxen_core::formatter;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use std::path::{Path, PathBuf};

/// Parse `src` and return its AST's `Debug` text with every `Span`
/// replaced by a bare `Span`, so two parses compare by structure only.
fn parses(src: &str) -> Result<String, String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().map_err(|d| {
        format!(
            "lex: {:?}",
            d.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        )
    })?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|d| {
        format!(
            "parse: {:?}",
            d.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        )
    })?;
    Ok(strip_spans(&format!("{program:?}")))
}

/// `Span` has only integer fields, so each `Span { .. }` ends at the
/// first `}` after it.
fn strip_spans(debug: &str) -> String {
    const OPEN: &str = "Span { start: ";
    let mut out = String::with_capacity(debug.len());
    let mut rest = debug;
    while let Some(pos) = rest.find(OPEN) {
        out.push_str(&rest[..pos]);
        out.push_str("Span");
        let after = &rest[pos..];
        let close = after
            .find('}')
            .expect("unterminated Span in AST debug output");
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Files whose formatted output parses to a different AST, or no longer
/// parses at all, today. Each one is a known formatter bug (audit family
/// F-5: interpolation re-emitted from raw tokens, dropped default
/// arguments, retired-token spellings, `do` blocks rewritten as brace
/// blocks, reordered doc comments). Fixing F-5
/// empties this list; a listed file that round-trips cleanly fails the test
/// so the list only shrinks.
const KNOWN_AST_CHANGES: &[&str] = &[
    "library/std/array/src/lib.rx",
    "library/std/core/src/lib.rx",
    "library/std/future/src/queue.rx",
    "library/std/future/src/schedule.rx",
    "library/std/future/src/scope.rx",
    "library/std/future/src/stream.rx",
    "library/std/map/src/lib.rx",
    "library/std/scalar/src/lib.rx",
    "library/std/set/src/lib.rx",
    "library/std/string/src/string.rx",
    "tests/release-e2e/cases/1003_float_erased_word_boundaries.rx",
    "tests/release-e2e/cases/100_closure_method_heap_return.rx",
    "tests/release-e2e/cases/1073_string_each_char_byte_line.rx",
    "tests/release-e2e/cases/1080_string_swapcase_chomp_chop.rx",
    "tests/release-e2e/cases/1101_string_regex_bridge.rx",
    "tests/release-e2e/cases/1121_integer_number_theory.rx",
    "tests/release-e2e/cases/1130_integer_iteration.rx",
    "tests/release-e2e/cases/1170_char_scalar_methods.rx",
    "tests/release-e2e/cases/1840_spaceship_cross_class_borrow.rx",
    "tests/release-e2e/cases/1921_array_float_elem_methods.rx",
    "tests/release-e2e/cases/413_scope_drop_cancels_pending_child.rx",
    "tests/release-e2e/cases/420_race_drop_cancels_both_arms.rx",
    "tests/release-e2e/cases/429_stream_source_drain.rx",
    "tests/release-e2e/cases/430_stream_task_backed_pump.rx",
    "tests/release-e2e/cases/431_stream_map.rx",
    "tests/release-e2e/cases/432_stream_filter.rx",
    "tests/release-e2e/cases/433_stream_take_chain.rx",
    "tests/release-e2e/cases/434_stream_drop_on_cancel.rx",
    "tests/release-e2e/cases/435_stream_adapter_fuse.rx",
    "tests/release-e2e/cases/436_stream_chain_drop.rx",
    "tests/release-e2e/cases/437_testclock_advance_fires_sleep.rx",
    "tests/release-e2e/cases/438_testclock_advance_to_next_set.rx",
    "tests/release-e2e/cases/440_testclock_set_absolute.rx",
    "tests/release-e2e/cases/441_testclock_guard_restore.rx",
    "tests/release-e2e/cases/444_schedule_sync_retry.rx",
    "tests/release-e2e/cases/445_retry_async_testclock.rx",
    "tests/release-e2e/cases/446_retry_drop_mid_backoff.rx",
    "tests/release-e2e/cases/447_retry_while_input.rx",
    "tests/release-e2e/cases/448_retry_and_then_budget.rx",
    "tests/release-e2e/cases/449_retry_jittered_bound.rx",
    "tests/release-e2e/cases/460_parenless_no_arg_init_mixin_field.rx",
    "tests/release-e2e/cases/473_any_future_call_no_poison.rx",
    "tests/release-e2e/cases/474_generic_method_mixin_mono.rx",
    "tests/release-e2e/cases/475_closure_capture_no_leak.rx",
    "tests/release-e2e/cases/507_fs_metadata.rx",
    "tests/release-e2e/cases/510_file_open_read_to_string.rx",
    "tests/release-e2e/cases/512_file_append.rx",
    "tests/release-e2e/cases/514_file_seek_start.rx",
    "tests/release-e2e/cases/515_file_seek_end.rx",
    "tests/release-e2e/cases/516_file_metadata.rx",
    "tests/release-e2e/cases/518_file_drop_closes.rx",
    "tests/release-e2e/cases/530_fs_copy.rx",
    "tests/release-e2e/cases/531_fs_rename.rx",
    "tests/release-e2e/cases/533_fs_remove_dir_all.rx",
    "tests/release-e2e/cases/534_fs_canonicalize.rx",
    "tests/release-e2e/cases/535_fs_write_atomic.rx",
    "tests/release-e2e/cases/536_fs_read_link.rx",
    "tests/release-e2e/cases/537_fs_symlink.rx",
    "tests/release-e2e/cases/539_fs_create_dir_all_existing_file_conflict.rx",
    "tests/release-e2e/cases/543_tcp_stream_shutdown.rx",
    "tests/release-e2e/cases/570_bufreader_file_read_line.rx",
    "tests/release-e2e/cases/605_generic_self_ffi_method.rx",
    "tests/release-e2e/cases/612_set_enumerable.rx",
    "tests/release-e2e/cases/613_hash_enumerable.rx",
    "tests/release-e2e/cases/614_unwrap_or_else.rx",
    "tests/release-e2e/cases/642_free_fn_do_block.rx",
    "tests/release-e2e/cases/644_overload_do_block.rx",
    "tests/release-e2e/cases/653_f32_negative_int_literal_compare.rx",
    "tests/release-e2e/cases/655_generic_fn_mixin_two_implementors.rx",
    "tests/release-e2e/cases/656_generic_fn_calling_generic_fn.rx",
    "tests/release-e2e/cases/657_generic_fn_mixin_default_body.rx",
    "tests/release-e2e/cases/658_generic_fn_mixin_single_implementor.rx",
    "tests/release-e2e/cases/659_generic_fn_mixin_struct_implementor.rx",
    "tests/release-e2e/cases/721_async_def_no_await_handpoll.rx",
    "tests/release-e2e/cases/722_async_def_chained_await_handpoll.rx",
    "tests/release-e2e/cases/802_optional_args_root.rx",
    "tests/release-e2e/cases/804_method_overload_defaults.rx",
    "tests/release-e2e/cases/908_block_decl_yield.rx",
    "tests/release-e2e/cases/909_block_defined_and_yield_value.rx",
    "tests/release-e2e/cases/910_block_call_and_capture.rx",
    "tests/release-e2e/cases/911_generic_fn_with_block.rx",
    "tests/release-e2e/cases/912_block_backcompat_and_method.rx",
    "tests/release-e2e/cases/917_alias_generic_free_fn.rx",
    "tests/release-e2e/cases/920_yield_method_free_fn_name_collision.rx",
    "tests/release-e2e/cases/921_block_optional_method_parenless.rx",
    "tests/release-e2e/cases/923_clone_on_borrow_owns.rx",
    "tests/release-e2e/cases/936_enumerable_capture_return.rx",
    "tests/release-e2e/cases/93_yield_block.rx",
    "tests/release-e2e/cases/946_block_forwarding.rx",
    "tests/release-e2e/cases/952_power_operator.rx",
    "tests/release-e2e/cases/965_comparable_spaceship.rx",
    "tests/release-e2e/cases/971_integer_times.rx",
    "tests/release-e2e/cases/973_tap_then.rx",
    "tests/release-e2e/cases/976_enumerable_ruby_aliases.rx",
    "tests/release-e2e/cases/977_user_tap_wins.rx",
    "tests/release-e2e/cases/978_tap_unit_receiver.rx",
    "tests/release-e2e/cases/979_tap_then_as_puts_arg.rx",
    "tests/release-e2e/cases/980_mixin_default_block_self_param.rx",
    "tests/release-e2e/cases/981_mixin_default_block_generic_return.rx",
    "tests/release-e2e/cases/982_user_class_enumerable_map.rx",
    "tests/release-e2e/cases/983_kernel_tap_then_generic_heads.rx",
    "tests/release-e2e/cases/984_builtin_head_operators.rx",
    "tests/release-e2e/cases/985_enumerable_find_all_none_one.rx",
    "tests/release-e2e/cases/986_hash_key_and_each.rx",
    "tests/release-e2e/cases/990_alias_generic_builtin_head.rx",
];

fn collect_rx(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_rx(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rx") {
                out.push(p);
            }
        }
    }
}

#[test]
fn formatter_roundtrips_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut files = Vec::new();
    collect_rx(&root.join("library/std"), &mut files);
    collect_rx(&root.join("tests/release-e2e/cases"), &mut files);
    files.sort();

    let mut failures: Vec<String> = Vec::new();
    let mut tested = 0usize;
    let mut known_seen: Vec<String> = Vec::new();

    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Baseline: only test files the parser already accepts.
        let Ok(ast_before) = parses(&src) else {
            continue;
        };
        tested += 1;
        let rel = f.strip_prefix(root).unwrap_or(f).display();
        let rel_str = rel.to_string();

        let r1 = formatter::format(&src);
        if !r1.errors.is_empty() {
            failures.push(format!("{rel}: format errors: {:?}", r1.errors));
            continue;
        }
        // Semantic sync: formatted output must still parse.
        let ast_after = match parses(&r1.output) {
            Ok(ast) => ast,
            Err(_) if KNOWN_AST_CHANGES.contains(&rel_str.as_str()) => {
                known_seen.push(rel_str);
                continue;
            }
            Err(e) => {
                failures.push(format!("{rel}: formatted output no longer parses: {e}"));
                continue;
            }
        };
        // Semantic sync: formatting must not change the program.
        if ast_after != ast_before {
            if KNOWN_AST_CHANGES.contains(&rel_str.as_str()) {
                // Output is already wrong; idempotency of it says nothing.
                known_seen.push(rel_str);
                continue;
            } else {
                let at = ast_before
                    .bytes()
                    .zip(ast_after.bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or(ast_before.len().min(ast_after.len()));
                let near = |s: &str| {
                    s.get(at.saturating_sub(40)..(at + 80).min(s.len()))
                        .unwrap_or("")
                        .to_string()
                };
                failures.push(format!(
                "{rel}: formatted output parses to a different AST\n    before: {}\n    after:  {}",
                near(&ast_before),
                near(&ast_after)
            ));
                continue;
            }
        }
        // Idempotency.
        let r2 = formatter::format(&r1.output);
        if r2.output != r1.output {
            failures.push(format!("{rel}: not idempotent (format twice differs)"));
        }
    }

    for known in KNOWN_AST_CHANGES {
        if !known_seen.iter().any(|k| k == known) {
            failures.push(format!(
                "{known}: listed in KNOWN_AST_CHANGES but no longer changes AST; remove it"
            ));
        }
    }

    eprintln!(
        "formatter corpus: tested {tested} files, {} failures",
        failures.len()
    );
    if !failures.is_empty() {
        let shown: Vec<_> = failures.iter().take(200).cloned().collect();
        panic!(
            "{} formatter round-trip failures (showing {}):\n{}",
            failures.len(),
            shown.len(),
            shown.join("\n")
        );
    }
}
