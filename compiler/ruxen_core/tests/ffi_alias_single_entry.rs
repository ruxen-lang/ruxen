//! B1 pin test for
//! `docs/specs/system/compiler_consolidation.spec.md` §B1 — FFI alias
//! lookup has ONE entry point.
//!
//! Background: pre-consolidation, the method-call lowering in
//! `mir/lower/expr/method_call.rs` carried an inline
//! `self.ffi_alias_map.contains_key(...)` probe at the top of Branch 1
//! (the `.new` / static-ctor fast path) AND the fn-call lowering in
//! `mir/lower/expr/fn_call.rs` had its own direct `ffi_alias_map.get`
//! lookup. Future work to add a new "is this method an FFI alias?"
//! check would have had to touch both — exactly the trio-leak
//! symptom.
//!
//! §B1 introduced `Lowerer::lookup_ffi_alias` (the SINGLE ENTRY POINT
//! for alias LOOKUP) and routes both call sites through it. The
//! `resolve_ffi_alias_callee` wrapper preserves the historical
//! "miss → unchanged-mangled-name" caller surface for the dispatch
//! tail.
//!
//! This pin asserts the duplication can't return — no code outside
//! `mir/lower/mod.rs` may access `ffi_alias_map` directly.

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

/// `mir/lower/mod.rs` is the only file allowed to mention
/// `ffi_alias_map` outside of doc comments. Every other file must
/// route through `lookup_ffi_alias` or `resolve_ffi_alias_callee`.
#[test]
fn no_direct_ffi_alias_map_access_outside_lowerer() {
    let mut files = Vec::new();
    walk_rs_files(&src_dir(), &mut files);
    let lowerer_mod = src_dir().join("mir/lower/mod.rs");

    let mut offenders = Vec::new();
    for path in &files {
        if path == &lowerer_mod {
            // The struct field, initial population, and the two
            // helpers (`lookup_ffi_alias` / `resolve_ffi_alias_callee`)
            // legitimately mention the map here.
            continue;
        }
        let content = std::fs::read_to_string(path).expect("read");
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            // Allow comments / doc references — only flag CODE
            // references (the `self.ffi_alias_map.<method>` syntax).
            if trimmed.starts_with("///") || trimmed.starts_with("//") {
                continue;
            }
            // Pattern: anything that calls a method on the map.
            // Whitelist the field declaration line if it ever ends
            // up here, but it's only in mod.rs.
            if line.contains("ffi_alias_map.get(")
                || line.contains("ffi_alias_map.contains_key(")
                || line.contains("ffi_alias_map.insert(")
                || line.contains("ffi_alias_map.remove(")
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
        "FFI alias map access must flow through `lookup_ffi_alias` or \
         `resolve_ffi_alias_callee` (defined in \
         compiler/ruxen_core/src/mir/lower/mod.rs). Direct accesses \
         from other files defeat the single-entry-point guarantee:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn lookup_ffi_alias_is_defined_once() {
    let mut files = Vec::new();
    walk_rs_files(&src_dir(), &mut files);
    let mut count = 0;
    for path in &files {
        let content = std::fs::read_to_string(path).expect("read");
        count += content.matches("fn lookup_ffi_alias(").count();
    }
    assert_eq!(
        count, 1,
        "expected exactly one `fn lookup_ffi_alias` definition; found {}",
        count
    );
}
