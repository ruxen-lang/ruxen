//! Build pipeline: `riven build`, `riven run`, `riven check`, `riven clean`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use riven_core::borrow_check;
use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir;
use riven_core::parser::Parser;
use riven_core::typeck;

use crate::lock::LockFile;
use crate::manifest::Manifest;
use crate::module_discovery::ModuleTree;
use crate::resolve_deps;
use crate::rlib::{self, Exports, TypeMetadata};

/// `riven build [--release] [--locked] [--bin <name>]`
pub fn build(release: bool, locked: bool, bin_name: Option<&str>) -> Result<(), String> {
    let start = Instant::now();
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;
    manifest.validate()?;

    // Advisory: dev-dependencies are parsed + survive `riven add --dev`
    // / `riven remove`, but `riven build` does not yet compile them
    // (no test runner consumer exists in v1).  Tell the user up front
    // so a misspelled dev-dep doesn't fail silently after a
    // successful build.  v1.next plan: a `riven test` subcommand that
    // builds dev-deps alongside the project.
    if !manifest.dev_dependencies.is_empty() {
        let names: Vec<&str> = manifest
            .dev_dependencies
            .keys()
            .map(|s| s.as_str())
            .collect();
        eprintln!(
            "  warning: [dev-dependencies] ({}) are recorded in Riven.toml but not \
             compiled by `riven build` — `riven test` (v1.next) will consume them.",
            names.join(", ")
        );
    }

    let profile = if release { "release" } else { "debug" };
    let target_dir = project_dir.join("target").join(profile);
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
                    return Err("Riven.lock is out of date with Riven.toml.\n  \
                         Run `riven update` to regenerate the lock file."
                        .to_string());
                }
                None => {
                    return Err("Riven.lock not found but --locked was specified.\n  \
                         Run `riven build` first to generate the lock file."
                        .to_string());
                }
            }
        }

        let result = resolve_deps::resolve(&project_dir, &manifest, existing_lock.as_ref())?;

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
            compile_piece(
                &dep.source_dir,
                &dep.name,
                &dep.version,
                &rlib_path,
                release,
                &extern_libs,
            )?;
            extern_libs.push((dep.name.clone(), rlib_path));
        }
    }

    // Step 3: Compile project source
    println!(
        "  Compiling piece `{}` v{}",
        manifest.package.name, manifest.package.version
    );

    if manifest.build_type() == "library" {
        // Library: produce an .rlib
        let rlib_path = target_dir.join(format!("{}.rlib", manifest.package.name));
        compile_piece(
            &project_dir,
            &manifest.package.name,
            &manifest.package.version,
            &rlib_path,
            release,
            &extern_libs,
        )?;
    } else {
        // Binary: produce an executable
        let output_name = bin_name.unwrap_or(&manifest.package.name);
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

/// `riven run [--release] [-- <args>]`
pub fn run(release: bool, args: Vec<String>) -> Result<(), String> {
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;

    if manifest.build_type() == "library" {
        return Err("cannot run a library project. Use `riven build` instead.".to_string());
    }

    build(release, false, None)?;
    let profile = if release { "release" } else { "debug" };
    let binary = project_dir
        .join("target")
        .join(profile)
        .join(&manifest.package.name);

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

/// `riven check` — type-check without codegen.
pub fn check() -> Result<(), String> {
    let start = Instant::now();
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;
    manifest.validate()?;

    let entry = project_dir.join(manifest.entry_point());
    if !entry.exists() {
        return Err(format!("entry point not found: {}", entry.display()));
    }

    // Discover and gather all module sources
    let tree = ModuleTree::discover(&project_dir)?;
    let combined = gather_sources(&project_dir, &tree, &manifest.package.name)?;

    if let Err(e) = check_single_file(&combined, &entry) {
        eprintln!("{}", e);
        return Err("type checking failed".to_string());
    }

    let elapsed = start.elapsed();
    println!(
        "    Finished checking `{}` in {:.2}s",
        manifest.package.name,
        elapsed.as_secs_f64()
    );

    Ok(())
}

/// `riven clean` — remove the target/ directory.
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

/// Find the project root by searching upward for Riven.toml.
pub fn find_project_root() -> Result<PathBuf, String> {
    let mut dir =
        std::env::current_dir().map_err(|e| format!("failed to get current directory: {}", e))?;

    loop {
        if dir.join("Riven.toml").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err(
                "could not find `Riven.toml` in this directory or any parent directory".to_string(),
            );
        }
    }
}

/// Compile a single .rvn file through the full pipeline: lex → parse → typecheck → borrow check → MIR → codegen.
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
        .any(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error);

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
        .any(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error);

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

    // Read the entry point first (main.rvn or lib.rvn)
    let entry = if project_dir.join("src/lib.rvn").exists() {
        project_dir.join("src/lib.rvn")
    } else if project_dir.join("src/main.rvn").exists() {
        project_dir.join("src/main.rvn")
    } else {
        return Err(format!(
            "piece `{}` has no entry point (src/lib.rvn or src/main.rvn)",
            name
        ));
    };

    let entry_source = fs::read_to_string(&entry)
        .map_err(|e| format!("failed to read {}: {}", entry.display(), e))?;
    combined.push_str(&entry_source);
    combined.push('\n');

    // Append all module files (non-entry-point .rvn files)
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
///   `riven build` in between.
/// - Compiler-version mismatch → dependency was built with a different
///   rivenc.  Common after upgrading the toolchain.
/// - Corrupted archive / missing metadata → rlib is truncated or
///   stale from a build that crashed mid-write.
fn extract_dep_object_code(name: &str, rlib_path: &Path) -> Result<Vec<u8>, String> {
    if !rlib_path.exists() {
        return Err(format!(
            "dependency `{}` is not built\n  \
             Expected rlib at: {}\n  \
             Hint: run `riven build` from this project; if the dep is \
             a path dependency, ensure its source dir compiles cleanly.",
            name,
            rlib_path.display()
        ));
    }
    rlib::extract_object_code(rlib_path).map_err(|e| {
        let lower = e.to_lowercase();
        // Pattern-match the underlying error to give an actionable hint.
        if lower.contains("incompatible") || lower.contains("compiled with rivenc") {
            format!(
                "dependency `{}` was built with an incompatible compiler version\n  \
                 Path: {}\n  \
                 Cause: {}\n  \
                 Hint: run `riven clean` then `riven build` to rebuild the dep \
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
                 run `riven clean` then `riven build` to regenerate.",
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

/// Compile a dependency piece into an .rlib file.
fn compile_piece(
    source_dir: &Path,
    name: &str,
    version: &str,
    rlib_path: &Path,
    release: bool,
    _extern_libs: &[(String, PathBuf)],
) -> Result<(), String> {
    let tree = ModuleTree::discover(source_dir)?;

    // Gather all source files: entry point first, then modules
    let combined = gather_sources(source_dir, &tree, name)?;

    let entry_file = source_dir.join("src/lib.rvn");
    let (object_bytes, mut metadata) = compile_single_file(&combined, &entry_file, release)?;

    // Set the piece metadata
    metadata.name = name.to_string();
    metadata.version = version.to_string();

    let source_hash = rlib::hash_sources(source_dir)?;
    rlib::create_rlib(rlib_path, name, &object_bytes, &metadata, &source_hash)?;

    Ok(())
}

/// Compile the main project into an executable.
fn compile_project(
    project_dir: &Path,
    manifest: &Manifest,
    output_path: &Path,
    release: bool,
    extern_libs: &[(String, PathBuf)],
    dep_source_dirs: &[PathBuf],
) -> Result<(), String> {
    let entry = project_dir.join(manifest.entry_point());
    if !entry.exists() {
        return Err(format!("entry point not found: {}", entry.display()));
    }

    let tree = ModuleTree::discover(project_dir)?;
    let user_source = gather_sources(project_dir, &tree, &manifest.package.name)?;

    // Flat-merge dep sources ahead of user source so the resolver
    // sees their declarations during typecheck. v1: symbols are flat
    // (no `use <pkg>.X` namespacing). The proper module-wrap
    // (`module <pkg> ... end`) was attempted but exposes a deeper
    // resolver gap: classes nested inside a `module` block don't
    // propagate their field DefIds into method-body scope, so any
    // dep with `self.<field>` access in its methods fails to
    // typecheck. Until that's fixed, `use rondo.Foo` desugars to
    // top-level `Foo`. Pin: `docs/rondo_v1_blockers.md` B12.
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

    // Phase 3: Type check
    let type_result = typeck::type_check(&program);
    let has_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error);

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

    let output_str = output_path.to_string_lossy().to_string();
    codegen::compile_with_options(&mir_program, &output_str, false, &extra_link_flags, backend)?;

    Ok(())
}

/// Build type metadata from the HIR type checking result.
fn build_metadata_from_hir(type_result: &typeck::TypeCheckResult) -> TypeMetadata {
    use riven_core::hir::nodes::{HirItem, HirMixinItem};

    let mut exports = Exports::default();

    for item in &type_result.program.items {
        match item {
            HirItem::Function(func) => {
                if func.visibility == riven_core::parser::ast::Visibility::Public {
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
                        .filter(|m| m.visibility == riven_core::parser::ast::Visibility::Public)
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
        let tmp = std::env::temp_dir().join(format!("riven_rlib_missing_{}", std::process::id()));
        let bogus = tmp.join("not-built.rlib");
        let err = extract_dep_object_code("my-dep", &bogus).expect_err("should error");
        assert!(
            err.contains("my-dep") && err.contains("not built"),
            "expected dep name + 'not built' in error, got: {}",
            err
        );
        assert!(
            err.contains("Hint:") && err.contains("riven build"),
            "expected actionable hint with `riven build`, got: {}",
            err
        );
    }

    #[test]
    fn corrupted_rlib_produces_actionable_error() {
        // Write an empty file with .rlib extension — passes the exists()
        // check but `extract_object_code` fails to find the inner .o.
        let tmp = std::env::temp_dir().join(format!("riven_rlib_corrupt_{}", std::process::id()));
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
            err.contains("riven clean") || err.contains("riven build"),
            "expected recovery hint; got: {}",
            err
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
