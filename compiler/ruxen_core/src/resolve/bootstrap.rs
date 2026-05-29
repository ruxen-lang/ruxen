//! Stdlib bootstrap loader (#06.8 Wave 1 Task 0b).
//!
//! Reads stdlib `.rx` files at compiler startup, BEFORE user code is
//! parsed, and routes them through the SAME parser that user code uses
//! — no second grammar, no second resolver path. The parsed programs
//! that come back are the future home of every stdlib class once
//! self-hosting migrations (Waves 2–5) move them from
//! `resolve/stdlib/mod.rs` registrations into Ruxen source.
//!
//! For Wave 1 the loader is a **parse-only gate**: `BOOTSTRAP_FILES`
//! is intentionally empty, so [`run_bootstrap`] is a no-op in
//! production. The infrastructure exists, can be exercised through
//! the test-only [`run_bootstrap_with_files`] shim, and reports
//! parse failures with file:line cite (the [E0725] diagnostic) — but
//! it does NOT yet inject anything into the prelude. That hookup
//! lands with the first stdlib `.rx` migration (Wave 2 — `iter.rx`
//! / `net.rx` which already exist as aspirational docs).
//!
//! ## Path resolution
//!
//! 1. **`RUXEN_STDLIB_PATH` env var** — explicit override, same role
//!    as Rust's `RUST_SYSROOT`. Honoured by tests so they can point
//!    at a tempdir without touching the installed sysroot.
//! 2. **Workspace `library/std/`** — the development fallback.
//!    Detected by walking up from `CARGO_MANIFEST_DIR` until a
//!    `library/std/` directory containing the per-package layout
//!    (e.g. `io/src/lib.rx`) is found. Matches how
//!    `error_code_registry.rs` finds `docs/errors/`. The pre-#06.95
//!    `library/std/_legacy/src/` and `library/std/src/` flat layouts
//!    remain as transitional fallbacks.
//! 3. **Exe-adjacent install layout** — when running an installed
//!    `ruxenc`, `<exe>/../library/std/` is the conventional sysroot
//!    location.
//!
//! Failures at any stage emit `E0725` with the resolved file path so a
//! contributor can navigate to the offending stdlib source directly.

use crate::diagnostics::Diagnostic;
use crate::lexer::token::Span;
use crate::lexer::Lexer;
use crate::parser::ast;
use crate::parser::ast::Program;
use crate::parser::Parser;
use crate::resolve::stdlib_embedded;
use std::path::{Path, PathBuf};

/// The stdlib bootstrap file list. Wave 1.5 (#06.8 Phase 3) shipped one
/// proof-of-life file — `_bootstrap_smoke.rx` — that declares a single
/// FFI alias against the runtime test symbol `ruxen_test_extern_add_one`.
/// Wave 2 (#06.8) starts the actual stdlib migration; `rand.rx` is the
/// first module whose surface lives entirely in the .rx source rather
/// than in the Rust `resolve/stdlib/mod.rs` registrations.
///
/// Paths are relative to `<sysroot>/library/std/`.
/// Files are loaded in the order listed. Cross-file type dependencies
/// must be respected: a `.rx` that references `IoError` in a lib
/// block (e.g. `def foo(...) -> Result[T, IoError]`) needs `io.rx`
/// — which owns the enum — to come earlier in the list. The
/// bootstrap merge processes each file with Pass-1 forward-decl AND
/// full lib-type resolution in the same loop, so within-list order
/// matters.
/// Paths are interpreted relative to the resolved stdlib root.
///
/// #06.95 Phase B: the stdlib lives in per-module packages under
/// `library/std/<pkg>/src/lib.rx`. Within-list order still matters
/// because the bootstrap merge runs Pass-1 forward-decl AND full
/// lib-type resolution in the same loop — a `.rx` that references a
/// type owned by another package needs that package's `lib.rx` to come
/// earlier. The ordering mirrors the topological layering described in
/// `docs/prompts/v1/06_95_stdlib_packagization.md` (Layer 0 first, then
/// Layer 1, …).
///
/// During the transition the legacy fallback path
/// (`library/std/_legacy/src/<file>.rx`) is preserved at the loader
/// level via `resolve_stdlib_root` — checkouts still on the flat layout
/// keep working.
pub const BOOTSTRAP_FILES: &[&str] = &[
    // std-core ships the 16 builtin mixins (Send / Sync / Clone / Copy /
    // Displayable / Comparable / PartialEq / Eq / Hash / Default / Ord /
    // PartialOrd / Drop / Into[T] / Iterable / Error). Every other
    // bootstrap file plus user code references them through `include
    // Send` / `T: Sync` / etc., so core MUST load first. Previously
    // `register_builtins` was meant to register these in Rust before
    // bootstrap ran (see `resolve/stdlib/mod.rs` Phase D-3 comment),
    // but the actual move from Rust registrations to .rx left the
    // file off the load list — `include Send` on a user class was
    // surfacing E0608 until B5 of
    // docs/specs/system/zero_rust_stdlib_classes.spec.md tracked it down.
    "core/src/lib.rx",
    "bootstrap_smoke/src/lib.rx",
    // io ships IoError + IoErrorKind which rand/env/fs/net reference.
    "io/src/lib.rx",
    "rand/src/lib.rx",
    "path/src/lib.rx",
    "env/src/lib.rx",
    "iter/src/lib.rx",
    "hash/src/lib.rx",
    "fmt/src/lib.rx",
    "net/src/lib.rx",
    // bufio depends on BOTH io (File / IoError) and net (TcpStream).
    // Loaded AFTER both to resolve the cross-package references in
    // the `module BufReader { class File; class Tcp }` declarations.
    "bufio/src/lib.rx",
    "process/src/lib.rx",
    "time/src/lib.rx",
    "fs/src/lib.rx",
    "sync/src/lib.rx",
    // future depends on std-core only and ships the async sub-phase 1
    // surface: `Future` mixin, `Poll[T]` enum, `Context` / `Waker`
    // class shells with lib decls pointing at the stubbed
    // `runtime/executor.c` (every entry panics until sub-phase 3).
    // Loaded AFTER sync purely so the deprecated empty Context /
    // Waker shells in sync.rx (deleted in this commit) don't risk
    // re-registering on top of the real ones if any caller still
    // looks them up there.
    "future/src/lib.rx",
    // async_fs depends on future (Future mixin / Poll / reactor primitives)
    // + io (IoError) + fs (in spirit; the package owns AsyncFile parallel
    // to sync File). Sub-phase 4B of docs/specs/stdlib/async_io.spec.md.
    // Lives in its own package so AsyncFile / AsyncOpenFuture /
    // AsyncReadToStringFuture / AsyncWriteAllFuture can `include Future`
    // — fs/src/lib.rx loads BEFORE future/src/lib.rx and so cannot.
    "async_fs/src/lib.rx",
    // async_net depends on future (Future mixin / Poll / reactor primitives)
    // + io (IoError) + net (in spirit; the package owns AsyncTcpListener /
    // AsyncTcpStream parallel to sync net). Sub-phase 4C of
    // docs/specs/stdlib/async_io.spec.md.
    "async_net/src/lib.rx",
    // async_io ships AsyncStdin (read_line) — the third async stdlib
    // variant alongside async_fs (AsyncFile) and async_net (AsyncTcpStream /
    // AsyncTcpListener). Sits in its own package for the same reason as
    // the other two: it `include`s Future and so must load AFTER
    // future/src/lib.rx. AsyncStdout / AsyncStderr are deferred to
    // v1.1 (blocking writes via std::io cover the demand profile;
    // kernel write-buffering means non-blocking stdout has no real
    // use case). See prompt 15 DoD bullet 4.
    "async_io/src/lib.rx",
    // executor depends on future (block_on takes a Future-implementing
    // value). Sub-phase 3 of the async round (docs/specs/stdlib/
    // executor.spec.md). The user-visible `block_on` is a free fn here;
    // the compiler intrinsic rewriter in async_lowering replaces every
    // call site with an inline poll loop at AST time, so this
    // declaration only carries the signature surface — the body never
    // actually runs.
    "executor/src/lib.rx",
    "string/src/lib.rx",
    "option_result/src/lib.rx",
    "array/src/lib.rx",
    "map/src/lib.rx",
    "set/src/lib.rx",
    // json depends on array/map/string for its explicit builder surface.
    // Keep it after those packages so `Array[Json]`, `Map[String, Json]`,
    // and `String` payload helpers resolve.
    "json/src/lib.rx",
    // foobar — trio-leak pin fixture (B5 of
    // docs/specs/system/zero_rust_stdlib_classes.spec.md). Adding a
    // stdlib class via a fresh package MUST require ONLY this entry
    // in `compiler/ruxen_core/src/`. Anything else is an auto-connect
    // gap to fix in the bootstrap pipeline, not in this list.
    "foobar/src/lib.rx",
    // regex — PCRE2-backed `Regex`, `Match`, `RegexError` classes.
    // Depends on string/array/map/option_result for return types
    // (`Array[Match]`, `HashMap[String, String]`, `Option[String]`,
    // `Result[Regex, RegexError]`) — all loaded above. The
    // `/pat/flags` literal + `~=` operator surface in the lexer /
    // parser / typeck reference `Ty::Class { name: "Regex" }`, and
    // every method call (`is_match`, `find`, `scan`, `replace`,
    // `replace_all`, `split`, plus `Match` accessors) routes through
    // the per-class `lib "runtime/regex.c"` FFI aliases declared
    // here. Without this entry, those calls mangle to bare
    // `Regex_<method>` / `Match_<method>` and fail to link.
    "regex/src/lib.rx",
    // bench — prompt 13 microbenchmark harness. `Bencher` class
    // with auto-scaling `iter` + Int-typed `black_box` opaque
    // barrier (C shim). Depends on `time` (Instant.now /
    // .elapsed.as_nanos), already loaded above.
    "bench/src/lib.rx",
    // test — pure-Ruxen test framework (Tester DSL + Matcher + Runner).
    // Depends on string/array/option_result/fmt/sync — all already loaded
    // above. Discovery + synthesis live in Rust (ruxenc::test_runner);
    // this entry registers the runtime classes the synthesised `def main`
    // references via `use std.test.Tester` / `use std.test.Runner`.
    "test/src/lib.rx",
];

/// Derive the ordered list of package names from [`BOOTSTRAP_FILES`].
/// The first path segment of each entry is the package name (e.g.
/// `"io/src/lib.rx"` → `"io"`), matching the per-package layout
/// `library/std/<pkg>/src/lib.rx`. Duplicates in BOOTSTRAP_FILES are
/// preserved (none expected; would indicate a bug in the file list).
///
/// Task #17: this is the single source of truth for std-submodule
/// namespaces. `register_builtins` consumes it (plus a small static
/// list of synthetic namespaces — `thread` / `signal` — that re-export
/// sync.rx shims under legacy import paths) to assemble the `std`
/// module's items at resolver-init time. Adding a new stdlib package
/// is therefore a one-line BOOTSTRAP_FILES edit: `std.<new>.X`
/// resolution comes along automatically.
pub fn bootstrap_package_names() -> Vec<&'static str> {
    BOOTSTRAP_FILES
        .iter()
        .filter_map(|rel| rel.split('/').next())
        .collect()
}

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

/// Variant of [`run_bootstrap`] that returns each parsed program
/// alongside the package name derived from its relative path
/// (e.g. `"io/src/lib.rx"` → `"io"`). Callers that need to
/// associate items with their owning package (e.g. for
/// auto-populating `std.<pkg>` submodules) use this surface.
pub fn run_bootstrap_with_package_names(
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(String, Program)> {
    let programs = run_bootstrap(diagnostics);
    BOOTSTRAP_FILES
        .iter()
        .zip(programs)
        .filter_map(|(rel, p)| {
            // First path segment is the package name.
            let pkg = rel.split('/').next()?.to_string();
            Some((pkg, p))
        })
        .collect()
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

    // Resolution order:
    //   1. `path_override` argument (tests pin via tempdir)
    //   2. `RUXEN_STDLIB_PATH` env (dev override — edit .rx without
    //      recompiling the compiler)
    //   3. Embedded sources baked in by `include_str!` (default path
    //      for any released binary)
    //   4. Legacy filesystem walks (workspace `library/std/`,
    //      `<exe-dir>/../library/std/`) — retained so a Cargo
    //      `cargo run` from the workspace still works without
    //      regenerating the embedded table
    let fs_root: Option<PathBuf> = path_override.map(PathBuf::from).or_else(|| {
        std::env::var("RUXEN_STDLIB_PATH")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
    });

    // If the caller forced a filesystem root, honour it. Otherwise
    // try embedded first, then fall back to the legacy filesystem
    // discovery so existing workflows aren't broken.
    let prefer_embedded = fs_root.is_none();

    let mut out = Vec::with_capacity(files.len());
    for rel in files {
        let loaded = if prefer_embedded {
            // Multi-file packages: prefer the sibling-aware loader so
            // the embedded path matches what the filesystem path
            // produces (lib.rx + every sibling concatenated, items
            // reordered so types come before functions).
            if let Some(sources) = stdlib_embedded::embedded_pkg_sources(rel) {
                load_embedded_multi_file(rel, sources)
            } else {
                match stdlib_embedded::embedded_source(rel) {
                    Some(src) => parse_stdlib_source(src, rel),
                    None => {
                        // Embedded table is missing this file — fall back to
                        // the legacy filesystem walk for this one entry.
                        match resolve_stdlib_root() {
                            Some(root) => load_stdlib_file(&root, rel),
                            None => Err(missing_stdlib_diagnostic(rel)),
                        }
                    }
                }
            }
        } else {
            load_stdlib_file(fs_root.as_ref().unwrap(), rel)
        };

        match loaded {
            Ok(program) => out.push(program),
            Err(diag) => diagnostics.push(diag),
        }
    }
    out
}

fn missing_stdlib_diagnostic(rel: &str) -> Diagnostic {
    Diagnostic::error_with_code(
        format!(
            "stdlib bootstrap failed at library/std/{}: not present in \
             embedded table and no filesystem fallback found. Rebuild \
             the ruxen binary, or set `RUXEN_STDLIB_PATH` to a workspace \
             checkout.",
            rel
        ),
        Span::new(0, 0, 0, 0),
        "E0725",
    )
}

/// Resolve the stdlib source root, in order:
///   1. `$RUXEN_STDLIB_PATH`
///   2. workspace `library/std/src/` (walk up from `CARGO_MANIFEST_DIR`)
///   3. `<exe-dir>/../library/std/src/`
///
/// Returns `None` only when every option fails — at which point the
/// caller emits an E0725 against an empty span (there is no source
/// file to anchor against).
pub fn resolve_stdlib_root() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("RUXEN_STDLIB_PATH") {
        let p = PathBuf::from(env_path);
        if p.is_dir() {
            return Some(p);
        }
    }

    // #06.95 Phase B: the stdlib layout is per-package — each module
    // owns its own directory under `library/std/` with a `Ruxen.toml`
    // manifest, a `src/lib.rx` surface, and a `runtime/` C-source
    // tree. The bootstrap root is `library/std/` itself; the loader
    // walks that directory looking for sub-package manifests. The
    // legacy `_legacy/src/` and `src/` paths are kept as fallbacks
    // during the transition.
    if let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
        let mut cur = PathBuf::from(manifest_dir);
        for _ in 0..5 {
            let pkg_root = cur.join("library/std");
            if pkg_root.join("io/src/lib.rx").is_file() {
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
            if pkg_root.join("io/src/lib.rx").is_file() {
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

/// Read, lex, and parse one stdlib package entry relative to the
/// resolved sysroot root. The `rel` path points at a `lib.rx` (e.g.
/// `"io/src/lib.rx"`) — the loader reads `lib.rx` PLUS every other
/// `.rx` file alongside it (sibling files in the same `src/` dir)
/// and concatenates them into one parse input. This is what lets a
/// stdlib package grow past a single file:
///
///   library/std/io/src/
///     lib.rx        ← entry point, listed in BOOTSTRAP_FILES
///     reader.rx     ← sibling, auto-included
///     writer.rx     ← sibling, auto-included
///
/// Non-`lib.rx` entries are loaded verbatim (no sibling scan) for
/// callers that explicitly point at a single source. Returns the
/// parsed [`Program`] on success, or an E0725 diagnostic that names
/// the file and the first parser/lexer error's line on failure.
fn load_stdlib_file(root: &Path, rel: &str) -> Result<Program, Diagnostic> {
    let full = root.join(rel);
    let entry_name = full.file_name().and_then(|f| f.to_str()).unwrap_or("");
    if entry_name != "lib.rx" {
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
        return parse_stdlib_source(&source, rel);
    }
    // lib.rx — collect every sibling `.rx` in the same dir and
    // concatenate. `lib.rx` always loads first so any forward-
    // referenced types declared at the top of lib.rx (the historical
    // single-file shape) keep resolving cleanly; siblings are loaded
    // in deterministic (sorted-by-filename) order after that.
    let pkg_src_dir = full.parent().ok_or_else(|| {
        Diagnostic::error_with_code(
            format!(
                "stdlib bootstrap failed at library/std/{}: cannot resolve parent dir",
                rel
            ),
            Span::new(0, 0, 0, 0),
            "E0725",
        )
    })?;
    let mut sibling_paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(pkg_src_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("rx") {
                continue;
            }
            if p.file_name().and_then(|f| f.to_str()) == Some("lib.rx") {
                continue;
            }
            sibling_paths.push(p);
        }
    }
    sibling_paths.sort();
    let lib_src = std::fs::read_to_string(&full).map_err(|io_err| {
        Diagnostic::error_with_code(
            format!(
                "stdlib bootstrap failed at library/std/{}: cannot read file: {}",
                rel, io_err
            ),
            Span::new(0, 0, 0, 0),
            "E0725",
        )
    })?;
    if sibling_paths.is_empty() {
        // Common case: package is a single file. Parse + return as
        // before so the historical single-file shape stays unchanged.
        return parse_stdlib_source(&lib_src, rel);
    }
    // Multi-file package: concatenate lib.rx + siblings into one
    // parse input separated by newlines so spans don't bleed across
    // file boundaries inside an item.
    //
    // Ordering matters: resolver pass-1 resolves free-function signatures
    // eagerly (no separate forward-declare pass for fn params/returns).
    // If `async_close_future.rx` defines `def of(s: AsyncTcpStream)` and
    // sorts alphabetically BEFORE `async_tcp_stream.rx`, the param type
    // can't resolve. Read each sibling once and emit files containing
    // top-level class declarations first (alphabetical within group),
    // then function-only files. lib.rx always leads.
    let mut sibling_sources: Vec<(PathBuf, String)> = Vec::with_capacity(sibling_paths.len());
    for sib in sibling_paths {
        let src = std::fs::read_to_string(&sib).map_err(|io_err| {
            Diagnostic::error_with_code(
                format!(
                    "stdlib bootstrap failed at {}: cannot read sibling: {}",
                    sib.display(),
                    io_err
                ),
                Span::new(0, 0, 0, 0),
                "E0725",
            )
        })?;
        sibling_sources.push((sib, src));
    }
    sibling_sources.sort_by(|(a_path, a_src), (b_path, b_src)| {
        let a_has = source_has_top_level_class(a_src);
        let b_has = source_has_top_level_class(b_src);
        b_has.cmp(&a_has).then_with(|| a_path.cmp(b_path))
    });
    let total_extra: usize = sibling_sources.iter().map(|(_, s)| s.len() + 1).sum();
    let mut combined = String::with_capacity(lib_src.len() + total_extra);
    combined.push_str(&lib_src);
    for (_, src) in &sibling_sources {
        combined.push('\n');
        combined.push_str(src);
    }
    let mut program = parse_stdlib_source(&combined, rel)?;
    // Reorder top-level items so type declarations precede functions
    // and lib decls. Resolver pass-1 resolves free-function signatures
    // eagerly — `def f(x: AsyncTcpStream)` in `async_close_future.rx`
    // can be parsed BEFORE `class AsyncTcpStream` in
    // `async_tcp_stream.rx` even after class-bearing files are sorted
    // first by filename, because the same file mixes a class and a
    // free function. Stable sort by item-category keeps siblings'
    // intra-file order intact while floating Function/Lib items to
    // the back. Use/Const/Extern stay at the front to match historical
    // single-file layout. See multi-file package loader above.
    program.items.sort_by_key(item_load_priority);
    Ok(program)
}

/// Embedded twin of the multi-file branch in [`load_stdlib_file`].
/// Concatenates `lib.rx` + every sibling source in the same order
/// the filesystem loader would emit, then reorders top-level items
/// by [`item_load_priority`] so type declarations precede function /
/// lib decls. Keeps the embedded path indistinguishable from the
/// filesystem path for downstream resolve.
fn load_embedded_multi_file(
    rel: &str,
    sources: Vec<(&'static str, &'static str)>,
) -> Result<Program, Diagnostic> {
    // Index 0 is always `lib.rx`; the rest are siblings. Sort the
    // sibling tail by (class-bearing-first, filename) so any later
    // free-fn signature referencing a sibling class sees the class
    // declared earlier in the combined source.
    let (lib_entry, rest) = sources
        .split_first()
        .expect("embedded_pkg_sources returns lib.rx + ≥1 sibling");
    let mut siblings: Vec<&(&'static str, &'static str)> = rest.iter().collect();
    siblings.sort_by(|a, b| {
        let a_has = source_has_top_level_class(a.1);
        let b_has = source_has_top_level_class(b.1);
        b_has.cmp(&a_has).then_with(|| a.0.cmp(b.0))
    });
    let total: usize = lib_entry.1.len() + siblings.iter().map(|s| s.1.len() + 1).sum::<usize>();
    let mut combined = String::with_capacity(total);
    combined.push_str(lib_entry.1);
    for (_, src) in &siblings {
        combined.push('\n');
        combined.push_str(src);
    }
    let mut program = parse_stdlib_source(&combined, rel)?;
    program.items.sort_by_key(item_load_priority);
    Ok(program)
}

/// Priority key for stable-sorting top-level items in a multi-file
/// bootstrap package. Lower comes first.
///   0 — Use (must precede any use-of-imports)
///   1 — Module/Class/Struct/Enum/Mixin/TypeAlias/Newtype (types and
///       namespaces — register names so later signatures resolve)
///   2 — Impl (extension of an already-registered class)
///   3 — Function/Lib/Extern/Const (consume types in their signatures)
fn item_load_priority(item: &ast::TopLevelItem) -> u8 {
    match item {
        ast::TopLevelItem::Use(_) => 0,
        ast::TopLevelItem::Module(_)
        | ast::TopLevelItem::Class(_)
        | ast::TopLevelItem::Struct(_)
        | ast::TopLevelItem::Enum(_)
        | ast::TopLevelItem::Mixin(_)
        | ast::TopLevelItem::TypeAlias(_)
        | ast::TopLevelItem::Newtype(_) => 1,
        ast::TopLevelItem::Impl(_) => 2,
        ast::TopLevelItem::Function(_)
        | ast::TopLevelItem::Lib(_)
        | ast::TopLevelItem::Extern(_)
        | ast::TopLevelItem::Const(_) => 3,
    }
}

/// Cheap textual probe: does the source contain a top-level `class `
/// declaration? Used by the multi-file loader to emit class-bearing
/// files before function-only files so forward-referenced class types
/// in free-fn signatures resolve during pass-1.
///
/// Conservatively scans for the literal `class ` at the start of a
/// line (after optional whitespace). False positives from comments
/// that happen to start with `class ` are tolerable — they just push
/// the file earlier in the concatenation, which never breaks resolve.
fn source_has_top_level_class(src: &str) -> bool {
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ") {
            return true;
        }
    }
    false
}

/// Lex + parse one stdlib source string. Shared between the
/// filesystem loader and the embedded-table loader so the
/// diagnostic shape is identical in both paths.
fn parse_stdlib_source(source: &str, rel: &str) -> Result<Program, Diagnostic> {
    let mut lexer = Lexer::new(source);
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
