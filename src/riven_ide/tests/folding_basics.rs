//! Folding-range fixture tests for Wave-1 LSP. Spec
//! `docs/requirements/tier3_01_lsp.md` §5.9.
//!
//! Per `feedback_no_inline_rvn_in_pin_tests.md`, every Riven source
//! lives in a `.rvn` fixture under `tests/fixtures/folding/`. Each
//! test loads its fixture by stem, runs analysis, and asserts on the
//! `Vec<FoldingRange>` returned by `folding_ranges`.

use lsp_types::{FoldingRange, FoldingRangeKind};
use riven_ide::analysis::analyze;
use riven_ide::folding::folding_ranges;

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/folding")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn ranges_of(stem: &str) -> Vec<FoldingRange> {
    let source = load(stem);
    let result = analyze(&source);
    folding_ranges(&result)
}

fn has_region(ranges: &[FoldingRange], start: u32, end: u32) -> bool {
    ranges.iter().any(|r| {
        r.start_line == start
            && r.end_line == end
            && r.kind == Some(FoldingRangeKind::Region)
    })
}

#[test]
fn def_body_folds_from_def_to_end() {
    // def_body.rvn: `def fold_anchor_simple_def` on line 0, `end` on line 5.
    let ranges = ranges_of("def_body");
    assert!(
        has_region(&ranges, 0, 5),
        "expected a Region range covering lines 0..=5 for the def body, got {:?}",
        ranges
    );
}

#[test]
fn class_with_methods_yields_multiple_ranges() {
    // class_with_methods.rvn: class on lines 0..=12, three method defs inside.
    // We don't care about exact end lines of methods for the assertion
    // — just that the class fold AND at least two method folds appear.
    let ranges = ranges_of("class_with_methods");
    let regions: Vec<&FoldingRange> = ranges
        .iter()
        .filter(|r| r.kind == Some(FoldingRangeKind::Region))
        .collect();
    assert!(
        regions.len() >= 4,
        "expected >=4 Region folds (1 class + 3 methods), got {} in {:?}",
        regions.len(),
        ranges
    );
    // The outermost fold should start at line 0 and end on the line
    // of the class `end`.
    assert!(
        regions.iter().any(|r| r.start_line == 0),
        "no fold starts at the class line: {:?}",
        ranges
    );
}

#[test]
fn if_elsif_else_chain_folds() {
    // if_elsif_else.rvn: def + nested if/elsif/else. The outer def
    // should fold; the `if` chain (including elsif desugared as
    // nested If) should also produce folds.
    let ranges = ranges_of("if_elsif_else");
    let region_count = ranges
        .iter()
        .filter(|r| r.kind == Some(FoldingRangeKind::Region))
        .count();
    // 1 def + 1 outer `if` + 1 `elsif` (nested If) = >= 3.
    assert!(
        region_count >= 3,
        "expected >=3 Region folds (def + if-chain), got {}: {:?}",
        region_count,
        ranges
    );
}

#[test]
fn loop_body_folds() {
    // loop_body.rvn: `def` on line 0, `loop` opens on line 2, `end`
    // for loop on line 6, `end` for def on line 8.
    let ranges = ranges_of("loop_body");
    let regions: Vec<&FoldingRange> = ranges
        .iter()
        .filter(|r| r.kind == Some(FoldingRangeKind::Region))
        .collect();
    // Need both: the def fold AND the loop fold.
    assert!(
        regions.iter().any(|r| r.start_line == 0),
        "no fold starts at def line 0: {:?}",
        ranges
    );
    assert!(
        regions.iter().any(|r| r.start_line == 2),
        "no fold starts at loop line 2: {:?}",
        ranges
    );
}

#[test]
fn consecutive_use_imports_collapse_into_one_range() {
    // uses_top_of_file.rvn: three `use` lines (0..=2), blank, then def.
    let ranges = ranges_of("uses_top_of_file");
    let imports: Vec<&FoldingRange> = ranges
        .iter()
        .filter(|r| r.kind == Some(FoldingRangeKind::Imports))
        .collect();
    assert_eq!(
        imports.len(),
        1,
        "expected exactly one Imports range, got {:?}",
        ranges
    );
    assert_eq!(imports[0].start_line, 0);
    assert_eq!(imports[0].end_line, 2);
}

#[test]
fn comment_block_of_three_lines_folds() {
    // comment_block.rvn: lines 0..=2 are `#`-comments, then a def.
    let ranges = ranges_of("comment_block");
    let comments: Vec<&FoldingRange> = ranges
        .iter()
        .filter(|r| r.kind == Some(FoldingRangeKind::Comment))
        .collect();
    assert_eq!(
        comments.len(),
        1,
        "expected exactly one Comment range, got {:?}",
        ranges
    );
    assert_eq!(comments[0].start_line, 0);
    assert_eq!(comments[0].end_line, 2);
}

#[test]
fn single_line_blocks_are_filtered_out() {
    // Every range in any fixture must satisfy end_line > start_line.
    // This is the invariant — single-line constructs (start == end)
    // are not foldable and must be dropped.
    for stem in [
        "def_body",
        "class_with_methods",
        "if_elsif_else",
        "loop_body",
        "uses_top_of_file",
        "comment_block",
        "single_line_block",
        "nested_match",
    ] {
        let ranges = ranges_of(stem);
        for r in &ranges {
            assert!(
                r.end_line > r.start_line,
                "fixture {} produced a single-line fold {:?}",
                stem,
                r
            );
        }
    }
}

#[test]
fn match_expression_folds() {
    // nested_match.rvn: def on line 0, match on line 1 ending line 5,
    // def end on line 6.
    let ranges = ranges_of("nested_match");
    let regions: Vec<&FoldingRange> = ranges
        .iter()
        .filter(|r| r.kind == Some(FoldingRangeKind::Region))
        .collect();
    assert!(
        regions.iter().any(|r| r.start_line == 0),
        "no fold starts at def line 0: {:?}",
        ranges
    );
    assert!(
        regions.iter().any(|r| r.start_line == 1),
        "no fold starts at match line 1: {:?}",
        ranges
    );
}
