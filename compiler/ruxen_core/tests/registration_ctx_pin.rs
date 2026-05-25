//! B3 pin test for
//! `docs/specs/system/compiler_consolidation.spec.md` — Pass 1 type
//! registration has ONE entry point with caller-identity threaded as
//! an EXPLICIT `RegistrationCtx` argument (not side-channel resolver
//! fields).
//!
//! Background: pre-consolidation, `Resolver` carried two fields —
//! `merging_bootstrap: bool` and `defer_class_lib_decls: bool` —
//! flipped before/after each bootstrap-merge phase. The Class arm of
//! `register_top_level_type_with_ffi_in` then branched on
//! `self.merging_bootstrap` to decide whether to enter namespace-
//! anchor mode (reuse the existing DefId for builtin type names) or
//! perform a fresh registration. This was caller-identity inferred
//! from hidden state.
//!
//! Spec §B3 stop condition says the body MAY branch on caller-
//! specific state, but the asymmetry has to be a CONSTRUCTOR ARGUMENT,
//! not an internal `if self.merging_bootstrap`. Commit (this one)
//! introduced `RegistrationCtx` and removed the fields.
//!
//! These pins assert the duplication can't return.

use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn src_dir() -> PathBuf {
    workspace_root().join("compiler/ruxen_core/src")
}

fn walk_rs_files(dir: &PathBuf, files: &mut Vec<PathBuf>) {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs_files(&path, files);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn no_self_merging_bootstrap_branches_remain() {
    // The hidden-state field `Resolver::merging_bootstrap` is gone.
    // Any `self.merging_bootstrap` reference in compiler src is a
    // regression. Doc comments may still reference the historical
    // pattern; code branches may not.
    let mut files = Vec::new();
    walk_rs_files(&src_dir(), &mut files);
    let mut offenders = Vec::new();
    for path in &files {
        let content = std::fs::read_to_string(path).expect("read");
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("///") || trimmed.starts_with("//") {
                continue;
            }
            if line.contains("self.merging_bootstrap")
                || line.contains("self.defer_class_lib_decls")
            {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(workspace_root()).unwrap().display(),
                    lineno + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Pass 1 registration must branch on the explicit \
         `RegistrationCtx` argument, not side-channel resolver \
         fields. Offenders:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn registration_ctx_constructors_exist() {
    // Surface check: both intent-named constructors are defined.
    // Callers MUST go through one of them — the raw struct literal
    // path is still possible in Rust, but the named constructors are
    // the encouraged surface and grepping for them tells us whether
    // a fresh call site forgot to declare its intent.
    let ffi_rs = src_dir().join("resolve/ffi_registration.rs");
    let content = std::fs::read_to_string(&ffi_rs).expect("read ffi_registration.rs");
    assert!(
        content.contains("fn user_program() -> Self"),
        "expected `RegistrationCtx::user_program()` constructor in \
         resolve/ffi_registration.rs"
    );
    assert!(
        content.contains("fn bootstrap_first_walk() -> Self"),
        "expected `RegistrationCtx::bootstrap_first_walk()` \
         constructor in resolve/ffi_registration.rs"
    );
}

#[test]
fn register_top_level_type_with_ffi_in_is_defined_once() {
    // The internal `_in` variant is the single entry point. The
    // public wrapper just defaults to an empty module path. There
    // must be exactly one definition of each.
    let mut files = Vec::new();
    walk_rs_files(&src_dir(), &mut files);
    let mut wrapper_defs = 0;
    let mut inner_defs = 0;
    for path in &files {
        let content = std::fs::read_to_string(path).expect("read");
        wrapper_defs += content
            .matches("fn register_top_level_type_with_ffi(")
            .count();
        inner_defs += content
            .matches("fn register_top_level_type_with_ffi_in(")
            .count();
    }
    assert_eq!(
        wrapper_defs, 1,
        "expected exactly one `fn register_top_level_type_with_ffi` definition; \
         found {}",
        wrapper_defs
    );
    assert_eq!(
        inner_defs, 1,
        "expected exactly one `fn register_top_level_type_with_ffi_in` definition; \
         found {}",
        inner_defs
    );
}
