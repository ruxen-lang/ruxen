//! Hover must render signatures the same way `ruxen fmt` does — in
//! particular, a no-parameter `def` is written WITHOUT parens (`def f`),
//! never `def f()`. See the cross-binary parity rule.

use ruxen_ide::analysis::analyze;
use ruxen_ide::hover::hover_at;

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hover")
        .join(format!("{stem}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Hovering a no-parameter `def` renders `def get_items -> Array[Int]`
/// (no parens), matching the formatter — not `def get_items()`.
#[test]
fn hover_no_param_def_omits_parens() {
    let source = load("no_param_def");
    let result = analyze(&source);

    // Cursor on the `get_items` reference in `let xs = get_items`.
    let offset = source.rfind("get_items").expect("reference present");
    let position = result.line_index.position_of(offset);
    let hover = hover_at(&result, position).expect("hover available on get_items");

    assert!(
        hover.content.contains("def get_items -> Array[Int]"),
        "hover should render the no-param def without parens; got:\n{}",
        hover.content
    );
    assert!(
        !hover.content.contains("get_items()"),
        "hover must NOT add `()` to a no-param def (diverges from `ruxen fmt`); got:\n{}",
        hover.content
    );
}
