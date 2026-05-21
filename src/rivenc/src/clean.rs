//! `rivenc clean` / `riven clean` — cache cleaner.
//!
//! Args layout: just the post-subcommand flags. `--global` clears
//! `~/.cache/riven/`; otherwise clears `target/riven/incremental/` for the
//! current project (resolved by walking upward to find Cargo.toml/riven.toml).

use crate::cache::{self, clear_global_cache, CacheStore};

use crate::compile::project_target_riven;

pub fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--global") {
        clear_global_cache().map_err(|e| format!("Failed to clean global cache: {}", e))?;
        println!(
            "Cleaned global cache at {}",
            cache::global_cache_dir().display()
        );
        return Ok(());
    }

    let store = CacheStore::new(project_target_riven());
    store
        .clear()
        .map_err(|e| format!("Failed to clean cache: {}", e))?;
    println!("Cleaned {}", store.incremental_dir().display());
    Ok(())
}
