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

    /// Update dependencies (all or specific), or rebuild the
    /// toolchain itself from a local source checkout via
    /// `--from-source <path>`.
    Update {
        /// Specific piece to update
        piece: Option<String>,

        /// Rebuild + reinstall the ruxen toolchain from a local
        /// source checkout. Pass `.` for the current directory.
        /// When set, the `[piece]` argument is ignored — this is a
        /// self-update flow, not a dependency update.
        #[arg(long, value_name = "PATH")]
        from_source: Option<String>,
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
}
