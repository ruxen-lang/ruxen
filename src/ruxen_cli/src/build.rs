//! Build pipeline: `ruxen build`, `ruxen run`, `ruxen check`, `ruxen clean`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ruxen_core::codegen::target::ResolvedTarget;
use ruxen_core::codegen::{self, Backend};
use ruxen_core::driver::Options;
use ruxen_core::mir::nodes::MirProgram;
use ruxen_core::module_loader::{ModuleLoader, SourceBundle};
use ruxenc::compile;

use crate::lock::LockFile;
use crate::manifest::Manifest;
use crate::module_discovery::ModuleTree;
use crate::resolve_deps;
use crate::rlib::{self, TypeMetadata};

/// `ruxen build [--release] [--locked] [--bin <name>] [--target <triple>]`
pub fn build(
    release: bool,
    locked: bool,
    bin_name: Option<&str>,
    target: Option<&str>,
) -> Result<(), String> {
    let start = Instant::now();
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;
    manifest.validate()?;
    let package = manifest.require_package()?.clone();

    // tier 4.02: resolve the cross-compilation target. CLI `--target` wins;
    // otherwise fall back to `[build] target` in the manifest; otherwise host.
    let target_str: Option<String> = target
        .map(|s| s.to_string())
        .or_else(|| manifest.build.as_ref().and_then(|b| b.target.clone()));
    let resolved_target = ResolvedTarget::resolve(target_str.as_deref())?;
    let opt_level = manifest.opt_level(release).map(|level| level.to_string());
    let backend = Backend::select(release, None, opt_level.as_deref(), &resolved_target)?;

    // Advisory: dev-dependencies are parsed + survive `ruxen add --dev`
    // / `ruxen remove`, but `ruxen build` does not yet compile them
    // (no test runner consumer exists in v1).  Tell the user up front
    // so a misspelled dev-dep doesn't fail silently after a
    // successful build.  v1.next plan: a `ruxen test` subcommand that
    // builds dev-deps alongside the project.
    if !manifest.dev_dependencies.is_empty() {
        let names: Vec<&str> = manifest
            .dev_dependencies
            .keys()
            .map(|s| s.as_str())
            .collect();
        eprintln!(
            "  warning: [dev-dependencies] ({}) are recorded in Ruxen.toml but not \
             compiled by `ruxen build` — `ruxen test` (v1.next) will consume them.",
            names.join(", ")
        );
    }

    let profile = if release { "release" } else { "debug" };
    // Shared `target/` semantics: when this project is a member of a
    // workspace, every member writes into `<workspace>/target/` so
    // shared deps are built once. Falls back to the project's own
    // `target/` for standalone projects.
    let target_root = find_target_root(&project_dir);
    // Per-target output dir (spec §5.7): `target/<triple>/<profile>/` for a
    // cross target; host stays `target/<profile>/` (back-compat — no triple
    // prefix), matching Cargo.
    let target_dir = if resolved_target.is_host() {
        target_root.join("target").join(profile)
    } else {
        target_root
            .join("target")
            .join(resolved_target.canonical())
            .join(profile)
    };
    fs::create_dir_all(&target_dir)
        .map_err(|e| format!("failed to create target directory: {}", e))?;
    fs::create_dir_all(target_dir.join("deps"))
        .map_err(|e| format!("failed to create deps directory: {}", e))?;

    // Step 1: Resolve dependencies
    let resolved = if !manifest.dependencies.is_empty() {
        let existing_lock = LockFile::load(&project_dir).ok();

        if locked {
            match &existing_lock {
                Some(lock) if lock.is_up_to_date(&manifest) => {}
                Some(_) => {
                    return Err("Ruxen.lock is out of date with Ruxen.toml.\n  \
                         Run `ruxen update` to regenerate the lock file."
                        .to_string());
                }
                None => {
                    return Err("Ruxen.lock not found but --locked was specified.\n  \
                         Run `ruxen build` first to generate the lock file."
                        .to_string());
                }
            }
        }

        // Build sibling-member map so `pkg-b = "0.1.0"` inside one
        // workspace member resolves to the sibling's source dir rather
        // than the registry-rejection path.
        let mut workspace_members: std::collections::BTreeMap<String, PathBuf> = Default::default();
        if let Some(ws_root) = crate::manifest::find_workspace_root(&project_dir) {
            let ws_manifest = Manifest::load(&ws_root)?;
            if let Some(ws) = ws_manifest.workspace.as_ref() {
                for (member_dir, member_name) in
                    crate::manifest::expand_workspace_members(&ws_root, &ws.members)?
                {
                    // Skip self — a member is not a dep of itself.
                    if member_dir != project_dir {
                        workspace_members.insert(member_name, member_dir);
                    }
                }
            }
        }
        let result = resolve_deps::resolve_with_workspace(
            &project_dir,
            &manifest,
            existing_lock.as_ref(),
            &workspace_members,
        )?;

        // Verify checksums
        result.lock.verify_checksums(&project_dir)?;

        // Save lock file
        result.lock.save(&project_dir)?;

        Some(result)
    } else {
        None
    };

    // Step 2: Compile dependencies in topological order
    let mut extern_libs: Vec<(String, PathBuf)> = Vec::new();

    if let Some(ref resolve_result) = resolved {
        for dep in &resolve_result.deps {
            println!("  Compiling piece `{}` v{}", dep.name, dep.version);
            let rlib_path = target_dir.join("deps").join(format!("{}.rlib", dep.name));
            // A dependency may itself depend on earlier-built packages —
            // flat-merge ITS transitive deps so its `src/**.rx` can
            // reference them (Q16).
            let dep_deps = transitive_dep_source_dirs(dep, &resolve_result.deps);
            compile_piece(
                &dep.source_dir,
                &dep.name,
                &dep.version,
                &rlib_path,
                backend,
                &extern_libs,
                &dep_deps,
                &resolved_target,
            )?;
            extern_libs.push((dep.name.clone(), rlib_path));
        }
    }

    // Step 3: Compile project source
    println!("  Compiling piece `{}` v{}", package.name, package.version);

    if manifest.build_type() == "library" {
        // Library: produce an .rlib. Flat-merge the full dependency
        // closure ahead of the library's own source so `src/**.rx` can
        // reference dependency symbols (Q16) — the same mechanism the
        // binary path uses, never an extern-rlib link.
        let rlib_path = target_dir.join(format!("{}.rlib", package.name));
        let dep_source_dirs: Vec<PathBuf> = resolved
            .as_ref()
            .map(|r| r.deps.iter().map(|d| d.source_dir.clone()).collect())
            .unwrap_or_default();
        compile_piece(
            &project_dir,
            &package.name,
            &package.version,
            &rlib_path,
            backend,
            &extern_libs,
            &dep_source_dirs,
            &resolved_target,
        )?;
    } else {
        // Binary: produce an executable
        let output_name = bin_name.unwrap_or(&package.name);
        let output_path = target_dir.join(output_name);
        let dep_source_dirs: Vec<PathBuf> = resolved
            .as_ref()
            .map(|r| r.deps.iter().map(|d| d.source_dir.clone()).collect())
            .unwrap_or_default();
        compile_project(
            &project_dir,
            &manifest,
            &output_path,
            backend,
            &extern_libs,
            &dep_source_dirs,
            &resolved_target,
        )?;
    }

    let elapsed = start.elapsed();
    println!(
        "    Finished {} target in {:.2}s",
        profile,
        elapsed.as_secs_f64()
    );

    Ok(())
}

/// `ruxen run [--release] [--target <triple>] [-- <args>]`
pub fn run(release: bool, target: Option<&str>, args: Vec<String>) -> Result<(), String> {
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;
    let package = manifest.require_package()?;

    if manifest.build_type() == "library" {
        return Err("cannot run a library project. Use `ruxen build` instead.".to_string());
    }

    // §3 non-goal: `ruxen run --target <non-host>` does not launch an emulator.
    // Resolve and reject up front with an actionable message rather than
    // building a binary the host can't execute.
    let resolved_target = ResolvedTarget::resolve(target)?;
    if !resolved_target.is_host() {
        return Err(format!(
            "cannot run a binary cross-compiled for '{}' on the host. \
             Build it with `ruxen build --target {}` and run it on the target \
             (or in a container — see docs/CROSS_COMPILE.md).",
            resolved_target.canonical(),
            resolved_target.canonical()
        ));
    }

    build(release, false, None, target)?;
    let profile = if release { "release" } else { "debug" };
    let target_root = find_target_root(&project_dir);
    let binary = target_root.join("target").join(profile).join(&package.name);

    if !binary.exists() {
        return Err(format!("binary not found at {}", binary.display()));
    }

    let status = std::process::Command::new(&binary)
        .args(&args)
        .status()
        .map_err(|e| format!("failed to run binary: {}", e))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

/// `ruxen check [--target <triple>]` — run the front end `ruxen build` runs
/// (type, borrow and loan checks, MIR lowering) without codegen.
pub fn check(target: Option<&str>) -> Result<(), String> {
    let start = Instant::now();
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;
    manifest.validate()?;
    let package = manifest.require_package()?;

    // The target picks the stdlib the program is checked against, as in
    // `ruxen build`; an invalid `--target` is a hard error.
    let target_str: Option<String> = target
        .map(|s| s.to_string())
        .or_else(|| manifest.build.as_ref().and_then(|b| b.target.clone()));
    let resolved_target = ResolvedTarget::resolve(target_str.as_deref())?;

    let entry = project_dir.join(manifest.entry_point());
    if !entry.exists() {
        return Err(format!("entry point not found: {}", entry.display()));
    }

    let dep_source_dirs = resolve_dep_source_dirs(&project_dir)?;
    let combined =
        gather_project_bundle(&project_dir, &entry, &dep_source_dirs, &package.name)?.source;

    front_end(
        &combined,
        &entry,
        &Options::for_target(&resolved_target, false),
    )?;

    let elapsed = start.elapsed();
    println!(
        "    Finished checking `{}` in {:.2}s",
        package.name,
        elapsed.as_secs_f64()
    );

    Ok(())
}

/// `ruxen clean` — remove the project's target root (the workspace
/// `target/` when run inside a member), incremental cache included.
pub fn clean() -> Result<(), String> {
    let project_dir = find_project_root()?;
    let target_dir = find_target_root(&project_dir).join("target");

    if target_dir.exists() {
        fs::remove_dir_all(&target_dir)
            .map_err(|e| format!("failed to remove target directory: {}", e))?;
        println!("  Removed {}", target_dir.display());
    }

    Ok(())
}

/// The nearest member when invoked inside a workspace. The workspace root
/// (which may also contain a Ruxen.toml) is found separately by
/// [`manifest::find_workspace_root`] and used only to compute the shared
/// `target/` directory; build/run still operate against the nearest
/// member's package.
pub use ruxenc::compile::find_project_root;

/// Return the directory that owns `target/`. When `project_dir` is a
/// member of a workspace, this is the workspace root; otherwise the
/// project's own directory.
pub fn find_target_root(project_dir: &Path) -> PathBuf {
    // The workspace search starts ONE level above project_dir — a
    // workspace root may itself contain `[workspace]` AND `[package]`
    // (Cargo's non-virtual root), but for a standalone non-workspace
    // package we must not match that package's own Ruxen.toml as a
    // workspace. The `is_workspace_root` check inside
    // `find_workspace_root` is what guards against that — but starting
    // at `project_dir.parent()` also handles the edge case where
    // `project_dir` itself is the workspace root: that's a self-build
    // and the target stays in `project_dir/target/` regardless.
    if let Some(parent) = project_dir.parent() {
        if let Some(ws) = crate::manifest::find_workspace_root(parent) {
            return ws;
        }
    }
    project_dir.to_path_buf()
}

/// Run the front end on `source`, the bundle whose entry file is `entry`,
/// and take its MIR program. Errors (and, on success, warnings) carry the
/// entry file's path, as `ruxen compile` prints them.
fn front_end(source: &str, entry: &Path, options: &Options) -> Result<MirProgram, String> {
    let path = entry.to_string_lossy();
    let (_, mir_program) = compile::front_end(&compile::parse(source, &path)?, &path, options)?;
    Ok(mir_program)
}

/// Gather all source files in a project into a single combined source string.
///
/// Reads the entry point first, then all module files. This allows the compiler
/// to see all definitions in a single compilation unit.
fn gather_sources(project_dir: &Path, tree: &ModuleTree, name: &str) -> Result<String, String> {
    let mut combined = String::new();

    // Read the entry point first (main.rx or lib.rx)
    let entry = if project_dir.join("src/lib.rx").exists() {
        project_dir.join("src/lib.rx")
    } else if project_dir.join("src/main.rx").exists() {
        project_dir.join("src/main.rx")
    } else {
        return Err(format!(
            "piece `{}` has no entry point (src/lib.rx or src/main.rx)",
            name
        ));
    };

    let entry_source = fs::read_to_string(&entry)
        .map_err(|e| format!("failed to read {}: {}", entry.display(), e))?;
    combined.push_str(&entry_source);
    combined.push('\n');

    // Append all module files (non-entry-point .rx files)
    for (module_name, file_path) in &tree.files {
        if module_name.is_empty() {
            continue; // skip entry point (already included)
        }
        let module_source = fs::read_to_string(file_path)
            .map_err(|e| format!("failed to read {}: {}", file_path.display(), e))?;
        combined.push_str(&module_source);
        combined.push('\n');
    }

    Ok(combined)
}

fn gather_project_bundle(
    project_dir: &Path,
    entry: &Path,
    dep_source_dirs: &[PathBuf],
    name: &str,
) -> Result<SourceBundle, String> {
    let mut load_paths = Vec::with_capacity(dep_source_dirs.len() + 1);
    load_paths.push(project_dir.to_path_buf());
    load_paths.extend(dep_source_dirs.iter().cloned());
    let bundle = ModuleLoader::new(load_paths).load(entry)?;
    if bundle.has_directives {
        return Ok(bundle);
    }

    let tree = ModuleTree::discover(project_dir)?;
    let mut source = gather_dep_sources(dep_source_dirs)?;
    source.push_str(&gather_sources(project_dir, &tree, name)?);
    Ok(SourceBundle {
        source,
        files: bundle.files,
        autoloads: bundle.autoloads,
        has_directives: false,
    })
}

/// Wrap the rlib object-extraction call with rich diagnostic context.
///
/// The underlying I/O / archive errors are technically accurate (`No
/// such file or directory`, `object code not found in .rlib`, …)
/// but give the user no path forward.  Surface the dep name and a
/// concrete fix hint based on the failure shape:
///
/// - Missing rlib → dependency was never built.  Most common cause:
///   user edited `[dependencies]` then ran the binary directly without
///   `ruxen build` in between.
/// - Any failure reading an rlib that exists → the archive is
///   unreadable, truncated or stale from a build that crashed
///   mid-write; rebuilding regenerates it.
fn extract_dep_object_code(name: &str, rlib_path: &Path) -> Result<Vec<u8>, String> {
    if !rlib_path.exists() {
        return Err(format!(
            "dependency `{}` is not built\n  \
             Expected rlib at: {}\n  \
             Hint: run `ruxen build` from this project; if the dep is \
             a path dependency, ensure its source dir compiles cleanly.",
            name,
            rlib_path.display()
        ));
    }
    rlib::extract_object_code(rlib_path).map_err(|e| {
        format!(
            "dependency `{}` rlib could not be read\n  \
             Path: {}\n  \
             Cause: {}\n  \
             Hint: a previous build may have been interrupted; \
             run `ruxen clean` then `ruxen build` to regenerate.",
            name,
            rlib_path.display(),
            e
        )
    })
}

/// Flat-merge every dependency package's `src/**.rx` into a single source
/// string, in the order given (callers pass topologically-sorted dep dirs).
///
/// This is the ONE mechanism by which a dependency's symbols enter a
/// consuming compilation unit in v1: the dep's source is prepended ahead
/// of the user source so the resolver sees its declarations during
/// typecheck. v1 symbols are flat (no `use <pkg>.X` namespacing). The
/// proper module-wrap (`module <pkg> ... end`) was attempted but exposes a
/// deeper resolver gap: classes nested inside a `module` block don't
/// propagate their field DefIds into method-body scope, so any dep with
/// `self.<field>` access in its methods fails to typecheck. Until that's
/// fixed, `use rondo.Foo` desugars to top-level `Foo`. Pin:
/// `docs/rondo_v1_blockers.md` B12.
///
/// Because the dep is compiled INTO the consuming unit (one object, one
/// definition of every symbol), there is no extern-rlib link and therefore
/// no duplicate-symbol/double-link risk — see
/// `docs/decisions/q16-dep-symbols-in-lib-check-test-builds.md`.
pub fn gather_dep_sources(dep_source_dirs: &[PathBuf]) -> Result<String, String> {
    let mut combined = String::new();
    for dep_dir in dep_source_dirs {
        let dep_tree = ModuleTree::discover(dep_dir)?;
        let dep_name = dep_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("dep");
        let dep_source = gather_sources(dep_dir, &dep_tree, dep_name)?;
        combined.push_str(&dep_source);
        combined.push('\n');
    }
    Ok(combined)
}

/// Resolve a project's dependency source directories (topologically
/// sorted, leaves first), so library / `check` / `test` builds can
/// flat-merge them exactly as the binary path does.
///
/// Returns an empty vec when the project declares no `[dependencies]`.
/// This is the workspace-aware resolution path shared with `build()`;
/// it does NOT write the lock file or verify checksums (those side
/// effects belong to a real build, not a `check`/`test` visibility pass).
pub fn resolve_dep_source_dirs(project_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let manifest = Manifest::load(project_dir)?;
    if manifest.dependencies.is_empty() {
        return Ok(Vec::new());
    }

    let existing_lock = LockFile::load(project_dir).ok();

    let mut workspace_members: std::collections::BTreeMap<String, PathBuf> = Default::default();
    if let Some(ws_root) = crate::manifest::find_workspace_root(project_dir) {
        let ws_manifest = Manifest::load(&ws_root)?;
        if let Some(ws) = ws_manifest.workspace.as_ref() {
            for (member_dir, member_name) in
                crate::manifest::expand_workspace_members(&ws_root, &ws.members)?
            {
                if member_dir != project_dir {
                    workspace_members.insert(member_name, member_dir);
                }
            }
        }
    }

    let result = resolve_deps::resolve_with_workspace(
        project_dir,
        &manifest,
        existing_lock.as_ref(),
        &workspace_members,
    )?;

    Ok(result.deps.iter().map(|d| d.source_dir.clone()).collect())
}

/// Given the full topologically-sorted dep list and a target dep, return
/// the source dirs of every package the target (transitively) depends on,
/// preserving topo order. Used to flat-merge a dependency's OWN deps when
/// building that dependency's rlib.
fn transitive_dep_source_dirs(
    target: &resolve_deps::ResolvedDep,
    all_deps: &[resolve_deps::ResolvedDep],
) -> Vec<PathBuf> {
    use std::collections::BTreeMap;
    let by_name: BTreeMap<&str, &resolve_deps::ResolvedDep> =
        all_deps.iter().map(|d| (d.name.as_str(), d)).collect();

    // Collect the transitive closure of names.
    let mut needed: std::collections::BTreeSet<String> = Default::default();
    let mut stack: Vec<String> = target.dependencies.clone();
    while let Some(n) = stack.pop() {
        if needed.insert(n.clone()) {
            if let Some(d) = by_name.get(n.as_str()) {
                stack.extend(d.dependencies.clone());
            }
        }
    }

    // Emit in the canonical topo order of `all_deps`.
    all_deps
        .iter()
        .filter(|d| needed.contains(&d.name))
        .map(|d| d.source_dir.clone())
        .collect()
}

/// Compile a dependency piece into an .rlib file, checked against
/// `target`'s stdlib.
///
/// `dep_source_dirs` carries the (topologically-sorted) source dirs of
/// every package this piece depends on, flat-merged ahead of its own
/// source so its `src/**.rx` can reference dependency symbols (Q16). For
/// a leaf dependency this is empty; for the consuming library it is the
/// project's full dependency closure.
fn compile_piece(
    source_dir: &Path,
    name: &str,
    version: &str,
    rlib_path: &Path,
    backend: Backend,
    _extern_libs: &[(String, PathBuf)],
    dep_source_dirs: &[PathBuf],
    target: &ResolvedTarget,
) -> Result<(), String> {
    let entry_file = if source_dir.join("src/lib.rx").is_file() {
        source_dir.join("src/lib.rx")
    } else {
        source_dir.join("src/main.rx")
    };
    let bundle = gather_project_bundle(source_dir, &entry_file, dep_source_dirs, name)?;
    let combined = bundle.source;
    let options = Options::for_target(target, false);
    let object_bytes =
        backend.emit_object(&front_end(&combined, &entry_file, &options)?, target)?;
    let metadata = TypeMetadata {
        compiler_version: rlib::COMPILER_VERSION.to_string(),
        name: name.to_string(),
        version: version.to_string(),
    };

    let source_hash = rlib::hash_bytes(combined.as_bytes());
    rlib::create_rlib(rlib_path, name, &object_bytes, &metadata, &source_hash)?;

    Ok(())
}

/// Compile the main project into an executable.
#[allow(clippy::too_many_arguments)]
fn compile_project(
    project_dir: &Path,
    manifest: &Manifest,
    output_path: &Path,
    backend: Backend,
    extern_libs: &[(String, PathBuf)],
    dep_source_dirs: &[PathBuf],
    target: &ResolvedTarget,
) -> Result<(), String> {
    let entry = project_dir.join(manifest.entry_point());
    if !entry.exists() {
        return Err(format!("entry point not found: {}", entry.display()));
    }

    let package_name = manifest.require_package()?.name.clone();
    let source = gather_project_bundle(project_dir, &entry, dep_source_dirs, &package_name)?.source;

    let mir_program = front_end(&source, &entry, &Options::for_target(target, false))?;

    // Collect link flags from extern libs.  Wrap each rlib extraction
    // with rich error context — the dep name and a concrete fix hint
    // are far more useful than the raw "No such file or directory"
    // that the underlying I/O layer surfaces.
    //
    // v0.1 flat-merge: when dep sources are already prepended to the
    // user source, the dep object would duplicate every symbol the
    // user object now also emits (mixin dispatch helpers, etc.).
    // Skip extern-lib linking in that case — the dep is inlined.
    let mut extra_link_flags: Vec<String> = Vec::new();
    let extern_libs: &[(String, PathBuf)] = if dep_source_dirs.is_empty() {
        extern_libs
    } else {
        &[]
    };
    for (name, rlib_path) in extern_libs {
        let obj_bytes = extract_dep_object_code(name, rlib_path)?;
        let obj_path = rlib_path.with_extension("o");
        fs::write(&obj_path, &obj_bytes).map_err(|e| {
            format!(
                "failed to write object file for dependency `{}`\n  Path: {}\n  Cause: {}",
                name,
                obj_path.display(),
                e
            )
        })?;
        extra_link_flags.push(obj_path.to_string_lossy().to_string());
    }

    // User-side link inputs, shared with `ruxen test`: the project's and
    // every path-dep's `runtime/*.c` (what stdlib packages get for free,
    // so a `lib "runtime/foo.c"` decl resolves), each flat-merged dep's
    // `[system_libs] libs` (Q32; `collect_system_lib_flags` only walks the
    // stdlib) and the project's and deps' `[system_libs] frameworks`.
    let link_inputs = codegen::LinkInputs::for_project(project_dir, dep_source_dirs)?;
    let mut user_runtime = link_inputs.runtime_sources;
    // `.m` (Objective-C / AppKit) shims are native-only — never compile them for
    // wasm (no AppKit; clang would fail). On the wasm reactor path their symbols
    // become host imports like other deps' runtime, so a quiver web app that
    // transitively depends on canvas's native shim still links cleanly.
    if target.is_wasm() {
        user_runtime.retain(|p| p.extension().and_then(|s| s.to_str()) != Some("m"));
    }

    for lib in &link_inputs.system_libs {
        extra_link_flags.push(format!("-l{}", lib));
    }

    // macOS frameworks (e.g. a native AppKit widget backend's `Cocoa`) go on
    // as `-framework <name>` so plain `ruxen build/run` links them. Native
    // macOS targets only, never wasm (wasm-ld rejects -framework), so a web
    // app transitively depending on canvas still links.
    #[cfg(target_os = "macos")]
    if !target.is_wasm() {
        for fw in link_inputs.frameworks {
            extra_link_flags.push("-framework".to_string());
            extra_link_flags.push(fw);
        }
    }

    let output_str = output_path.to_string_lossy().to_string();
    // Host → None (byte-identical path); cross → the resolved target.
    let target_opt = if target.is_host() { None } else { Some(target) };
    codegen::compile_with_options_for_target(
        &mir_program,
        &output_str,
        false,
        &extra_link_flags,
        &user_runtime,
        backend,
        target_opt,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_rlib_produces_actionable_error() {
        let tmp = std::env::temp_dir().join(format!("ruxen_rlib_missing_{}", std::process::id()));
        let bogus = tmp.join("not-built.rlib");
        let err = extract_dep_object_code("my-dep", &bogus).expect_err("should error");
        assert!(
            err.contains("my-dep") && err.contains("not built"),
            "expected dep name + 'not built' in error, got: {}",
            err
        );
        assert!(
            err.contains("Hint:") && err.contains("ruxen build"),
            "expected actionable hint with `ruxen build`, got: {}",
            err
        );
    }

    #[test]
    fn corrupted_rlib_produces_actionable_error() {
        // Write an empty file with .rlib extension — passes the exists()
        // check but `extract_object_code` fails to find the inner .o.
        let tmp = std::env::temp_dir().join(format!("ruxen_rlib_corrupt_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let rlib = tmp.join("broken.rlib");
        std::fs::write(&rlib, b"").expect("write empty file");
        let err = extract_dep_object_code("foo", &rlib).expect_err("should error");
        assert!(
            err.contains("foo"),
            "expected dep name in error; got: {}",
            err
        );
        assert!(
            err.contains("ruxen clean") || err.contains("ruxen build"),
            "expected recovery hint; got: {}",
            err
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
