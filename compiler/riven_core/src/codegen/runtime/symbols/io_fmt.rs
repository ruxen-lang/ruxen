//! I/O, fmt::Formatter, env, process runtime symbols.

// I/O surface (stdin/stdout/stderr + flat puts/print/eputs entry points).
pub(in crate::codegen::runtime) const IO: &[&str] = &[
    "riven_puts",
    "riven_print",
    "riven_eputs",
    "riven_read_line",
    "riven_stdin",
    "riven_stdout",
    "riven_stderr",
    "riven_stdin_read_line",
    "riven_stdin_read_to_string",
    "riven_stdin_lines",
    "riven_stdout_write_str",
    "riven_stdout_flush",
    "riven_stderr_write_str",
    "riven_stderr_flush",
    // Phase 2 stdlib (#06.1): Stdout / Stderr convenience methods.
    "riven_stdout_print",
    "riven_stdout_println",
    "riven_stderr_eprint",
    "riven_stderr_eprintln",
];

// std::fmt::Formatter buffer surface (#06.A3 / #06.D4).
pub(in crate::codegen::runtime) const FMT: &[&str] = &[
    "riven_fmt_formatter_new",
    "riven_fmt_formatter_free",
    "riven_fmt_formatter_write_str",
    "riven_fmt_formatter_write_char",
    "riven_fmt_formatter_buffer",
    "riven_fmt_formatter_len",
    // Phase 2 stdlib (#06.D4): spec-aware constructor + precision
    // accessor + per-type precision helpers used by the synth `_fmt`
    // bodies.
    "riven_fmt_formatter_new_with_spec",
    "riven_fmt_formatter_precision",
    "riven_float_to_string_prec",
    "riven_string_truncate_chars",
];

// std::env + std::process surface.
pub(in crate::codegen::runtime) const ENV_PROCESS: &[&str] = &[
    "riven_env_init",
    "riven_env_args_count",
    "riven_env_args_at",
    "riven_env_args",
    "riven_env_var",
    // Phase 2 stdlib (#06): env / fs additions.
    "riven_env_vars",
    "riven_env_current_dir",
    // Phase 2 stdlib (#06): std::process::Command builder + Output /
    // ExitStatus accessor surface. Wire layouts documented in
    // `runtime.c` at `riven_command_new`.
    "riven_command_new",
    "riven_command_arg",
    "riven_command_args",
    "riven_command_env",
    "riven_command_current_dir",
    "riven_command_status",
    "riven_command_output",
    "riven_command_drop",
    "riven_exit_status_code",
    "riven_exit_status_success",
    "riven_exit_status_free",
    "riven_output_stdout",
    "riven_output_stderr",
    "riven_output_status",
    "riven_output_drop",
    "riven_process_exit",
    // std::process::run (Phase 3): fork+execvp a child, inherit stdio,
    // return exit code (or 128+signal on signal termination, 127 on
    // fork/exec failure).
    "riven_process_run",
];
