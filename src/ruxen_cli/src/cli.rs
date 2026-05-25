//! CLI argument definitions using clap derive macros.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ruxen", version, about = "The Ruxen language toolchain")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Show detailed output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Color output: auto, always, never
    #[arg(long, global = true, default_value = "auto")]
    pub color: String,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a new Ruxen project
    New {
        /// Project name
        name: String,
        /// Create a library project (default: binary)
        #[arg(long)]
        lib: bool,
        /// Don't initialize a git repository
        #[arg(long)]
        no_git: bool,
    },

    /// Initialize a Ruxen project in the current directory
    Init,

    /// Compile the current project
    Build {
        /// Build with optimizations
        #[arg(long)]
        release: bool,
        /// Fail if Ruxen.lock is out of date (for CI)
        #[arg(long)]
        locked: bool,
        /// Build a specific binary
        #[arg(long)]
        bin: Option<String>,
    },

    /// Build and run the project
    Run {
        /// Run the release build
        #[arg(long)]
        release: bool,
        /// Arguments passed to the program
        #[arg(last = true)]
        args: Vec<String>,
    },

    /// Type-check without generating code
    Check,

    /// Remove the target/ directory
    Clean,

    /// Add a dependency to Ruxen.toml
    Add {
        /// Piece name
        piece: String,
        /// Specify version requirement
        #[arg(long)]
        version: Option<String>,
        /// Add as git dependency
        #[arg(long)]
        git: Option<String>,
        /// Add as path dependency
        #[arg(long)]
        path: Option<String>,
        /// Add to [dev-dependencies]
        #[arg(long)]
        dev: bool,
        /// Git branch (with --git)
        #[arg(long)]
        branch: Option<String>,
        /// Git tag (with --git)
        #[arg(long)]
        tag: Option<String>,
        /// Git revision (with --git)
        #[arg(long)]
        rev: Option<String>,
    },

    /// Remove a dependency
    Remove {
        /// Piece name
        piece: String,
    },

    /// Update project dependencies (all or a specific one).
    Update {
        /// Specific piece to update (default: all)
        piece: Option<String>,
    },

    /// Upgrade the Ruxen toolchain itself.
    Upgrade {
        /// Rebuild and reinstall from a local source checkout
        /// instead of fetching a release. Pass `.` for the
        /// current directory.
        #[arg(long, value_name = "PATH")]
        from_source: Option<String>,

        /// Pin a specific release tag when fetching from GitHub
        /// (e.g. `v0.2.0`). Ignored when `--from-source` is set.
        #[arg(long, value_name = "TAG")]
        version: Option<String>,
    },

    /// Display dependency tree
    Tree,

    /// Verify lock file checksums
    Verify,

    /// Explain a compiler error code (e.g. `ruxen explain E0001`)
    Explain {
        /// Error code to look up (e.g. `E0001`)
        code: String,
    },

    /// Compile a single .rx file directly (low-level driver — like rustc).
    /// For project-level builds use `ruxen build`.
    Compile {
        /// Path to a .rx file
        file: String,
        /// Output binary path
        #[arg(short)]
        output: Option<String>,
        /// Forwarded flags (--emit=ast/hir/mir/tokens, --release,
        /// --backend=..., --opt-level=..., --force, --verbose)
        #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
        extra: Vec<String>,
    },

    /// Format .rx files in-place. Mirrors `ruxenc fmt`.
    Fmt {
        /// Files to format (default: all .rx files recursively)
        files: Vec<String>,
        /// Exit 1 if any file would change (CI mode)
        #[arg(long)]
        check: bool,
        /// Show diff of what would change
        #[arg(long)]
        diff: bool,
        /// Read stdin, write formatted to stdout
        #[arg(long)]
        stdin: bool,
        /// Logical filepath when using --stdin
        #[arg(long)]
        filepath: Option<String>,
    },

    /// Run microbenchmarks defined as `def bench_*(b: &var Bencher)` in a file.
    Bench {
        /// The .rx file containing bench fns
        file: String,
        /// Substring filter on bench names
        #[arg(long)]
        filter: Option<String>,
        /// Initial iteration count (default 100, auto-scales)
        #[arg(long = "iter-hint")]
        iter_hint: Option<i64>,
    },

    /// Test framework — placeholder. See docs/prompts/v1/19_phase5_test_framework.md.
    Test {
        /// Substring filter on test names
        #[arg(long)]
        filter: Option<String>,
    },

    /// Start the Ruxen Language Server (LSP over stdio).
    ///
    /// Launched by editors / IDEs; communicates over stdin/stdout. There
    /// are no flags today.
    Lsp,

    /// Start the interactive Ruxen REPL.
    Repl,
}

#[cfg(test)]
mod tests {
    //! Argv-parse tests for the `update` / `upgrade` subcommand split.
    //! Live in the lib (not under `tests/`) so they don't drag the
    //! `ruxen` bin into the test link graph — the bin pulls in the
    //! cranelift-JIT runtime archive from `ruxen_repl`, which is
    //! orthogonal to the CLI surface this test exercises.

    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    #[test]
    fn update_accepts_no_args() {
        let cli = parse(&["ruxen", "update"]).expect("ruxen update parses");
        match cli.command {
            Command::Update { piece } => assert!(piece.is_none()),
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn update_accepts_piece() {
        let cli = parse(&["ruxen", "update", "foo"]).expect("ruxen update foo parses");
        match cli.command {
            Command::Update { piece } => assert_eq!(piece.as_deref(), Some("foo")),
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn update_rejects_from_source() {
        // The whole point of the split — `--from-source` must no longer
        // live on `update`. It belongs to `upgrade` now.
        let err = match parse(&["ruxen", "update", "--from-source", "."]) {
            Ok(_) => panic!(
                "ruxen update --from-source must error now that the flag moved to `upgrade`"
            ),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("--from-source") || msg.contains("unexpected"),
            "expected clap to reject --from-source on update; got: {msg}"
        );
    }

    #[test]
    fn upgrade_accepts_no_args() {
        let cli = parse(&["ruxen", "upgrade"]).expect("ruxen upgrade parses");
        match cli.command {
            Command::Upgrade {
                from_source,
                version,
            } => {
                assert!(from_source.is_none());
                assert!(version.is_none());
            }
            _ => panic!("expected Upgrade"),
        }
    }

    #[test]
    fn upgrade_accepts_from_source() {
        let cli = parse(&["ruxen", "upgrade", "--from-source", "/tmp/checkout"])
            .expect("ruxen upgrade --from-source parses");
        match cli.command {
            Command::Upgrade {
                from_source,
                version,
            } => {
                assert_eq!(from_source.as_deref(), Some("/tmp/checkout"));
                assert!(version.is_none());
            }
            _ => panic!("expected Upgrade"),
        }
    }

    #[test]
    fn upgrade_accepts_version() {
        let cli = parse(&["ruxen", "upgrade", "--version", "v0.2.0"])
            .expect("ruxen upgrade --version parses");
        match cli.command {
            Command::Upgrade {
                from_source,
                version,
            } => {
                assert!(from_source.is_none());
                assert_eq!(version.as_deref(), Some("v0.2.0"));
            }
            _ => panic!("expected Upgrade"),
        }
    }
}
