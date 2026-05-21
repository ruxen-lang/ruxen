//! Wave-1 LSP `UseIndex` — pin tests covering the contract laid out
//! in `docs/requirements/tier3_01_lsp.md` §5.2.
//!
//! Per `feedback_no_inline_rvn_in_pin_tests.md`, every Riven source
//! lives in a `.rvn` fixture under `tests/fixtures/use_index/`. Each
//! fixture uses a unique anchor identifier (e.g. `var_anchor_qzx`)
//! that appears exactly the expected number of times in the file and
//! is NEVER mentioned in a comment — so we can find a def by name
//! and count `uses` deterministically.

use riven_core::hir::nodes::DefId;
use riven_ide::analysis::{analyze, AnalysisResult};
use riven_ide::use_index::UseIndex;

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/use_index")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Run `analyze` and return the populated index, panicking with a
/// clear message if it's absent — the analysis must succeed for these
/// fixtures.
fn index_of(source: &str) -> (AnalysisResult, UseIndex) {
    let result = analyze(source);
    let idx = result
        .use_index
        .as_ref()
        .unwrap_or_else(|| panic!("use_index missing — analysis stopped early"))
        .clone();
    (result, idx)
}

/// Find the active def by name. The bootstrap-merge in the resolver
/// (`project_riven_bootstrap_merge_skips_class_bodies.md`) sometimes
/// registers the same top-level name twice — once during HIR build
/// and once again during the merge pass. Use-sites bind to whichever
/// of the duplicates the resolver picked, so the "active" def is the
/// one with the most recorded uses. Ties (e.g. an unreferenced class)
/// resolve to the first match — that's fine because both candidates
/// share the same span.
fn def_by_name(result: &AnalysisResult, name: &str) -> DefId {
    let idx = result.use_index.as_ref().expect("use_index");
    let symbols = result.symbols.as_ref().expect("symbols");
    let mut best: Option<(DefId, usize)> = None;
    for d in symbols.iter() {
        if d.name != name {
            continue;
        }
        let count = idx.count(d.id);
        match best {
            None => best = Some((d.id, count)),
            Some((_, prev)) if count > prev => best = Some((d.id, count)),
            _ => {}
        }
    }
    best.unwrap_or_else(|| panic!("no def named `{}` found", name))
        .0
}

// ─── Tests ──────────────────────────────────────────────────────────



#[test]
fn local_variable_collects_decl_plus_three_uses() {
    let src = load("local_var_three_refs");
    let (result, idx) = index_of(&src);
    let def = def_by_name(&result, "var_anchor_qzx");
    // 1 decl + 3 uses (`= var_anchor_qzx + var_anchor_qzx` then `= var_anchor_qzx`)
    assert_eq!(
        idx.count(def),
        4,
        "expected 1 decl + 3 uses, got spans = {:?}",
        idx.spans_for(def)
    );
}

#[test]
fn top_level_fn_collects_decl_plus_two_calls() {
    let src = load("top_level_fn_two_calls");
    let (result, idx) = index_of(&src);
    let def = def_by_name(&result, "greet_anchor_qzx");
    // 1 decl + 2 call sites
    assert_eq!(
        idx.count(def),
        3,
        "expected 1 decl + 2 calls, got spans = {:?}",
        idx.spans_for(def)
    );
}

#[test]
fn first_entry_is_the_definition_site() {
    // For a function, the first span must be the `def` line, which
    // covers `def greet_anchor_qzx`.
    let src = load("top_level_fn_two_calls");
    let (result, idx) = index_of(&src);
    let def = def_by_name(&result, "greet_anchor_qzx");
    let spans = idx.spans_for(def);
    assert!(!spans.is_empty(), "expected at least the decl span");
    // The def-site span should sit at the start of the source.
    // Riven's lexer is 1-indexed, so the first source line is `line == 1`.
    assert_eq!(
        spans[0].line, 1,
        "first entry should be the def site (line 1, 1-indexed), got line {}",
        spans[0].line
    );
}

#[test]
fn class_method_invocation_recorded() {
    let src = load("method_invocation");
    let (result, idx) = index_of(&src);
    let def = def_by_name(&result, "bump_anchor_qzx");
    // 1 decl + 1 call site (`c.bump_anchor_qzx`)
    assert_eq!(
        idx.count(def),
        2,
        "expected 1 decl + 1 method-call, got spans = {:?}",
        idx.spans_for(def)
    );
}

#[test]
fn field_access_lands_on_field_def() {
    let src = load("field_access");
    let (result, idx) = index_of(&src);
    let def = def_by_name(&result, "px_anchor_qzx");
    // 1 decl + 1 use (`p.px_anchor_qzx`).
    // Note: the constructor's `@px_anchor_qzx` is a *param* with its
    // own DefId, not the field — fields and shorthand-init params are
    // distinct defs in this resolver. So we expect the field to have
    // decl + 1 explicit `.px_anchor_qzx` access.
    let count = idx.count(def);
    assert!(
        count >= 2,
        "expected at least 1 decl + 1 field-access, got {} spans = {:?}",
        count,
        idx.spans_for(def)
    );
}

#[test]
fn class_in_let_annotation_recorded() {
    let src = load("class_in_let_annotation");
    let (result, idx) = index_of(&src);
    let def = def_by_name(&result, "WidgetTypeAnchorQzx");
    // 1 decl + at least 2 uses (annotation + constructor).
    let count = idx.count(def);
    assert!(
        count >= 3,
        "expected at least decl + annotation + ctor = 3 spans, got {}: {:?}",
        count,
        idx.spans_for(def)
    );
}

#[test]
fn class_never_referenced_has_only_decl_entry() {
    let src = load("class_never_referenced");
    let (result, idx) = index_of(&src);
    let def = def_by_name(&result, "LonelyAnchorQzx");
    assert_eq!(
        idx.count(def),
        1,
        "unreferenced class should have only the decl, got spans = {:?}",
        idx.spans_for(def)
    );
}

#[test]
fn parameter_used_twice_in_body() {
    let src = load("param_used_twice");
    let (result, idx) = index_of(&src);
    let def = def_by_name(&result, "input_anchor_qzx");
    // 1 decl (the param decl) + 2 body refs.
    assert_eq!(
        idx.count(def),
        3,
        "expected 1 decl + 2 body refs, got spans = {:?}",
        idx.spans_for(def)
    );
}

#[test]
fn skips_compiler_internal_double_underscore_defs() {
    // We can't directly name a `__`-prefixed def — none are emitted by
    // this simple program — but the contract is that NO key in the
    // index has a name starting with `__`. Sweep the symbol table for
    // any such def and verify they're absent from `uses`.
    let src = load("skips_synth_internals");
    let (result, idx) = index_of(&src);
    let symbols = result.symbols.as_ref().unwrap();
    for def in symbols.iter() {
        if def.name.starts_with("__") {
            assert!(
                !idx.uses.contains_key(&def.id),
                "compiler-internal def `{}` leaked into the index",
                def.name
            );
        }
    }
    // Sanity: the user-visible local is present.
    let visible = def_by_name(&result, "visible_anchor_qzx");
    assert!(idx.count(visible) >= 2, "decl + 1 use minimum");
}
