//! `ruxenc fmt` / `ruxen fmt` — formatter CLI surface.
//!
//! Args layout: just the post-subcommand flags + positional files. The bare
//! `ruxenc fmt` invocation passes `&args[2..]` here; `ruxen fmt` builds the
//! same shape from the clap struct.
//!
//! Exit codes the original binary distinguished (1 = check-mode found drift,
//! 2 = errors) are collapsed to `Err(String)` here. The caller exits 1 on Err;
//! the specific 1-vs-2 nuance was undocumented externally and not depended on
//! by any in-tree CI.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

pub fn run(args: &[String]) -> Result<(), String> {
    let mut check_mode = false;
    let mut diff_mode = false;
    let mut stdin_mode = false;
    let mut filepath: Option<String> = None;
    let mut files: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--check" => check_mode = true,
            "--diff" => diff_mode = true,
            "--stdin" => stdin_mode = true,
            s if s.starts_with("--filepath=") => {
                filepath = Some(s[11..].to_string());
            }
            "--filepath" => {
                i += 1;
                if i >= args.len() {
                    return Err("--filepath requires a value".into());
                }
                filepath = Some(args[i].clone());
            }
            s if s.starts_with("--") => {
                return Err(format!("Unknown option: {}", s));
            }
            _ => files.push(args[i].clone()),
        }
        i += 1;
    }

    if stdin_mode {
        return run_fmt_stdin(filepath.as_deref());
    }

    // If no files specified, discover all .rx files recursively
    if files.is_empty() {
        files = discover_rx_files(".");
        if files.is_empty() {
            eprintln!("No .rx files found.");
            return Ok(());
        }
    }

    let mut any_changed = false;
    let mut any_errors = false;

    for path in &files {
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading '{}': {}", path, e);
                any_errors = true;
                continue;
            }
        };

        let result = ruxen_core::formatter::format(&source);

        if !result.errors.is_empty() {
            for err in &result.errors {
                eprintln!("{}: {}", path, err);
            }
            any_errors = true;
            continue;
        }

        if result.changed {
            any_changed = true;

            if check_mode {
                println!("{}", path);
            } else if diff_mode {
                print_diff(path, &source, &result.output);
            } else {
                // Write formatted output back to file
                if let Err(e) = fs::write(path, &result.output) {
                    eprintln!("Error writing '{}': {}", path, e);
                    any_errors = true;
                } else {
                    println!("Formatted {}", path);
                }
            }
        }
    }

    if any_errors {
        return Err("one or more files failed to format".into());
    }
    if check_mode && any_changed {
        return Err("formatting check failed: files would change".into());
    }
    Ok(())
}

fn run_fmt_stdin(filepath: Option<&str>) -> Result<(), String> {
    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .map_err(|e| format!("Error reading stdin: {}", e))?;

    let result = ruxen_core::formatter::format(&source);

    if !result.errors.is_empty() {
        let label = filepath.unwrap_or("<stdin>");
        for err in &result.errors {
            eprintln!("{}: {}", label, err);
        }
    }

    print!("{}", result.output);
    Ok(())
}

fn discover_rx_files(dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    discover_rx_files_recursive(Path::new(dir), &mut files);
    files.sort();
    files
}

fn discover_rx_files_recursive(dir: &Path, files: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden directories and common ignore patterns
        if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            discover_rx_files_recursive(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rx") {
            if let Some(s) = path.to_str() {
                files.push(s.to_string());
            }
        }
    }
}

fn print_diff(path: &str, original: &str, formatted: &str) {
    println!("--- {}", path);
    println!("+++ {}", path);

    let orig_lines: Vec<&str> = original.lines().collect();
    let fmt_lines: Vec<&str> = formatted.lines().collect();

    // Simple line-by-line diff
    let max_lines = orig_lines.len().max(fmt_lines.len());
    let mut in_hunk = false;
    let mut hunk_start = 0;

    for i in 0..max_lines {
        let orig_line = orig_lines.get(i).copied().unwrap_or("");
        let fmt_line = fmt_lines.get(i).copied().unwrap_or("");

        if orig_line != fmt_line {
            if !in_hunk {
                hunk_start = i;
                let context_start = i.saturating_sub(2);
                println!(
                    "@@ -{},{} +{},{} @@",
                    context_start + 1,
                    3.min(orig_lines.len() - context_start),
                    context_start + 1,
                    3.min(fmt_lines.len() - context_start)
                );
                // Context lines before
                for j in context_start..i {
                    if let Some(l) = orig_lines.get(j) {
                        println!(" {}", l);
                    }
                }
                in_hunk = true;
            }
            if i < orig_lines.len() {
                println!("-{}", orig_line);
            }
            if i < fmt_lines.len() {
                println!("+{}", fmt_line);
            }
        } else if in_hunk {
            // Print some context after the hunk
            println!(" {}", orig_line);
            if i - hunk_start > 5 {
                in_hunk = false;
            }
        }
    }
}
