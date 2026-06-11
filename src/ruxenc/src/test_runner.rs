//! `ruxen test` — discover `tests/**.rx`, wrap each in a synthesised
//! `def main`, build per-file binaries through the incremental cache,
//! and dispatch them in parallel with fork-per-test isolation supplied
//! by `library/std/test/runtime/test.c`.
//!
//! Architecture mirrors `ruxenc bench` (src/ruxenc/src/bench.rs):
//!   1. Discovery — walk `tests/**.rx`, excluding `tests/support/`.
//!   2. Synthesis — prepend `def main { Runner.new(...); Runner.set_current(...); ` + body + ` r.execute; }`.
//!   3. Compile each synthesised file via `crate::compile::run` so we
//!      inherit the existing incremental cache for free.
//!   4. Execute — bounded thread-pool via `std::thread::scope`, no
//!      new crates.
//!   5. Render — pretty (default) / TAP / JSON.
//!
//! See docs/superpowers/specs/2026-05-23-test-framework-design.md.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

pub struct TestOptions {
    pub filter: Option<String>,
    pub release: bool,
    pub test_threads: String,
    pub fail_fast: bool,
    pub nocapture: bool,
    pub list: bool,
    pub no_run: bool,
    pub include_pending: bool,
    pub format: String,
    /// Q16: source dirs of the project's path dependencies (topologically
    /// sorted), resolved by the caller
    /// (`ruxen_cli::build::resolve_dep_source_dirs`). Flat-merged ahead of
    /// the project's own library source into each synthesised test wrapper
    /// so a `tests/**.rx` file can reference dependency symbols. Empty when
    /// the project has no dependencies, or when `ruxenc` drives the test
    /// runner directly (single-file mode, no manifest resolution).
    pub dep_source_dirs: Vec<PathBuf>,
}

/// Per-file execution result. Aggregated across files into the summary
/// line; consumed by the TAP / JSON renderers.
#[derive(Debug)]
pub struct TestFileResult {
    pub test_path: String,
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    pub stdout: String,
    pub stderr: String,
    pub exit_ok: bool,
}

pub fn run(opts: TestOptions) -> Result<(), String> {
    let project_dir = find_project_root()?;
    let mut files = discover_test_files(&project_dir)?;

    // Filter (substring on derived test-path, NOT individual `it` names —
    // narrowing inside a file requires runtime-side filter dispatch which
    // we defer to v1.1).
    if let Some(pat) = opts.filter.as_deref() {
        files.retain(|f| test_path_for(&project_dir, f).contains(pat));
    }

    if files.is_empty() {
        println!("no test files found under tests/");
        return Ok(());
    }

    if opts.list {
        for f in &files {
            println!("{}", test_path_for(&project_dir, f));
        }
        return Ok(());
    }

    let out_dir = project_dir.join("target").join("ruxen").join("test-build");
    fs::create_dir_all(&out_dir).map_err(|e| format!("create {}: {}", out_dir.display(), e))?;

    // PHASE A — serial build.
    //
    // The Ruxen incremental cache (target/ruxen/incremental/manifest.bin)
    // is not thread-safe; concurrent compile::run calls race on the
    // manifest rename. Per-file build is fast once warm (cache hit), so
    // we build serially here and reserve parallelism for the execution
    // phase below where the wall-time actually lives.
    let mut built: Vec<(PathBuf, String, PathBuf)> = Vec::new(); // (user_file, test_path, bin_path)
    for f in &files {
        let tp = test_path_for(&project_dir, f);
        match build_one(
            &project_dir,
            &tp,
            f,
            &out_dir,
            opts.release,
            &opts.dep_source_dirs,
        ) {
            Ok(bin_path) => built.push((f.clone(), tp, bin_path)),
            Err(e) => {
                // Surface build errors as failed-file results so the
                // summary still produces the right exit code.
                let mut errs = Vec::new();
                errs.push(TestFileResult {
                    test_path: tp,
                    passed: 0,
                    failed: 1,
                    pending: 0,
                    stdout: String::new(),
                    stderr: format!("build error: {}", e),
                    exit_ok: false,
                });
                let opts_format = opts.format.clone();
                match opts_format.as_str() {
                    "tap" => crate::test_output::render_tap(&errs),
                    "json" => crate::test_output::render_json(&errs),
                    _ => render_pretty(&errs, true),
                }
                std::process::exit(1);
            }
        }
    }

    if opts.no_run {
        // Spec: --no-run builds but does not execute. Report nothing
        // beyond build completion.
        return Ok(());
    }

    // PHASE B — parallel execute.
    let n_workers = resolve_test_threads(&opts.test_threads);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let results: Arc<Mutex<Vec<TestFileResult>>> = Arc::new(Mutex::new(Vec::new()));
    let queue: Arc<Mutex<Vec<(PathBuf, String, PathBuf)>>> = Arc::new(Mutex::new(built));

    // Per-test report style, handed to each test binary via the
    // RUXEN_TEST_FORMAT env var (std.test.Runner reads it). The default
    // (and `pretty`) is RSpec-style progress dots; `documentation` prints
    // group + per-case names; tap/json own their own output (quiet).
    let env_format: String = match opts.format.as_str() {
        "documentation" | "doc" => "documentation",
        "tap" => "tap",
        "json" => "json",
        _ => "progress",
    }
    .to_string();

    std::thread::scope(|scope| {
        for _ in 0..n_workers {
            let queue = queue.clone();
            let results = results.clone();
            let stop = stop.clone();
            let fail_fast = opts.fail_fast;
            let env_format = env_format.clone();
            scope.spawn(move || loop {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let item = {
                    let mut q = queue.lock().unwrap();
                    if q.is_empty() {
                        return;
                    }
                    q.remove(0)
                };
                let (_user_file, tp, bin_path) = item;
                let r = run_one(&tp, &bin_path, &env_format);
                let had_failure = r.failed > 0 || !r.exit_ok;
                results.lock().unwrap().push(r);
                if fail_fast && had_failure {
                    stop.store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            });
        }
    });

    let mut results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
    // Stable ordering: sort by test_path so the summary is deterministic
    // regardless of thread scheduling.
    results.sort_by(|a, b| a.test_path.cmp(&b.test_path));

    match opts.format.as_str() {
        "tap" => crate::test_output::render_tap(&results),
        "json" => crate::test_output::render_json(&results),
        _ => render_pretty(&results, opts.nocapture),
    }

    let total_failed: u32 = results.iter().map(|r| r.failed).sum();
    if total_failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn render_pretty(results: &[TestFileResult], nocapture: bool) {
    let mut total_passed = 0u32;
    let mut total_failed = 0u32;
    let mut total_pending = 0u32;
    for r in results {
        // Per-test report (progress dots / documentation lines) is emitted
        // by std.test.Runner on stderr — always surface it.
        if !r.stderr.is_empty() {
            eprint!("{}", r.stderr);
            if !r.stderr.ends_with('\n') {
                eprintln!();
            }
        }
        // Failure diagnostics (matcher messages) go to stdout — surface
        // them on failure (or when --nocapture is set).
        if (nocapture || r.failed > 0 || !r.exit_ok) && !r.stdout.is_empty() {
            println!("--- {} ---", r.test_path);
            print!("{}", r.stdout);
            if !r.stdout.ends_with('\n') {
                println!();
            }
        }
        total_passed += r.passed;
        total_failed += r.failed;
        total_pending += r.pending;
    }
    println!(
        "\ntest result: {}. {} passed; {} failed; {} pending",
        if total_failed == 0 { "ok" } else { "FAILED" },
        total_passed,
        total_failed,
        total_pending
    );
}

fn resolve_test_threads(s: &str) -> usize {
    if s == "auto" {
        std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(1)
    } else {
        s.parse::<usize>().ok().filter(|&n| n > 0).unwrap_or(1)
    }
}

/// Walk upward from CWD until we find a Ruxen.toml.
fn find_project_root() -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir().map_err(|e| format!("cannot read cwd: {}", e))?;
    loop {
        if dir.join("Ruxen.toml").exists() {
            return Ok(dir);
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => return Err("no Ruxen.toml found in CWD or any ancestor".into()),
        }
    }
}

/// Collect every `.rx` file under `<project_dir>/tests/` EXCEPT those
/// under `tests/support/` (helper modules — see spec §4.3).
fn discover_test_files(project_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let tests_dir = project_dir.join("tests");
    if !tests_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    walk(&tests_dir, &tests_dir, &mut out);
    out.sort();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if dir == root && p.file_name() == Some(std::ffi::OsStr::new("support")) {
                continue;
            }
            walk(root, &p, out);
        } else if p.extension() == Some(std::ffi::OsStr::new("rx")) {
            out.push(p);
        }
    }
}

/// Convert `<project>/tests/foo/bar.rx` -> "foo.bar".
fn test_path_for(project_dir: &Path, file: &Path) -> String {
    let tests = project_dir.join("tests");
    let rel = file.strip_prefix(&tests).unwrap_or(file);
    let mut s = rel.with_extension("").to_string_lossy().into_owned();
    if std::path::MAIN_SEPARATOR != '.' {
        s = s.replace(std::path::MAIN_SEPARATOR, ".");
    }
    s
}

/// Flat-merge a *library* project's own `src/**.rx` so the test binary
/// can reference the project's classes and free fns. `ruxenc` is a
/// single-file driver (it does not resolve project deps), so without
/// this a test file can only see std-prelude symbols — making it
/// impossible to test the very library that owns the `tests/` dir.
///
/// Mirrors `ruxen_cli`'s `gather_sources` ordering: entry (`src/lib.rx`)
/// first, then the remaining `src/**.rx` sorted by path. We only merge
/// for **library** projects (those with `src/lib.rx`); a `src/main.rx`
/// binary project would inject its own `def main`, clashing with the
/// synthesised one, so we skip it and return `None`.
fn gather_project_lib_sources(project_dir: &Path) -> Result<Option<String>, String> {
    gather_package_lib_src(project_dir)
}

/// Gather one package's library source (`src/lib.rx` entry first, then the
/// rest of `src/**.rx` sorted by path). Returns `None` for a package with
/// no `src/lib.rx` (a binary package — merging it would inject a clashing
/// `def main`). Shared by the project gatherer and the Q16 dep gatherer.
fn gather_package_lib_src(package_dir: &Path) -> Result<Option<String>, String> {
    let lib = package_dir.join("src").join("lib.rx");
    if !lib.is_file() {
        return Ok(None);
    }
    let mut combined =
        fs::read_to_string(&lib).map_err(|e| format!("read {}: {}", lib.display(), e))?;
    combined.push('\n');

    let mut others = Vec::new();
    collect_rx(&package_dir.join("src"), &lib, &mut others);
    others.sort();
    for f in &others {
        let src = fs::read_to_string(f).map_err(|e| format!("read {}: {}", f.display(), e))?;
        combined.push_str(&src);
        combined.push('\n');
    }
    Ok(Some(combined))
}

/// Q16: flat-merge every dependency package's library source ahead of the
/// project's own source, so a `tests/**.rx` file can reference dependency
/// symbols — the same flat-merge a binary/library build performs. `dep_dirs`
/// is the caller-resolved, topologically-sorted dependency source-dir list
/// (empty when there are no deps or when `ruxenc` runs single-file).
fn gather_dep_lib_sources(dep_dirs: &[PathBuf]) -> Result<String, String> {
    let mut combined = String::new();
    for dir in dep_dirs {
        if let Some(src) = gather_package_lib_src(dir)? {
            combined.push_str(&src);
            combined.push('\n');
        }
    }
    Ok(combined)
}

/// Q18: split a test file into (top-level items to hoist ABOVE the
/// synthesised `def main`, test statements to keep INSIDE it).
///
/// A column-0 line beginning a top-level item — `use` / `const` / `type`
/// (single-line), or a block item `def` / `pub def` / `class` / `struct` /
/// `enum` / `mixin` / `module` / `extension` / `impl` (spanning to its
/// column-0 `end`) — plus any `##` doc-comment lines immediately preceding
/// such an item, is hoisted. A `##` doc that is NOT attached to a hoisted
/// item is neutralised to a plain `#` comment so it doesn't parse as a
/// statement inside `main`. Everything else stays in the main body.
fn split_test_body(body: &str) -> (String, String) {
    let lines: Vec<&str> = body.lines().collect();
    let mut hoisted = String::new();
    let mut main_body = String::new();

    let is_block_item_start = |t: &str| {
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let t = t.strip_prefix("protected ").unwrap_or(t);
        let t = t.strip_prefix("private ").unwrap_or(t);
        [
            "def ",
            "class ",
            "struct ",
            "enum ",
            "mixin ",
            "module ",
            "extension ",
            "impl ",
        ]
        .iter()
        .any(|kw| t.starts_with(kw))
            || t == "def"
    };
    let is_single_line_item =
        |t: &str| t.starts_with("use ") || t.starts_with("const ") || t.starts_with("type ");

    let mut i = 0;
    // Doc-comment lines pending attachment to the NEXT line.
    let mut pending_docs: Vec<&str> = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        let col0 = !line.starts_with([' ', '\t']);
        let trimmed = line.trim_start();

        // Accumulate column-0 doc comments; decide where they go once we see
        // the line they precede.
        if col0 && trimmed.starts_with("##") {
            pending_docs.push(line);
            i += 1;
            continue;
        }

        if col0 && is_single_line_item(trimmed) {
            // `use`/`const`/`type` don't carry the file-level docs that
            // typically precede them — neutralise those into the main body
            // so a stray `## …` never lands as a top-level doc.
            for d in pending_docs.drain(..) {
                main_body.push_str(&d.replacen("##", "#", 1));
                main_body.push('\n');
            }
            hoisted.push_str(line);
            hoisted.push('\n');
            i += 1;
            continue;
        }

        if col0 && is_block_item_start(trimmed) {
            for d in pending_docs.drain(..) {
                hoisted.push_str(d);
                hoisted.push('\n');
            }
            // Consume through the matching column-0 `end`.
            hoisted.push_str(line);
            hoisted.push('\n');
            i += 1;
            while i < lines.len() {
                let l = lines[i];
                hoisted.push_str(l);
                hoisted.push('\n');
                i += 1;
                if !l.starts_with([' ', '\t']) && l.trim() == "end" {
                    break;
                }
            }
            continue;
        }

        // Not a hoisted item — flush any pending docs as PLAIN comments
        // (a `##` doc in statement position is a parse error inside `main`).
        for d in pending_docs.drain(..) {
            let neutralised = d.replacen("##", "#", 1);
            main_body.push_str(&neutralised);
            main_body.push('\n');
        }
        main_body.push_str(line);
        main_body.push('\n');
        i += 1;
    }
    // Trailing docs with nothing after them → neutralise into the main body.
    for d in pending_docs.drain(..) {
        main_body.push_str(&d.replacen("##", "#", 1));
        main_body.push('\n');
    }

    (hoisted, main_body)
}

/// Recursively collect every `.rx` under `dir` except `skip` (the entry).
fn collect_rx(dir: &Path, skip: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_rx(&p, skip, out);
        } else if p.extension() == Some(std::ffi::OsStr::new("rx")) && p != skip {
            out.push(p);
        }
    }
}

/// Generate the wrapper .rx file that compiles to the per-file test
/// binary. Layout:
///
///   use std.test.Tester
///   use std.test.Runner
///   <project src/**.rx merged at top level, for library projects>
///   def main
///     let r = Runner.new("<test_path>")
///     Runner.set_current(r.handle_addr)
///     <user file body verbatim>
///     r.execute
///   end
fn synthesise_wrapper(
    project_dir: &Path,
    test_path: &str,
    user_file: &Path,
    out_dir: &Path,
    dep_dirs: &[PathBuf],
) -> Result<PathBuf, String> {
    let body = fs::read_to_string(user_file)
        .map_err(|e| format!("read {}: {}", user_file.display(), e))?;

    // Q16: dependency sources first, then the project's own library source,
    // then (below) the test body. Same flat-merge order as a binary build.
    let dep_src = gather_dep_lib_sources(dep_dirs)?;
    let project_src = gather_project_lib_sources(project_dir)?.unwrap_or_default();
    let project_src = format!("{dep_src}{project_src}");

    // Q18: a test file mixes TOP-LEVEL items (helper `def`s, `use`s, type
    // defs, `##` docs) with TEST STATEMENTS (`Tester.describe …`). Wrapping
    // the whole file in `def main` nests the items inside `main` — a
    // top-level `def`'s `end` closes `main` early, a `use` is a syntax error
    // mid-fn, and a leading `##` doc parses as a statement. Split the body:
    // hoist top-level items ABOVE `def main`, keep the test statements inside.
    let (hoisted_items, main_body) = split_test_body(&body);

    let prelude = format!(
        "# AUTO-GENERATED from {} — do not edit.\n\
         use std.test.Tester\n\
         use std.test.Runner\n\
         \n\
         {project_src}\n\
         {hoisted_items}\n\
         def main\n  \
           let r = Runner.new(\"{}\")\n  \
           Runner.set_current(r.handle_addr)\n",
        user_file.display(),
        test_path.replace('"', "\\\""),
    );
    let postlude = "\n  r.execute\nend\n";

    let synth = format!("{prelude}{main_body}\n{postlude}");

    fs::create_dir_all(out_dir).map_err(|e| format!("mkdir {}: {}", out_dir.display(), e))?;
    let synth_path = out_dir.join(format!("{}.synth.rx", test_path.replace('.', "_")));
    fs::write(&synth_path, &synth).map_err(|e| format!("write {}: {}", synth_path.display(), e))?;
    Ok(synth_path)
}

/// Compile one synthesised wrapper. Returns the path to the produced
/// binary. Called serially because the incremental cache's manifest.bin
/// rename is not thread-safe.
fn build_one(
    project_dir: &Path,
    test_path: &str,
    user_file: &Path,
    out_dir: &Path,
    release: bool,
    dep_dirs: &[PathBuf],
) -> Result<PathBuf, String> {
    let synth = synthesise_wrapper(project_dir, test_path, user_file, out_dir, dep_dirs)?;
    let profile = if release { "release" } else { "debug" };
    let bin_dir = project_dir.join("target").join(profile).join("test");
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    let bin_path = bin_dir.join(test_path.replace('.', "_"));

    let mut compile_args = vec![
        "ruxenc".to_string(),
        synth.to_string_lossy().into_owned(),
        "-o".to_string(),
        bin_path.to_string_lossy().into_owned(),
    ];
    if release {
        compile_args.push("--release".to_string());
    }
    // The project's own `runtime/*.c` must link into every test binary so
    // `lib "runtime/foo.c"` declarations in library code resolve — the same
    // discovery `ruxen build` performs (codegen::find_runtime_sources_in_dir).
    for c in ruxen_core::codegen::find_runtime_sources_in_dir(project_dir)? {
        compile_args.push(format!("--runtime-c={}", c.display()));
    }
    // Q32: a flat-merged FFI dependency's `.rx` bodies are compiled INTO this
    // test binary (Q16), so its C runtime symbols (`ruxen_*` from
    // `runtime/**.c`) and its `[system_libs]` link needs must reach this
    // executable's link line too — they aren't in any rlib. Mirror exactly
    // what `compile_project` (the `ruxen build` binary path) gathers for a
    // dependency: each dep's `runtime/**.c` plus each dep's declared
    // `[system_libs]` as `-l<lib>` link args. Without this, a consumer whose
    // `tests/**.rx` calls through an FFI dep fails at link with undefined
    // `ruxen_*` symbols (the canvas `ruxen_canvas_*` shape).
    for dep_dir in dep_dirs {
        for c in ruxen_core::codegen::find_runtime_sources_in_dir(dep_dir)? {
            compile_args.push(format!("--runtime-c={}", c.display()));
        }
        let dep_toml = dep_dir.join("Ruxen.toml");
        if let Ok(contents) = fs::read_to_string(&dep_toml) {
            for lib in ruxen_core::codegen::parse_system_libs(&contents) {
                compile_args.push(format!("--link-arg=-l{}", lib));
            }
        }
    }
    crate::compile::run(&compile_args).map_err(|e| format!("compile of {test_path}: {e}"))?;
    Ok(bin_path)
}

/// Spawn one previously-built test binary, capture stdout/stderr,
/// parse the summary line, return a TestFileResult. Safe to call
/// in parallel (each binary is its own process).
fn run_one(test_path: &str, bin_path: &Path, format: &str) -> TestFileResult {
    let output = match Command::new(bin_path)
        .env("RUXEN_TEST_FORMAT", format)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            return TestFileResult {
                test_path: test_path.to_string(),
                passed: 0,
                failed: 1,
                pending: 0,
                stdout: String::new(),
                stderr: format!("spawn {}: {}", bin_path.display(), e),
                exit_ok: false,
            };
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let (passed, failed, pending) = parse_summary_line(&stdout);

    TestFileResult {
        test_path: test_path.to_string(),
        passed,
        failed,
        pending,
        stdout,
        stderr,
        exit_ok: output.status.success(),
    }
}

/// Parse the Runner.execute summary line "N passed, M failed, K pending".
/// We look for the LAST line containing all three keywords so any
/// user-level diagnostic that happens to contain "passed" doesn't
/// shadow the canonical summary.
fn parse_summary_line(stdout: &str) -> (u32, u32, u32) {
    for line in stdout.lines().rev() {
        let s = line.trim();
        if !s.contains("passed") || !s.contains("failed") || !s.contains("pending") {
            continue;
        }
        let parts: Vec<&str> = s
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() >= 3 {
            return (
                parts[0].parse().unwrap_or(0),
                parts[1].parse().unwrap_or(0),
                parts[2].parse().unwrap_or(0),
            );
        }
    }
    (0, 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Q18: top-level items (use / def / type defs / docs) hoist ABOVE the
    // synthesised `def main`; test statements stay inside it; an unattached
    // `##` doc is neutralised to `#`.
    #[test]
    fn split_test_body_hoists_top_level_items() {
        let body = "\
## file doc that breaks
use own_pkg.Helper

def make -> Int
  let x = 1
  x
end

Tester.describe(\"x\") do
  it(\"works\")
end
";
        let (hoisted, main_body) = split_test_body(body);
        assert!(
            hoisted.contains("use own_pkg.Helper"),
            "use hoisted: {hoisted}"
        );
        assert!(
            hoisted.contains("def make -> Int"),
            "def hoisted: {hoisted}"
        );
        assert!(
            hoisted.contains("\n  x\nend"),
            "def body+end hoisted: {hoisted}"
        );
        assert!(
            main_body.contains("Tester.describe"),
            "test stmt stays in main: {main_body}"
        );
        assert!(
            !main_body.contains("use own_pkg"),
            "use must not remain in main: {main_body}"
        );
        assert!(
            !main_body.contains("def make"),
            "def must not remain in main: {main_body}"
        );
        // The unattached file doc became a plain `#` comment in main.
        assert!(
            main_body.contains("# file doc that breaks") && !main_body.contains("## file doc"),
            "stray doc neutralised: {main_body}"
        );
    }
    use std::fs;

    #[test]
    fn synthesise_wraps_user_body_with_runner() {
        let tmp = std::env::temp_dir().join("test-runner-synth-1");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let user_file = tmp.join("user.rx");
        fs::write(
            &user_file,
            "Tester.describe(\"X\") do |t: &var Tester|\n  t.it(\"y\") do\n    t.expect(1).to_eq(1)\n  end\nend",
        )
        .unwrap();
        let synth_path =
            synthesise_wrapper(&tmp, "foo.bar", &user_file, &tmp.join("out"), &[]).unwrap();
        let synth = fs::read_to_string(&synth_path).unwrap();
        assert!(synth.contains("def main"), "synth: {synth}");
        assert!(synth.contains("Runner.new(\"foo.bar\")"), "synth: {synth}");
        assert!(synth.contains("Tester.describe(\"X\")"), "synth: {synth}");
        assert!(synth.contains("r.execute"), "synth: {synth}");
    }

    #[test]
    fn synthesise_merges_library_source_at_top_level() {
        let tmp = std::env::temp_dir().join("test-runner-synth-lib");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(tmp.join("src/lib.rx"), "## pkg doc\n").unwrap();
        fs::write(
            tmp.join("src/widget.rx"),
            "class Widget\n  def answer -> Int\n    42\n  end\nend\n",
        )
        .unwrap();
        let user_file = tmp.join("widget_test.rx");
        fs::write(
            &user_file,
            "Tester.describe(\"W\") do |t: &var Tester|\n  t.it(\"y\") do\n    t.expect(Widget.new.answer).to_eq(42)\n  end\nend",
        )
        .unwrap();
        let synth_path =
            synthesise_wrapper(&tmp, "widget_test", &user_file, &tmp.join("out"), &[]).unwrap();
        let synth = fs::read_to_string(&synth_path).unwrap();
        // Library class must appear at top level, BEFORE the synthesised main.
        let class_at = synth.find("class Widget").expect("class merged");
        let main_at = synth.find("def main").expect("has main");
        assert!(class_at < main_at, "class must precede def main: {synth}");
    }

    // Q16: dependency library sources are flat-merged into the synthesised
    // test wrapper, ahead of the project's own source and before `def main`,
    // so a test file can reference a dependency symbol.
    #[test]
    fn synthesise_merges_dependency_source_before_project_and_main() {
        let tmp = std::env::temp_dir().join("test-runner-synth-dep");
        let _ = fs::remove_dir_all(&tmp);
        // Dependency package laid out like a real path dep.
        let dep_dir = tmp.join("dep-color");
        fs::create_dir_all(dep_dir.join("src")).unwrap();
        fs::write(
            dep_dir.join("src/lib.rx"),
            "pub struct Color\n  r: Int\nend\n",
        )
        .unwrap();

        // Consumer project with its own lib source.
        let proj = tmp.join("consumer");
        fs::create_dir_all(proj.join("src")).unwrap();
        fs::write(
            proj.join("src/lib.rx"),
            "pub def brightest -> Int\n  Color.new(255).r\nend\n",
        )
        .unwrap();

        let user_file = proj.join("color_test.rx");
        fs::write(
            &user_file,
            "Tester.describe(\"c\") do |t: &var Tester|\n  t.it(\"y\") do\n    t.expect(Color.new(255).r).to_eq(255)\n  end\nend",
        )
        .unwrap();

        let synth_path = synthesise_wrapper(
            &proj,
            "color_test",
            &user_file,
            &proj.join("out"),
            std::slice::from_ref(&dep_dir),
        )
        .unwrap();
        let synth = fs::read_to_string(&synth_path).unwrap();

        let dep_at = synth.find("struct Color").expect("dep struct merged");
        let proj_at = synth.find("def brightest").expect("project src merged");
        let main_at = synth.find("def main").expect("has main");
        // Order: dependency source → project source → def main.
        assert!(
            dep_at < proj_at && proj_at < main_at,
            "expected dep < project < main; got dep={dep_at} proj={proj_at} main={main_at}:\n{synth}"
        );
    }

    #[test]
    fn parse_summary_extracts_three_counts() {
        assert_eq!(
            parse_summary_line("noise\n3 passed, 1 failed, 2 pending\ntrailing\n"),
            (3, 1, 2)
        );
        assert_eq!(parse_summary_line("no summary"), (0, 0, 0));
    }

    #[test]
    fn discover_skips_support_dir() {
        let tmp = std::env::temp_dir().join("test-runner-discover-1");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("tests/support")).unwrap();
        fs::create_dir_all(tmp.join("tests/a")).unwrap();
        fs::write(
            tmp.join("Ruxen.toml"),
            "[package]\nname=\"p\"\nversion=\"0.0.1\"",
        )
        .unwrap();
        fs::write(tmp.join("tests/a/x.rx"), "").unwrap();
        fs::write(tmp.join("tests/support/helpers.rx"), "").unwrap();
        let files = discover_test_files(&tmp).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("a/x.rx"));
    }
}
