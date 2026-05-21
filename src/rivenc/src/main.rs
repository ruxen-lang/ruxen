//! `rivenc` — low-level compiler binary.
//!
//! Thin dispatch shell. Every subcommand body lives in the `rivenc` library
//! (`compile`, `fmt`, `clean`, `bench`); both this binary and the unified
//! `riven` driver call the same library code. Errors propagate as
//! `Result<(), String>` and become `exit(1)` here.

use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    let result: Result<(), String> = match args[1].as_str() {
        "fmt" => rivenc::fmt::run(&args[2..]),
        "clean" => rivenc::clean::run(&args[2..]),
        "bench" => rivenc::bench::run(&args[2..]),
        "--version" | "-V" => {
            println!("rivenc {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        // Bare invocation: `rivenc <file.rvn> [opts...]`.
        // compile::run expects args[0] = program name (ignored), args[1] = path.
        _ => rivenc::compile::run(&args),
    };

    if let Err(e) = result {
        eprintln!("{}", e);
        process::exit(1);
    }
}

fn print_usage() {
    eprintln!("Usage: rivenc <file.rvn> [options]");
    eprintln!("       rivenc fmt [options] [files...]");
    eprintln!("       rivenc clean [--global]");
    eprintln!("       rivenc bench <file.rvn> [--filter <pat>] [--iter-hint <N>]");
    eprintln!();
    eprintln!("Compiler options:");
    eprintln!("  -o <output>           Specify output file name");
    eprintln!("  --emit=tokens|ast|hir|mir   Dump intermediate stage and exit");
    eprintln!("  --release             Use LLVM backend with O2 optimization");
    eprintln!("  --backend=cranelift|llvm    Force backend");
    eprintln!("  --opt-level=0|1|2|3|s|z     Set optimization level");
    eprintln!("  --force               Ignore all caches and recompile");
    eprintln!("  --verbose             Emit [cache] log lines");
}
