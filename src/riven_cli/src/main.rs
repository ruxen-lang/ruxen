use clap::Parser;
use riven_cli::{build, cli, deps, explain, scaffold, self_update};

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
        // `riven clean` delegates to the rivenc library so there is a single
        // source of truth for cache cleanup. `rivenc::clean::run(&[])` clears
        // `target/riven/incremental/` for the current project; pass
        // `--global` for the user-wide cache.
        cli::Command::Clean => rivenc::clean::run(&[]),
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
        cli::Command::Update { piece, from_source } => {
            if let Some(src) = from_source {
                if piece.is_some() {
                    eprintln!(
                        "  warning: ignoring `<piece>` argument — \
                         `--from-source` is a toolchain self-update"
                    );
                }
                self_update::from_source(&src)
            } else {
                deps::update(piece.as_deref())
            }
        }
        cli::Command::Tree => deps::tree(),
        cli::Command::Verify => deps::verify(),
        cli::Command::Explain { code } => explain::explain(&code),

        // ── Low-level compiler subcommands ──────────────────────────
        // Each builds the legacy positional-args vector that the rivenc
        // library entry points expect (compile::run takes args[0] = program
        // name (ignored), args[1] = file path, then opts; fmt/bench/clean
        // take just the post-subcommand flag tail).
        cli::Command::Compile {
            file,
            output,
            extra,
        } => {
            let mut argv = vec!["riven".to_string(), file];
            if let Some(out) = output {
                argv.push("-o".to_string());
                argv.push(out);
            }
            argv.extend(extra);
            rivenc::compile::run(&argv)
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
            rivenc::fmt::run(&argv)
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
            rivenc::bench::run(&argv)
        }
        cli::Command::Test { filter: _ } => {
            eprintln!(
                "`riven test` not yet implemented — decisions locked in \
                 docs/prompts/v1/19_phase5_test_framework.md; implementation deferred."
            );
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
