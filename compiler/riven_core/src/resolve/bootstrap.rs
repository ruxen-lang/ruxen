//! Stdlib bootstrap loader (#06.8 Wave 1 Task 0b).
//!
//! Reads stdlib `.rvn` files at compiler startup, BEFORE user code is
//! parsed, and routes them through the SAME parser that user code uses
//! — no second grammar, no second resolver path. The parsed programs
//! that come back are the future home of every stdlib class once
//! self-hosting migrations (Waves 2–5) move them from
//! `resolve/stdlib/mod.rs` registrations into Riven source.
//!
//! For Wave 1 the loader is a **parse-only gate**: `BOOTSTRAP_FILES`
//! is intentionally empty, so [`run_bootstrap`] is a no-op in
//! production. The infrastructure exists, can be exercised through
//! the test-only [`run_bootstrap_with_files`] shim, and reports
//! parse failures with file:line cite (the [E0725] diagnostic) — but
//! it does NOT yet inject anything into the prelude. That hookup
//! lands with the first stdlib `.rvn` migration (Wave 2 — `iter.rvn`
//! / `net.rvn` which already exist as aspirational docs).
//!
//! ## Path resolution
//!
//! 1. **`RIVEN_STDLIB_PATH` env var** — explicit override, same role
//!    as Rust's `RUST_SYSROOT`. Honoured by tests so they can point
//!    at a tempdir without touching the installed sysroot.
//! 2. **Workspace `library/std/`** — the development fallback.
//!    Detected by walking up from `CARGO_MANIFEST_DIR` until a
//!    `library/std/` directory containing the per-package layout
//!    (e.g. `io/src/lib.rvn`) is found. Matches how
//!    `error_code_registry.rs` finds `docs/errors/`. The pre-#06.95
//!    `library/std/_legacy/src/` and `library/std/src/` flat layouts
//!    remain as transitional fallbacks.
//! 3. **Exe-adjacent install layout** — when running an installed
//!    `rivenc`, `<exe>/../library/std/` is the conventional sysroot
//!    location.
//!
//! Failures at any stage emit `E0725` with the resolved file path so a
//! contributor can navigate to the offending stdlib source directly.

use crate::diagnostics::Diagnostic;
use crate::lexer::token::Span;
use crate::lexer::Lexer;
use crate::parser::ast::Program;
use crate::parser::Parser;
use std::path::{Path, PathBuf};

/// The stdlib bootstrap file list. Wave 1.5 (#06.8 Phase 3) shipped one
/// proof-of-life file — `_bootstrap_smoke.rvn` — that declares a single
/// FFI alias against the runtime test symbol `riven_test_extern_add_one`.
/// Wave 2 (#06.8) starts the actual stdlib migration; `rand.rvn` is the
/// first module whose surface lives entirely in the .rvn source rather
/// than in the Rust `resolve/stdlib/mod.rs` registrations.
///
/// Paths are relative to `<sysroot>/library/std/`.
/// Files are loaded in the order listed. Cross-file type dependencies
/// must be respected: a `.rvn` that references `IoError` in a lib
/// block (e.g. `def foo(...) -> Result[T, IoError]`) needs `io.rvn`
/// — which owns the enum — to come earlier in the list. The
/// bootstrap merge processes each file with Pass-1 forward-decl AND
/// full lib-type resolution in the same loop, so within-list order
/// matters.
/// Paths are interpreted relative to the resolved stdlib root.
///
/// #06.95 Phase B: the stdlib lives in per-module packages under
/// `library/std/<pkg>/src/lib.rvn`. Within-list order still matters
/// because the bootstrap merge runs Pass-1 forward-decl AND full
/// lib-type resolution in the same loop — a `.rvn` that references a
/// type owned by another package needs that package's `lib.rvn` to come
/// earlier. The ordering mirrors the topological layering described in
/// `docs/prompts/v1/06_95_stdlib_packagization.md` (Layer 0 first, then
/// Layer 1, …).
///
/// During the transition the legacy fallback path
/// (`library/std/_legacy/src/<file>.rvn`) is preserved at the loader
/// level via `resolve_stdlib_root` — checkouts still on the flat layout
/// keep working.
pub const BOOTSTRAP_FILES: &[&str] = &[
    "bootstrap_smoke/src/lib.rvn",
    // io ships IoError + IoErrorKind which rand/env/fs reference.
    "io/src/lib.rvn",
    "rand/src/lib.rvn",
    "path/src/lib.rvn",
    "env/src/lib.rvn",
    "iter/src/lib.rvn",
    "hash/src/lib.rvn",
    "fmt/src/lib.rvn",
    "net/src/lib.rvn",
    "process/src/lib.rvn",
    "time/src/lib.rvn",
    "fs/src/lib.rvn",
    "sync/src/lib.rvn",
    "string/src/lib.rvn",
    "option_result/src/lib.rvn",
    "array/src/lib.rvn",
    "map/src/lib.rvn",
    "set/src/lib.rvn",
];

/// Production entry point: parse every stdlib file in
/// [`BOOTSTRAP_FILES`] and return the resulting [`Program`] AST list.
/// Parse failures are reported via the `diagnostics` out-parameter
/// as fatal E0725 errors; the caller decides whether to abort.
///
/// Wave 1: no-op (empty list). Wave 2+: this is how the prelude
/// gains its self-hosted stdlib types.
pub fn run_bootstrap(diagnostics: &mut Vec<Diagnostic>) -> Vec<Program> {
    run_bootstrap_with_files(BOOTSTRAP_FILES, None, diagnostics)
}

/// Test-friendly variant of [`run_bootstrap`] that takes an explicit
/// file list and an optional path override. The override pins the
/// sysroot to a specific directory (a tempdir, typically) so tests
/// can exercise the loader without depending on the workspace layout.
///
/// When `path_override` is `None`, the same resolution as
/// [`resolve_stdlib_root`] applies (env var → workspace → exe).
pub fn run_bootstrap_with_files(
    files: &[&str],
    path_override: Option<&Path>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Program> {
    if files.is_empty() {
        return Vec::new();
    }

    let root = match path_override
        .map(PathBuf::from)
        .or_else(resolve_stdlib_root)
    {
        Some(r) => r,
        None => {
            diagnostics.push(Diagnostic::error_with_code(
                "stdlib bootstrap failed: could not locate `library/std/` — \
                 set RIVEN_STDLIB_PATH or run from a workspace checkout",
                Span::new(0, 0, 0, 0),
                "E0725",
            ));
            return Vec::new();
        }
    };

    let mut out = Vec::with_capacity(files.len());
    for rel in files {
        match load_stdlib_file(&root, rel) {
            Ok(program) => out.push(program),
            Err(diag) => diagnostics.push(diag),
        }
    }
    out
}

/// Resolve the stdlib source root, in order:
///   1. `$RIVEN_STDLIB_PATH`
///   2. workspace `library/std/src/` (walk up from `CARGO_MANIFEST_DIR`)
///   3. `<exe-dir>/../library/std/src/`
///
/// Returns `None` only when every option fails — at which point the
/// caller emits an E0725 against an empty span (there is no source
/// file to anchor against).
pub fn resolve_stdlib_root() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("RIVEN_STDLIB_PATH") {
        let p = PathBuf::from(env_path);
        if p.is_dir() {
            return Some(p);
        }
    }

    // #06.95 Phase B: the stdlib layout is per-package — each module
    // owns its own directory under `library/std/` with a `Riven.toml`
    // manifest, a `src/lib.rvn` surface, and a `runtime/` C-source
    // tree. The bootstrap root is `library/std/` itself; the loader
    // walks that directory looking for sub-package manifests. The
    // legacy `_legacy/src/` and `src/` paths are kept as fallbacks
    // during the transition.
    if let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
        let mut cur = PathBuf::from(manifest_dir);
        for _ in 0..5 {
            let pkg_root = cur.join("library/std");
            if pkg_root.join("io/src/lib.rvn").is_file() {
                return Some(pkg_root);
            }
            let legacy = cur.join("library/std/_legacy/src");
            if legacy.is_dir() {
                return Some(legacy);
            }
            let flat = cur.join("library/std/src");
            if flat.is_dir() {
                return Some(flat);
            }
            if !cur.pop() {
                break;
            }
        }
    }

    // Exe-adjacent install layout: <exe>/../library/std/.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let pkg_root = exe_dir.join("../library/std");
            if pkg_root.join("io/src/lib.rvn").is_file() {
                return Some(pkg_root);
            }
            let legacy = exe_dir.join("../library/std/_legacy/src");
            if legacy.is_dir() {
                return Some(legacy);
            }
            let flat = exe_dir.join("../library/std/src");
            if flat.is_dir() {
                return Some(flat);
            }
        }
    }

    None
}

/// Read, lex, and parse one stdlib file relative to the resolved
/// sysroot root. Returns the parsed [`Program`] on success, or an
/// E0725 diagnostic that names the file and the first parser/lexer
/// error's line on failure.
fn load_stdlib_file(root: &Path, rel: &str) -> Result<Program, Diagnostic> {
    let full = root.join(rel);
    let source = std::fs::read_to_string(&full).map_err(|io_err| {
        Diagnostic::error_with_code(
            format!(
                "stdlib bootstrap failed at library/std/{}: cannot read file: {}",
                rel, io_err
            ),
            Span::new(0, 0, 0, 0),
            "E0725",
        )
    })?;

    let mut lexer = Lexer::new(&source);
    let tokens = match lexer.tokenize() {
        Ok(t) => t,
        Err(diags) => {
            let (line, msg) = first_error_line(&diags);
            return Err(Diagnostic::error_with_code(
                format!(
                    "stdlib bootstrap failed at library/std/{}:{}: lexer: {}",
                    rel, line, msg
                ),
                Span::new(0, 0, line, 0),
                "E0725",
            ));
        }
    };

    let mut parser = Parser::new(tokens);
    parser.parse().map_err(|diags| {
        let (line, msg) = first_error_line(&diags);
        Diagnostic::error_with_code(
            format!(
                "stdlib bootstrap failed at library/std/{}:{}: parser: {}",
                rel, line, msg
            ),
            Span::new(0, 0, line, 0),
            "E0725",
        )
    })
}

/// Pick the first error-level diagnostic's line and message. Falls
/// back to (0, "unknown error") if the diag list is empty — should
/// never happen but keeps the function total.
fn first_error_line(diags: &[Diagnostic]) -> (u32, String) {
    diags
        .iter()
        .find(|d| matches!(d.level, crate::diagnostics::DiagnosticLevel::Error))
        .or_else(|| diags.first())
        .map(|d| (d.span.line, d.message.clone()))
        .unwrap_or((0, "unknown error".to_string()))
}
