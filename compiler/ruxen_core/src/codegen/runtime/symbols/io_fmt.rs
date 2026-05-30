//! I/O, fmt::Formatter, env, process runtime symbols.

// I/O surface (stdin/stdout/stderr + flat puts/print/eputs entry points).
pub(in crate::codegen::runtime) const IO: &[&str] = &[
    "ruxen_puts",
    "ruxen_print",
    "ruxen_eputs",
    "ruxen_read_line",
    "ruxen_stdin",
    "ruxen_stdout",
    "ruxen_stderr",
    "ruxen_stdin_read_line",
    "ruxen_stdin_read_to_string",
    "ruxen_stdin_lines",
    "ruxen_stdout_write_str",
    "ruxen_stdout_flush",
    "ruxen_stderr_write_str",
    "ruxen_stderr_flush",
    // Phase 2 stdlib (#06.1): Stdout / Stderr convenience methods.
    "ruxen_stdout_print",
    "ruxen_stdout_println",
    "ruxen_stderr_eprint",
    "ruxen_stderr_eprintln",
];

// std::fmt::Formatter buffer surface (#06.A3 / #06.D4).
pub(in crate::codegen::runtime) const FMT: &[&str] = &[
    "ruxen_fmt_formatter_new",
    "ruxen_fmt_formatter_free",
    "ruxen_fmt_formatter_write_str",
    "ruxen_fmt_formatter_write_char",
    "ruxen_fmt_formatter_buffer",
    "ruxen_fmt_formatter_len",
    // Phase 2 stdlib (#06.D4): spec-aware constructor + precision
    // accessor + per-type precision helpers used by the synth `_fmt`
    // bodies.
    "ruxen_fmt_formatter_new_with_spec",
    "ruxen_fmt_formatter_precision",
    "ruxen_float_to_string_prec",
    "ruxen_string_truncate_chars",
];

// std::env + std::process surface.
pub(in crate::codegen::runtime) const ENV_PROCESS: &[&str] = &[
    "ruxen_env_init",
    "ruxen_env_args_count",
    "ruxen_env_args_at",
    "ruxen_env_args",
    "ruxen_env_var",
    // Phase 2 stdlib (#06): env / fs additions.
    "ruxen_env_vars",
    "ruxen_env_current_dir",
    // Phase 2 stdlib (#06): std::process::Command builder + Output /
    // ExitStatus accessor surface. Wire layouts documented in
    // `runtime.c` at `ruxen_command_new`.
    "ruxen_command_new",
    "ruxen_command_arg",
    "ruxen_command_args",
    "ruxen_command_env",
    "ruxen_command_current_dir",
    "ruxen_command_status",
    "ruxen_command_output",
    "ruxen_command_drop",
    "ruxen_exit_status_code",
    "ruxen_exit_status_success",
    "ruxen_exit_status_free",
    "ruxen_output_stdout",
    "ruxen_output_stderr",
    "ruxen_output_status",
    "ruxen_output_drop",
    "ruxen_process_exit",
    // std::process::run (Phase 3): fork+execvp a child, inherit stdio,
    // return exit code (or 128+signal on signal termination, 127 on
    // fork/exec failure).
    "ruxen_process_run",
];
