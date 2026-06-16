//! Build pipeline: `ruxen build`, `ruxen run`, `ruxen check`, `ruxen clean`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ruxen_core::borrow_check;
use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

use crate::lock::LockFile;
use crate::manifest::Manifest;
use crate::module_discovery::ModuleTree;
use crate::resolve_deps;
use crate::rlib::{self, Exports, TypeMetadata};

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
    let resolved_target =
        ruxen_core::codegen::target::ResolvedTarget::resolve(target_str.as_deref())?;

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
                release,
                &extern_libs,
                &dep_deps,
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
            release,
            &extern_libs,
            &dep_source_dirs,
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
            release,
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
    let resolved_target = ruxen_core::codegen::target::ResolvedTarget::resolve(target)?;
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

/// `ruxen check [--target <triple>]` — type-check without codegen.
pub fn check(target: Option<&str>) -> Result<(), String> {
    let start = Instant::now();
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;
    manifest.validate()?;
    let package = manifest.require_package()?;

    // Validate the triple up front (an invalid `--target` is a hard error even
    // in `check`). Codegen is not run; the resolved target is reserved for
    // cfg(...) gating (tier 4.01).
    let target_str: Option<String> = target
        .map(|s| s.to_string())
        .or_else(|| manifest.build.as_ref().and_then(|b| b.target.clone()));
    let _resolved_target =
        ruxen_core::codegen::target::ResolvedTarget::resolve(target_str.as_deref())?;

    let entry = project_dir.join(manifest.entry_point());
    if !entry.exists() {
        return Err(format!("entry point not found: {}", entry.display()));
    }

    // Discover and gather all module sources. Flat-merge the project's
    // dependency sources ahead of its own (Q16) so `ruxen check` sees
    // dependency symbols — the same visibility a binary build gets.
    let tree = ModuleTree::discover(&project_dir)?;
    let dep_source_dirs = resolve_dep_source_dirs(&project_dir)?;
    let mut combined = gather_dep_sources(&dep_source_dirs)?;
    combined.push_str(&gather_sources(&project_dir, &tree, &package.name)?);

    if let Err(e) = check_single_file(&combined, &entry) {
        eprintln!("{}", e);
        return Err("type checking failed".to_string());
    }

    let elapsed = start.elapsed();
    println!(
        "    Finished checking `{}` in {:.2}s",
        package.name,
        elapsed.as_secs_f64()
    );

    Ok(())
}

/// `ruxen clean` — remove the target/ directory.
pub fn clean() -> Result<(), String> {
    let project_dir = find_project_root()?;
    let target_dir = project_dir.join("target");

    if target_dir.exists() {
        fs::remove_dir_all(&target_dir)
            .map_err(|e| format!("failed to remove target directory: {}", e))?;
        println!("  Removed {}", target_dir.display());
    }

    Ok(())
}

/// Find the project root by searching upward for Ruxen.toml.
///
/// Returns the FIRST ancestor with a Ruxen.toml — i.e. the nearest
/// member when invoked from inside a workspace. The workspace root
/// (which may also contain a Ruxen.toml) is found separately by
/// [`manifest::find_workspace_root`] and used only to compute the
/// shared `target/` directory; build/run still operate against the
/// nearest member's package.
pub fn find_project_root() -> Result<PathBuf, String> {
    let mut dir =
        std::env::current_dir().map_err(|e| format!("failed to get current directory: {}", e))?;

    loop {
        if dir.join("Ruxen.toml").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err(
                "could not find `Ruxen.toml` in this directory or any parent directory".to_string(),
            );
        }
    }
}

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

/// Compile a single .rx file through the full pipeline: lex → parse → typecheck → borrow check → MIR → codegen.
fn compile_single_file(
    source: &str,
    _file_path: &Path,
    release: bool,
) -> Result<(Vec<u8>, TypeMetadata), String> {
    // Phase 1: Lexing
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    // Phase 2: Parsing
    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    // Phase 3: Type checking
    let type_result = typeck::type_check(&program);
    let has_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error);

    if has_errors {
        let msgs: Vec<String> = type_result
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect();
        return Err(msgs.join("\n"));
    }

    // Phase 4: Borrow checking
    let borrow_errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    if !borrow_errors.is_empty() {
        let msgs: Vec<String> = borrow_errors.iter().map(|e| e.to_string()).collect();
        return Err(msgs.join("\n"));
    }

    // Phase 5: MIR lowering
    let mut lowerer = mir::lower::Lowerer::new(&type_result.symbols);
    let mir_program = lowerer
        .lower_program(&type_result.program)
        .map_err(|e| format!("MIR lowering error: {}", e))?;

    // Phase 6: Code generation
    let backend = if release {
        #[cfg(feature = "llvm")]
        {
            codegen::Backend::Llvm { opt_level: 2 }
        }
        #[cfg(not(feature = "llvm"))]
        {
            codegen::Backend::Cranelift
        }
    } else {
        codegen::Backend::Cranelift
    };

    let object_bytes = match backend {
        codegen::Backend::Cranelift => {
            let mut cg = codegen::cranelift::CodeGen::new()?;
            cg.compile_program(&mir_program)?;
            cg.finish()?
        }
        #[cfg(feature = "llvm")]
        codegen::Backend::Llvm { opt_level } => {
            let mut cg = codegen::llvm::CodeGen::new(opt_level)?;
            cg.compile_program(&mir_program)?;
            cg.finish()?
        }
    };

    // Build type metadata from HIR
    let metadata = build_metadata_from_hir(&type_result);

    Ok((object_bytes, metadata))
}

/// Type-check a single file (no codegen).
fn check_single_file(source: &str, file_path: &Path) -> Result<(), String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| format!("{}: {}", file_path.display(), d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| format!("{}: {}", file_path.display(), d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    let type_result = typeck::type_check(&program);
    let has_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error);

    if has_errors {
        let msgs: Vec<String> = type_result
            .diagnostics
            .iter()
            .map(|d| format!("{}: {}", file_path.display(), d))
            .collect();
        return Err(msgs.join("\n"));
    }

    Ok(())
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

/// Wrap the rlib object-extraction call with rich diagnostic context.
///
/// The underlying I/O / archive errors are technically accurate (`No
/// such file or directory`, `metadata.json not found in .rlib`, …)
/// but give the user no path forward.  Surface the dep name and a
/// concrete fix hint based on the failure shape:
///
/// - Missing rlib → dependency was never built.  Most common cause:
///   user edited `[dependencies]` then ran the binary directly without
///   `ruxen build` in between.
/// - Compiler-version mismatch → dependency was built with a different
///   ruxenc.  Common after upgrading the toolchain.
/// - Corrupted archive / missing metadata → rlib is truncated or
///   stale from a build that crashed mid-write.
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
        let lower = e.to_lowercase();
        // Pattern-match the underlying error to give an actionable hint.
        if lower.contains("incompatible") || lower.contains("compiled with ruxenc") {
            format!(
                "dependency `{}` was built with an incompatible compiler version\n  \
                 Path: {}\n  \
                 Cause: {}\n  \
                 Hint: run `ruxen clean` then `ruxen build` to rebuild the dep \
                 with the current toolchain.",
                name,
                rlib_path.display(),
                e
            )
        } else if lower.contains("not found in .rlib") || lower.contains("failed to read") {
            format!(
                "dependency `{}` rlib is corrupted or incomplete\n  \
                 Path: {}\n  \
                 Cause: {}\n  \
                 Hint: a previous build may have been interrupted; \
                 run `ruxen clean` then `ruxen build` to regenerate.",
                name,
                rlib_path.display(),
                e
            )
        } else {
            format!(
                "failed to load dependency `{}`\n  Path: {}\n  Cause: {}",
                name,
                rlib_path.display(),
                e
            )
        }
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

/// Compile a dependency piece into an .rlib file.
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
    release: bool,
    _extern_libs: &[(String, PathBuf)],
    dep_source_dirs: &[PathBuf],
) -> Result<(), String> {
    let tree = ModuleTree::discover(source_dir)?;

    // Gather all source files: dep sources first (Q16), then entry point
    // + modules. Flat-merge (not extern-rlib link) keeps a single object
    // per rlib — no duplicate symbols.
    let mut combined = gather_dep_sources(dep_source_dirs)?;
    combined.push_str(&gather_sources(source_dir, &tree, name)?);

    let entry_file = source_dir.join("src/lib.rx");
    let (object_bytes, mut metadata) = compile_single_file(&combined, &entry_file, release)?;

    // Set the piece metadata
    metadata.name = name.to_string();
    metadata.version = version.to_string();

    let source_hash = rlib::hash_sources(source_dir)?;
    rlib::create_rlib(rlib_path, name, &object_bytes, &metadata, &source_hash)?;

    Ok(())
}

/// Compile the main project into an executable.
#[allow(clippy::too_many_arguments)]
fn compile_project(
    project_dir: &Path,
    manifest: &Manifest,
    output_path: &Path,
    release: bool,
    extern_libs: &[(String, PathBuf)],
    dep_source_dirs: &[PathBuf],
    target: &ruxen_core::codegen::target::ResolvedTarget,
) -> Result<(), String> {
    let entry = project_dir.join(manifest.entry_point());
    if !entry.exists() {
        return Err(format!("entry point not found: {}", entry.display()));
    }

    let tree = ModuleTree::discover(project_dir)?;
    let package_name = manifest.require_package()?.name.clone();
    let user_source = gather_sources(project_dir, &tree, &package_name)?;

    // Flat-merge dep sources ahead of user source so the resolver
    // sees their declarations during typecheck (see `gather_dep_sources`).
    let mut combined = gather_dep_sources(dep_source_dirs)?;
    combined.push_str(&user_source);
    let source = combined;

    // Phase 1: Lex
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    // Phase 2: Parse
    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    // Phase 3: Type check. On the wasm target use the curated stdlib bootstrap
    // (heap-core subset) — the full bootstrap drags in `dispatch runtime` stdlib
    // (TimeSleepFuture) the LLVM/wasm backend can't lower (tier 4.09). Mirrors
    // the single-file `ruxen compile` wasm path.
    let type_result = if target.is_wasm() {
        typeck::type_check_wasm(&program)
    } else {
        typeck::type_check(&program)
    };
    let has_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error);

    if has_errors {
        let msgs: Vec<String> = type_result
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect();
        return Err(msgs.join("\n"));
    }

    // Phase 4: Borrow check
    let borrow_errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    if !borrow_errors.is_empty() {
        let msgs: Vec<String> = borrow_errors.iter().map(|e| e.to_string()).collect();
        return Err(msgs.join("\n"));
    }

    // Phase 5: MIR lowering
    let mut lowerer = mir::lower::Lowerer::new(&type_result.symbols);
    let mir_program = lowerer
        .lower_program(&type_result.program)
        .map_err(|e| format!("MIR lowering error: {}", e))?;

    // Phase 6: Code generation → link → executable
    // §5.8: a target Cranelift can't emit (wasm/embedded) forces LLVM even in
    // debug. Otherwise: LLVM for --release (when built), Cranelift for debug.
    let force_llvm = target.requires_llvm_backend();
    let backend = if release || force_llvm {
        #[cfg(feature = "llvm")]
        {
            codegen::Backend::Llvm { opt_level: 2 }
        }
        #[cfg(not(feature = "llvm"))]
        {
            if force_llvm {
                return Err(format!(
                    "target '{}' requires the LLVM backend, which this build of \
                     ruxen was not compiled with (rebuild with --features llvm).",
                    target.canonical()
                ));
            }
            codegen::Backend::Cranelift
        }
    } else {
        codegen::Backend::Cranelift
    };

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

    // Gather user-side C runtime sources: the project's own
    // `runtime/*.c` plus every path-dep's `runtime/*.c`. Mirrors what
    // stdlib packages get for free (`library/std/<pkg>/runtime/*.c`)
    // so a `lib "runtime/foo.c"` decl in user code resolves to symbols
    // defined in a sibling-of-`src/` C file.
    let mut user_runtime: Vec<PathBuf> = Vec::new();
    user_runtime.extend(codegen::find_runtime_sources_in_dir(project_dir)?);
    for dep_dir in dep_source_dirs {
        user_runtime.extend(codegen::find_runtime_sources_in_dir(dep_dir)?);
    }
    // `.m` (Objective-C / AppKit) shims are native-only — never compile them for
    // wasm (no AppKit; clang would fail). On the wasm reactor path their symbols
    // become host imports like other deps' runtime, so a quiver web app that
    // transitively depends on canvas's native shim still links cleanly.
    if target.is_wasm() {
        user_runtime.retain(|p| p.extension().and_then(|s| s.to_str()) != Some("m"));
    }

    // Q32: a flat-merged FFI dependency's `[system_libs]` (e.g. `-lpthread`)
    // must also reach this binary's link line, the same way its `runtime/*.c`
    // does above. `collect_system_lib_flags` only walks the STDLIB root, so a
    // user dep's link needs would otherwise be dropped. Mirrors the test
    // runner's dep `[system_libs]` forwarding.
    for dep_dir in dep_source_dirs {
        let dep_toml = dep_dir.join("Ruxen.toml");
        if let Ok(contents) = fs::read_to_string(&dep_toml) {
            for lib in codegen::parse_system_libs(&contents) {
                let flag = format!("-l{}", lib);
                if !extra_link_flags.contains(&flag) {
                    extra_link_flags.push(flag);
                }
            }
        }
    }

    // macOS frameworks: this binary OR any dep may declare `[system_libs]
    // frameworks = ["Cocoa", ...]` — e.g. a native AppKit widget backend. Forward
    // each as `-framework <name>` so plain `ruxen build/run` links them (no env
    // var). Native macOS targets only — never for wasm (wasm-ld rejects
    // -framework) so a web app transitively depending on canvas still links.
    #[cfg(target_os = "macos")]
    if !target.is_wasm() {
        let mut fw_tomls: Vec<PathBuf> = vec![project_dir.join("Ruxen.toml")];
        for dep_dir in dep_source_dirs {
            fw_tomls.push(dep_dir.join("Ruxen.toml"));
        }
        for toml_path in &fw_tomls {
            if let Ok(contents) = fs::read_to_string(toml_path) {
                for fw in codegen::parse_system_frameworks(&contents) {
                    let dup = extra_link_flags
                        .windows(2)
                        .any(|w| w[0] == "-framework" && w[1] == fw);
                    if !dup {
                        extra_link_flags.push("-framework".to_string());
                        extra_link_flags.push(fw);
                    }
                }
            }
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

/// Build type metadata from the HIR type checking result.
fn build_metadata_from_hir(type_result: &typeck::TypeCheckResult) -> TypeMetadata {
    use ruxen_core::hir::nodes::{HirItem, HirMixinItem};

    let mut exports = Exports::default();

    for item in &type_result.program.items {
        match item {
            HirItem::Function(func) => {
                if func.visibility == ruxen_core::parser::ast::Visibility::Public {
                    exports.functions.push(rlib::ExportedFunction {
                        name: func.name.clone(),
                        params: func
                            .params
                            .iter()
                            .map(|p| rlib::ExportedParam {
                                name: p.name.clone(),
                                ty: format!("{:?}", p.ty),
                            })
                            .collect(),
                        return_type: format!("{:?}", func.return_ty),
                        visibility: "public".to_string(),
                    });
                }
            }
            HirItem::Class(class) => {
                exports.types.push(rlib::ExportedType {
                    name: class.name.clone(),
                    kind: "class".to_string(),
                    fields: class
                        .fields
                        .iter()
                        .map(|f| rlib::ExportedField {
                            name: f.name.clone(),
                            ty: format!("{:?}", f.ty),
                            visibility: format!("{:?}", f.visibility),
                        })
                        .collect(),
                    methods: class
                        .methods
                        .iter()
                        .filter(|m| m.visibility == ruxen_core::parser::ast::Visibility::Public)
                        .map(|m| rlib::ExportedFunction {
                            name: m.name.clone(),
                            params: m
                                .params
                                .iter()
                                .map(|p| rlib::ExportedParam {
                                    name: p.name.clone(),
                                    ty: format!("{:?}", p.ty),
                                })
                                .collect(),
                            return_type: format!("{:?}", m.return_ty),
                            visibility: "public".to_string(),
                        })
                        .collect(),
                });
            }
            HirItem::Struct(s) => {
                exports.types.push(rlib::ExportedType {
                    name: s.name.clone(),
                    kind: "struct".to_string(),
                    fields: s
                        .fields
                        .iter()
                        .map(|f| rlib::ExportedField {
                            name: f.name.clone(),
                            ty: format!("{:?}", f.ty),
                            visibility: format!("{:?}", f.visibility),
                        })
                        .collect(),
                    methods: vec![],
                });
            }
            HirItem::Mixin(t) => {
                let methods = t
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        HirMixinItem::MethodSig {
                            name,
                            params,
                            return_ty,
                            ..
                        } => Some(rlib::ExportedFunction {
                            name: name.clone(),
                            params: params
                                .iter()
                                .map(|p| rlib::ExportedParam {
                                    name: p.name.clone(),
                                    ty: format!("{:?}", p.ty),
                                })
                                .collect(),
                            return_type: format!("{:?}", return_ty),
                            visibility: "public".to_string(),
                        }),
                        HirMixinItem::DefaultMethod(func) => Some(rlib::ExportedFunction {
                            name: func.name.clone(),
                            params: func
                                .params
                                .iter()
                                .map(|p| rlib::ExportedParam {
                                    name: p.name.clone(),
                                    ty: format!("{:?}", p.ty),
                                })
                                .collect(),
                            return_type: format!("{:?}", func.return_ty),
                            visibility: "public".to_string(),
                        }),
                        _ => None,
                    })
                    .collect();

                exports.traits.push(rlib::ExportedTrait {
                    name: t.name.clone(),
                    methods,
                });
            }
            _ => {}
        }
    }

    TypeMetadata {
        compiler_version: rlib::COMPILER_VERSION.to_string(),
        name: String::new(),
        version: String::new(),
        exports,
    }
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
