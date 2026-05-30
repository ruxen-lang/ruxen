use clap::Parser;
use ruxen_cli::{build, cli, deps, explain, publish, scaffold, self_update};

fn main() {
    let args = cli::Cli::parse();

    let result = match args.command {
        cli::Command::New { name, lib, no_git } => scaffold::new_project(&name, lib, no_git),
        cli::Command::Init => scaffold::init_project(),
        cli::Command::Build {
            release,
            locked,
            bin,
        } => build::build(release, locked, bin.as_deref()),
        cli::Command::Run { release, args } => build::run(release, args),
        cli::Command::Check => build::check(),
        // `ruxen clean` delegates to the ruxenc library so there is a single
        // source of truth for cache cleanup. `ruxenc::clean::run(&[])` clears
        // `target/ruxen/incremental/` for the current project; pass
        // `--global` for the user-wide cache.
        cli::Command::Clean => ruxenc::clean::run(&[]),
        cli::Command::Add {
            piece,
            version,
            git,
            path,
            dev,
            branch,
            tag,
            rev,
        } => deps::add(
            &piece,
            version.as_deref(),
            git.as_deref(),
            path.as_deref(),
            dev,
            branch.as_deref(),
            tag.as_deref(),
            rev.as_deref(),
        ),
        cli::Command::Remove { piece } => deps::remove(&piece),
        cli::Command::Update { piece, precise } => {
            deps::update(piece.as_deref(), precise.as_deref())
        }
        cli::Command::Upgrade {
            from_source,
            version,
        } => match from_source {
            Some(src) => {
                if version.is_some() {
                    eprintln!(
                        "  warning: ignoring `--version` — \
                         `--from-source` builds the checkout as-is"
                    );
                }
                self_update::from_source(&src)
            }
            None => self_update::from_release(version.as_deref()),
        },
        cli::Command::Tree => deps::tree(),
        cli::Command::Verify => deps::verify(),
        cli::Command::Explain { code } => explain::explain(&code),

        // ── Low-level compiler subcommands ──────────────────────────
        // Each builds the legacy positional-args vector that the ruxenc
        // library entry points expect (compile::run takes args[0] = program
        // name (ignored), args[1] = file path, then opts; fmt/bench/clean
        // take just the post-subcommand flag tail).
        cli::Command::Compile {
            file,
            output,
            extra,
        } => {
            let mut argv = vec!["ruxen".to_string(), file];
            if let Some(out) = output {
                argv.push("-o".to_string());
                argv.push(out);
            }
            argv.extend(extra);
            ruxenc::compile::run(&argv)
        }
        cli::Command::Fmt {
            files,
            check,
            diff,
            stdin,
            filepath,
        } => {
            let mut argv: Vec<String> = Vec::new();
            if check {
                argv.push("--check".to_string());
            }
            if diff {
                argv.push("--diff".to_string());
            }
            if stdin {
                argv.push("--stdin".to_string());
            }
            if let Some(fp) = filepath {
                argv.push(format!("--filepath={}", fp));
            }
            argv.extend(files);
            ruxenc::fmt::run(&argv)
        }
        cli::Command::Bench {
            file,
            filter,
            iter_hint,
        } => {
            let mut argv = vec![file];
            if let Some(pat) = filter {
                argv.push("--filter".to_string());
                argv.push(pat);
            }
            if let Some(n) = iter_hint {
                argv.push("--iter-hint".to_string());
                argv.push(n.to_string());
            }
            ruxenc::bench::run(&argv)
        }
        cli::Command::Test {
            filter,
            release,
            test_threads,
            fail_fast,
            nocapture,
            list,
            no_run,
            include_pending,
            format,
        } => ruxenc::test_runner::run(ruxenc::test_runner::TestOptions {
            filter,
            release,
            test_threads,
            fail_fast,
            nocapture,
            list,
            no_run,
            include_pending,
            format,
        }),

        // ── Editor / interactive subcommands ────────────────────────
        // Both crates expose a `run() -> Result<(), String>` library
        // entry point; the unified `ruxen` driver is the only shipped
        // binary that invokes them.
        cli::Command::Lsp => ruxen_lsp::run(),
        cli::Command::Repl => ruxen_repl::run(),

        cli::Command::Publish { dry_run, registry } => {
            publish::publish(dry_run, registry.as_deref())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
