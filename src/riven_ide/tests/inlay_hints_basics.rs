//! Integration tests for `riven_ide::inlay_hints` (spec §5.5).
//!
//! Each fixture lives in `tests/fixtures/inlay_hints/<stem>.rvn` per
//! `feedback_no_inline_rvn_in_pin_tests.md`. Hint labels are matched by
//! their `value:` shape so the tests don't pin themselves to the
//! formatter's exact `Display for Ty` output.

use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range};
use riven_ide::analysis::analyze;
use riven_ide::inlay_hints::{inlay_hints, InlayHintConfig};

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/inlay_hints")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn full_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: u32::MAX,
            character: u32::MAX,
        },
    }
}

fn label(h: &InlayHint) -> &str {
    match &h.label {
        InlayHintLabel::String(s) => s.as_str(),
        InlayHintLabel::LabelParts(_) => "",
    }
}

#[test]
fn unannotated_let_gets_a_type_hint() {
    let src = load("let_unannotated");
    let result = analyze(&src);
    let hints = inlay_hints(&result, full_range(), &InlayHintConfig::default());
    let type_hints: Vec<&InlayHint> = hints
        .iter()
        .filter(|h| h.kind == Some(InlayHintKind::TYPE))
        .collect();
    assert_eq!(
        type_hints.len(),
        1,
        "expected exactly one type hint, got {:?}",
        hints.iter().map(label).collect::<Vec<_>>()
    );
    let lbl = label(type_hints[0]);
    assert!(
        lbl.starts_with(": "),
        "type hint label should start with ': ', got {:?}",
        lbl
    );
    assert!(lbl.contains("Int"), "expected Int in hint, got {:?}", lbl);
}

#[test]
fn annotated_let_emits_no_type_hint() {
    let src = load("let_annotated");
    let result = analyze(&src);
    let hints = inlay_hints(&result, full_range(), &InlayHintConfig::default());
    let any_type = hints.iter().any(|h| h.kind == Some(InlayHintKind::TYPE));
    assert!(
        !any_type,
        "annotated let must not produce a type hint, got: {:?}",
        hints.iter().map(label).collect::<Vec<_>>()
    );
}

#[test]
fn fn_call_emits_param_name_hints_for_each_arg() {
    let src = load("fncall_param_names");
    let result = analyze(&src);
    let hints = inlay_hints(&result, full_range(), &InlayHintConfig::default());
    let param_labels: Vec<&str> = hints
        .iter()
        .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
        .map(label)
        .collect();
    assert!(
        param_labels.contains(&"name:"),
        "expected `name:` param hint, got {:?}",
        param_labels
    );
    assert!(
        param_labels.contains(&"count:"),
        "expected `count:` param hint, got {:?}",
        param_labels
    );
}

#[test]
fn arg_matching_param_name_suppresses_the_hint() {
    let src = load("fncall_skip_when_arg_matches_param");
    let result = analyze(&src);
    let hints = inlay_hints(&result, full_range(), &InlayHintConfig::default());
    let param_labels: Vec<&str> = hints
        .iter()
        .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
        .map(label)
        .collect();
    // configure(host, port) — argument identifiers match param names,
    // so no PARAMETER hints should fire for the call.
    assert!(
        !param_labels.contains(&"host:"),
        "should suppress `host:` hint when arg is identifier `host`: {:?}",
        param_labels
    );
    assert!(
        !param_labels.contains(&"port:"),
        "should suppress `port:` hint when arg is identifier `port`: {:?}",
        param_labels
    );
}

#[test]
fn method_call_emits_param_name_hints() {
    let src = load("method_call_hints");
    let result = analyze(&src);
    let hints = inlay_hints(&result, full_range(), &InlayHintConfig::default());
    let param_labels: Vec<&str> = hints
        .iter()
        .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
        .map(label)
        .collect();
    assert!(
        param_labels.contains(&"lhs:"),
        "expected `lhs:` from c.add(2, 3): {:?}",
        param_labels
    );
    assert!(
        param_labels.contains(&"rhs:"),
        "expected `rhs:` from c.add(2, 3): {:?}",
        param_labels
    );
}

#[test]
fn hints_outside_requested_range_are_filtered_out() {
    let src = load("multiple_lets_for_range_filter");
    let result = analyze(&src);

    // Restrict the range to just the line containing `early_a`.
    let early_offset = src.find("early_a").expect("anchor missing");
    let early_pos = result.line_index.position_of(early_offset);
    let narrow = Range {
        start: Position {
            line: early_pos.line,
            character: 0,
        },
        end: Position {
            line: early_pos.line,
            character: u32::MAX,
        },
    };

    let hints = inlay_hints(&result, narrow, &InlayHintConfig::default());
    let type_hint_lines: Vec<u32> = hints
        .iter()
        .filter(|h| h.kind == Some(InlayHintKind::TYPE))
        .map(|h| h.position.line)
        .collect();

    assert!(
        !type_hint_lines.is_empty(),
        "expected at least one type hint inside range, got none"
    );
    for line in &type_hint_lines {
        assert_eq!(
            *line, early_pos.line,
            "hint on line {} leaked through the range filter (expected only line {})",
            line, early_pos.line
        );
    }
}

#[test]
fn show_type_hints_false_suppresses_type_hints() {
    let src = load("let_unannotated");
    let result = analyze(&src);
    let cfg = InlayHintConfig {
        show_type_hints: false,
        show_param_hints: true,
    };
    let hints = inlay_hints(&result, full_range(), &cfg);
    assert!(
        hints.iter().all(|h| h.kind != Some(InlayHintKind::TYPE)),
        "show_type_hints=false should suppress type hints, got {:?}",
        hints.iter().map(label).collect::<Vec<_>>()
    );
}

#[test]
fn show_param_hints_false_suppresses_param_hints() {
    let src = load("fncall_param_names");
    let result = analyze(&src);
    let cfg = InlayHintConfig {
        show_type_hints: true,
        show_param_hints: false,
    };
    let hints = inlay_hints(&result, full_range(), &cfg);
    assert!(
        hints
            .iter()
            .all(|h| h.kind != Some(InlayHintKind::PARAMETER)),
        "show_param_hints=false should suppress param hints, got {:?}",
        hints.iter().map(label).collect::<Vec<_>>()
    );
}
