//! `rivenc bench` / `riven bench` — microbenchmark harness.
//!
//! `bench <file.rvn>` finds every `def bench_*(b: &var Bencher)` in the
//! file, appends a synthesised `def main` that runs each one against a fresh
//! `Bencher`, and compiles+runs the result. Pure-Riven harness — see
//! `library/std/bench/src/lib.rvn`. No compiler-side parser/keyword work;
//! bench fns are identified by name convention (`bench_*`) per
//! feedback_pure_riven_first.md.
//!
//! Args layout: just the post-subcommand flags + positional file. Bench-
//! specific exit codes (124 on timeout) are collapsed to `Err(String)` — the
//! caller exits 1 uniformly on Err.

use std::fs;
use std::path::Path;
use std::process;

use riven_core::lexer::Lexer;
use riven_core::parser::Parser;

pub fn run(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("Usage: bench <file.rvn> [--filter <pat>] [--iter-hint <N>]".into());
    }

    let mut path: Option<String> = None;
    let mut filter: Option<String> = None;
    let mut iter_hint: i64 = 100;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--filter" => {
                i += 1;
                if i >= args.len() {
                    return Err("--filter requires a value".into());
                }
                filter = Some(args[i].clone());
            }
            "--iter-hint" => {
                i += 1;
                if i >= args.len() {
                    return Err("--iter-hint requires a value".into());
                }
                iter_hint = args[i]
                    .parse()
                    .map_err(|_| "--iter-hint must be a positive integer".to_string())?;
            }
            s if s.starts_with("--") => {
                return Err(format!("Unknown bench option: {}", s));
            }
            _ => {
                if path.is_some() {
                    return Err(format!("Unexpected positional arg: {}", args[i]));
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let path = path.ok_or_else(|| "bench: missing <file.rvn>".to_string())?;

    if !path.ends_with(".rvn") {
        return Err(format!("bench: expected a .rvn file, got: {}", path));
    }

    let source =
        fs::read_to_string(&path).map_err(|e| format!("bench: cannot read {}: {}", path, e))?;

    // Parse just to collect bench-fn names. Diagnostics here are pure
    // syntax / item-shape — full typeck happens on the synthesised
    // file below, so a typo in a bench body will surface there with
    // the normal codepath.
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    // Collect bench fns: name convention `bench_*`, exactly one
    // parameter. The Bencher-receiver type-check happens at the real
    // typeck pass — we filter coarsely here so a fn with the wrong
    // signature still surfaces a meaningful error from the synthesised
    // main's call site instead of being silently dropped.
    use riven_core::parser::ast::TopLevelItem;
    let mut bench_names: Vec<String> = Vec::new();
    let mut has_main = false;
    for item in &program.items {
        if let TopLevelItem::Function(f) = item {
            if f.name == "main" {
                has_main = true;
            }
            if f.name.starts_with("bench_") && f.params.len() == 1 {
                if let Some(pat) = &filter {
                    if !f.name.contains(pat.as_str()) {
                        continue;
                    }
                }
                // Defence-in-depth: the parser already restricts function
                // identifiers to `[A-Za-z_][A-Za-z0-9_]*`, but the bench
                // synthesiser below text-splices `f.name` straight into a
                // generated `def main` body. A future parser bug that let
                // a non-identifier through would become arbitrary Riven
                // injection. Re-validate at the trust boundary.
                if !is_valid_riven_identifier(&f.name) {
                    return Err(format!(
                        "bench: function name `{}` is not a valid Riven identifier; \
                         refusing to splice into synthesised runner main",
                        f.name
                    ));
                }
                bench_names.push(f.name.clone());
            }
        }
    }

    if has_main {
        return Err(format!(
            "bench: {} declares `def main` — bench files must let the runner provide main. \
             Remove or rename the existing main and re-run.",
            path
        ));
    }

    if bench_names.is_empty() {
        return Err(format!(
            "bench: no `def bench_*(b: &var Bencher)` functions found{}.",
            filter
                .as_ref()
                .map(|p| format!(" matching `{}`", p))
                .unwrap_or_default()
        ));
    }

    // Synthesise a `def main` that walks each collected bench fn
    // against a fresh `Bencher`. Appended at the end of the source
    // so resolve/typeck sees all bench-fn definitions before the
    // call sites.
    let mut synth = source.clone();
    if !synth.ends_with('\n') {
        synth.push('\n');
    }
    synth.push_str("\n# ── rivenc bench: synthesised runner main ────────────────────\n");
    synth.push_str("def main\n");
    synth.push_str(&format!("  var bencher = Bencher.new({})\n", iter_hint));
    for name in &bench_names {
        synth.push_str(&format!("  {}(&var bencher)\n", name));
    }
    synth.push_str("end\n");

    // Write synth to a tmp file alongside the original so error
    // messages cite a stable path; compile + run through the normal
    // pipeline.
    let tmp_dir = std::env::temp_dir().join("rivenc-bench");
    fs::create_dir_all(&tmp_dir)
        .map_err(|e| format!("bench: cannot create {}: {}", tmp_dir.display(), e))?;
    let stem = Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bench");
    let synth_path = tmp_dir.join(format!("{}.bench.rvn", stem));
    let bin_path = tmp_dir.join(format!("{}.bench", stem));
    fs::write(&synth_path, &synth)
        .map_err(|e| format!("bench: cannot write {}: {}", synth_path.display(), e))?;

    // Forward to the normal compile path, then exec the binary.
    // compile::run expects args[0] = program name (ignored), args[1] = path.
    let compile_args = vec![
        "rivenc".to_string(),
        synth_path.to_string_lossy().into_owned(),
        "-o".to_string(),
        bin_path.to_string_lossy().into_owned(),
        "--force".to_string(),
    ];
    crate::compile::run(&compile_args)?;

    // Safety timeout — a stuck bench (e.g. a fixture that locks
    // without unlocking due to a drop-elaboration regression) would
    // otherwise hang `bench` indefinitely. Auto-scale within
    // `Bencher.iter` already caps total wall time at ≥ 100 ms per
    // bench, so even a slow real bench finishes in a few seconds;
    // anything past `bench_timeout` is treated as a deadlock or
    // pathological auto-scale path and killed. Override via env
    // `RIVENC_BENCH_TIMEOUT_SECS` (default 60). 0 disables.
    let bench_timeout: u64 = std::env::var("RIVENC_BENCH_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let output = if bench_timeout == 0 {
        process::Command::new(&bin_path).output()
    } else {
        let mut child = process::Command::new(&bin_path)
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("bench: failed to spawn {}: {}", bin_path.display(), e))?;
        let pid = child.id();
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break child.wait_with_output(),
                Ok(None) => {
                    if start.elapsed().as_secs() >= bench_timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!(
                            "bench: bench process pid={} exceeded RIVENC_BENCH_TIMEOUT_SECS={}s and was killed. \
                             A common cause is a drop-elaboration bug that leaves a resource (mutex / fd / refcount) \
                             held across iterations — re-run the fixture standalone to bisect.",
                            pid, bench_timeout
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => break Err(e),
            }
        }
    };

    let output =
        output.map_err(|e| format!("bench: failed to run {}: {}", bin_path.display(), e))?;
    if !output.stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        return Err(format!(
            "bench: child process exited with status {}",
            output.status.code().unwrap_or(1)
        ));
    }
    Ok(())
}

/// Conservative identifier check: `[A-Za-z_][A-Za-z0-9_]*`. Matches the
/// lexer's own definition of a bare identifier; refusing anything else
/// keeps the bench synthesiser's text-splice safe under any future
/// parser bug.
fn is_valid_riven_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
