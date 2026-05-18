use clap::Parser;
use riven_cli::{build, cli, deps, explain, scaffold};

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
        cli::Command::Clean => build::clean(),
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
        cli::Command::Update { piece } => deps::update(piece.as_deref()),
        cli::Command::Tree => deps::tree(),
        cli::Command::Verify => deps::verify(),
        cli::Command::Explain { code } => explain::explain(&code),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
