/// Tests for the Ruxen code formatter.
use super::*;

/// Load a formatter test fixture from `tests/fixtures/fmt/<name>.rx`.
/// Per the team's no-inline-rx-source rule, formatter inputs live as
/// real `.rx` files (which the parser/lexer themselves exercise),
/// not as `\n`-laden Rust string literals.
fn fmt_fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fmt")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

// ─── Idempotency Helper ─────────────────────────────────────────────

fn assert_idempotent(source: &str) {
    let first = format(source);
    let second = format(&first.output);
    assert_eq!(
        first.output, second.output,
        "Formatter is not idempotent!\nFirst pass:\n{}\nSecond pass:\n{}",
        first.output, second.output
    );
}

#[allow(dead_code)]
fn assert_formats_to(source: &str, expected: &str) {
    let result = format(source);
    assert_eq!(result.output, expected, "\nGot:\n{}", result.output);
    assert_idempotent(source);
}

#[allow(dead_code)]
fn assert_unchanged(source: &str) {
    let result = format(source);
    assert!(
        !result.changed,
        "Expected no change, but got:\n{}",
        result.output
    );
}

// ─── Basic Formatting ───────────────────────────────────────────────

#[test]
fn test_hello_world() {
    let source = fmt_fixture("test_hello_world");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(result.output.contains("def main"));
    assert!(result.output.ends_with('\n'));
    assert_idempotent(&source);
}

#[test]
fn test_simple_function() {
    let source = fmt_fixture("test_simple_function");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(result.output.contains("def add"));
    assert_idempotent(&source);
}

#[test]
fn test_class_definition() {
    let source = fmt_fixture("test_class_definition");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(result.output.contains("class Point"));
    assert!(result.output.contains("x: Int"));
    assert_idempotent(&source);
}

#[test]
fn test_enum_definition() {
    let source = fmt_fixture("test_enum_definition");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(result.output.contains("enum Color"));
    assert!(result.output.contains("Red"));
    assert_idempotent(&source);
}

#[test]
fn test_if_else() {
    let source = fmt_fixture("test_if_else");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(result.output.contains("if x > 0"));
    assert_idempotent(&source);
}

#[test]
fn test_match_expression() {
    let source = fmt_fixture("test_match_expression");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(result.output.contains("match c"));
    assert_idempotent(&source);
}

#[test]
fn test_trailing_newline() {
    let source = fmt_fixture("test_trailing_newline");
    let result = format(&source);
    assert!(result.output.ends_with('\n'));
}

// ─── Parse Error Handling ───────────────────────────────────────────

#[test]
fn test_syntax_error_returns_original() {
    let source = fmt_fixture("test_syntax_error_returns_original");
    let result = format(&source);
    assert_eq!(result.output, source);
    assert!(!result.changed);
}

// ─── Comment Preservation ───────────────────────────────────────────

#[test]
fn test_line_comment_preserved() {
    let source = fmt_fixture("test_line_comment_preserved");
    let result = format(&source);
    assert!(
        result.output.contains("# A comment"),
        "Comment missing from output: {}",
        result.output
    );
}

#[test]
fn test_doc_comment_preserved() {
    let source = fmt_fixture("test_doc_comment_preserved");
    let result = format(&source);
    assert!(
        result.output.contains("## Documentation"),
        "Doc comment missing from output: {}",
        result.output
    );
}

// ─── String Interpolation ───────────────────────────────────────────

#[test]
fn test_string_interpolation() {
    let source = fmt_fixture("test_string_interpolation");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(
        result.output.contains("#{x}") || result.output.contains("#{"),
        "Interpolation missing: {}",
        result.output
    );
    assert_idempotent(&source);
}

// ─── Mixin Inclusion (ruby-naming.spec.md §3.4 / §10a) ──────────────
// Legacy `impl Trait for Type ... end` is folded into the type body
// as an `include Trait` directive with methods scattered alongside.

#[test]
fn test_class_inherent_methods() {
    let source = fmt_fixture("test_class_inherent_methods");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(result.output.contains("class Priority"));
    assert_idempotent(&source);
}

#[test]
fn test_class_with_include_directive() {
    let source = fmt_fixture("test_class_with_include_directive");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(result.output.contains("include Display"));
    assert!(result.output.contains("class Priority"));
    assert_idempotent(&source);
}

// ─── Enum with Data ─────────────────────────────────────────────────

#[test]
fn test_enum_with_data() {
    let source = fmt_fixture("test_enum_with_data");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    // The formatter outputs variant fields with parentheses
    assert!(
        result.output.contains("Circle"),
        "Missing Circle in:\n{}",
        result.output
    );
    assert!(
        result.output.contains("radius"),
        "Missing radius in:\n{}",
        result.output
    );
    assert_idempotent(&source);
}

// ─── Mixin Definitions (ruby-naming.spec.md §3.4) ───────────────────

#[test]
fn test_mixin_definition() {
    let source = fmt_fixture("test_mixin_definition");
    let result = format(&source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert!(result.output.contains("Serializable"));
    assert_idempotent(&source);
}

// ─── Fixture File Tests ─────────────────────────────────────────────

macro_rules! fixture_test {
    ($name:ident, $file:expr) => {
        #[test]
        fn $name() {
            let source = std::fs::read_to_string(concat!("tests/fixtures/", $file))
                .expect(&format!("Failed to read fixture file: {}", $file));
            let result = format(&source);
            assert!(
                result.errors.is_empty(),
                "Fixture {} had format errors: {:?}",
                $file,
                result.errors
            );

            // Idempotency check
            let second = format(&result.output);
            assert_eq!(
                result.output, second.output,
                "Fixture {} is not idempotent!\nFirst:\n{}\nSecond:\n{}",
                $file, result.output, second.output
            );
        }
    };
}

fixture_test!(test_fixture_hello, "hello.rx");
fixture_test!(test_fixture_arithmetic, "arithmetic.rx");
fixture_test!(test_fixture_control_flow, "control_flow.rx");
fixture_test!(test_fixture_functions, "functions.rx");
fixture_test!(test_fixture_string_interp, "string_interp.rx");
fixture_test!(test_fixture_simple_class, "simple_class.rx");
fixture_test!(test_fixture_classes, "classes.rx");
fixture_test!(test_fixture_class_methods, "class_methods.rx");
fixture_test!(test_fixture_enums, "enums.rx");
fixture_test!(test_fixture_enum_data, "enum_data.rx");
fixture_test!(test_fixture_tasklist, "tasklist.rx");
fixture_test!(test_fixture_mini_sample, "mini_sample.rx");
fixture_test!(test_fixture_sample_program, "sample_program.rx");

// ─── Doc IR Tests ───────────────────────────────────────────────────

#[test]
fn test_doc_nest_indent() {
    use super::doc::*;
    let doc = concat(vec![
        text("class Foo"),
        nest(INDENT_WIDTH, concat(vec![hardline(), text("x: Int")])),
        hardline(),
        text("end"),
    ]);
    assert_eq!(render(&doc), "class Foo\n  x: Int\nend");
}

#[test]
fn test_doc_group_break_on_narrow() {
    use super::doc::*;
    let doc = group(concat(vec![
        text("def f("),
        nest(
            INDENT_WIDTH,
            concat(vec![
                softline(),
                text("a: Int"),
                text(","),
                line(),
                text("b: Int"),
            ]),
        ),
        softline(),
        text(")"),
    ]));
    // Wide: fits on one line
    assert_eq!(print_doc(&doc, 100), "def f(a: Int, b: Int)");
    // Narrow: breaks
    let narrow = print_doc(&doc, 15);
    assert!(
        narrow.contains('\n'),
        "Expected line break in narrow mode: {}",
        narrow
    );
}

// ─── Comment Collector Tests ────────────────────────────────────────

#[test]
fn test_comment_collector_multiple() {
    let source = fmt_fixture("test_comment_collector_multiple");
    let collector = comments::CommentCollector::new(&source);
    let (comments, _) = collector.collect();
    assert_eq!(comments.len(), 2);
}

#[test]
fn test_comment_inside_interpolation_ignored() {
    let source = fmt_fixture("test_comment_inside_interpolation_ignored");
    let collector = comments::CommentCollector::new(&source);
    let (comments, _) = collector.collect();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].kind, comments::CommentKind::Line);
}

// ─── Import Sorting Tests ───────────────────────────────────────────

#[test]
fn test_import_sorting_groups() {
    use super::format_imports::format_sorted_imports;
    use crate::lexer::token::Span;
    use crate::parser::ast::{UseDecl, UseKind};

    let imports = vec![
        UseDecl {
            path: vec!["Http".into(), "Client".into()],
            kind: UseKind::Simple,
            span: Span::new(0, 0, 1, 1),
        },
        UseDecl {
            path: vec!["Std".into(), "IO".into(), "File".into()],
            kind: UseKind::Simple,
            span: Span::new(0, 0, 1, 1),
        },
        UseDecl {
            path: vec!["app".into(), "models".into()],
            kind: UseKind::Simple,
            span: Span::new(0, 0, 1, 1),
        },
    ];

    let doc = format_sorted_imports(&imports);
    let rendered = doc::render(&doc);
    let lines: Vec<&str> = rendered.lines().collect();

    // Std should come first
    assert!(
        lines[0].contains("Std"),
        "First line should be Std import: {}",
        lines[0]
    );
}

// ─── Semantic round-trip regressions (fmt-in-sync-with-parser) ──────
//
// Each of these formats a source the parser accepts, then re-parses the
// formatter's output and asserts it still parses. They pin the fixes for the
// drift catalogued by `tests/formatter_corpus_roundtrip.rs`: the formatter
// must never emit a spelling the current parser rejects.

fn assert_reparses(source: &str) -> String {
    let result = format(source);
    assert!(
        result.errors.is_empty(),
        "format errors: {:?}",
        result.errors
    );
    let mut lexer = crate::lexer::Lexer::new(&result.output);
    let tokens = lexer.tokenize().unwrap_or_else(|d| {
        panic!(
            "lex of formatted output failed: {:?}\n---\n{}",
            d, result.output
        )
    });
    let mut parser = crate::parser::Parser::new(tokens);
    parser.parse().unwrap_or_else(|d| {
        panic!(
            "formatted output no longer parses: {:?}\n--- output ---\n{}",
            d.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
            result.output
        )
    });
    assert_idempotent(source);
    result.output
}

#[test]
fn fields_have_no_pub_prefix() {
    // Public is the default; `pub fd: Int` is not valid field syntax.
    let out = assert_reparses(&fmt_fixture("fields_have_no_pub_prefix"));
    assert!(!out.contains("pub "), "spurious pub prefix:\n{}", out);
    assert!(out.contains("fd: Int"), "{}", out);
}

#[test]
fn private_fields_use_section_marker() {
    // No corpus file exercises non-public fields; pin the section-marker form.
    let src = fmt_fixture("private_fields_use_section_marker");
    let out = assert_reparses(&src);
    assert!(
        out.contains("private"),
        "expected private section marker:\n{}",
        out
    );
    assert!(!out.contains("pub "), "{}", out);
}

#[test]
fn lib_block_name_keeps_quotes() {
    let src = fmt_fixture("lib_block_name_keeps_quotes");
    let out = assert_reparses(&src);
    assert!(
        out.contains("lib \"runtime/env.c\""),
        "lib name lost quotes:\n{}",
        out
    );
}

#[test]
fn ffi_class_method_keeps_self() {
    let src = fmt_fixture("ffi_class_method_keeps_self");
    let out = assert_reparses(&src);
    assert!(out.contains("def self.args"), "FFI self. dropped:\n{}", out);
}

#[test]
fn mixin_method_sig_def_before_var() {
    let src = fmt_fixture("mixin_method_sig_def_before_var");
    let out = assert_reparses(&src);
    assert!(
        out.contains("def var poll"),
        "expected `def var`, got:\n{}",
        out
    );
    assert!(!out.contains("var def"), "emitted `var def`:\n{}", out);
}

#[test]
fn extension_keyword_not_impl() {
    let src = fmt_fixture("extension_keyword_not_impl");
    let out = assert_reparses(&src);
    assert!(
        out.contains("extension Int"),
        "expected `extension`, got:\n{}",
        out
    );
}

#[test]
fn const_without_type_omits_annotation() {
    let out = assert_reparses(&fmt_fixture("const_without_type_omits_annotation"));
    assert!(
        !out.contains(": _"),
        "spurious inferred-type annotation:\n{}",
        out
    );
    assert!(out.contains("const MAX = 100"), "{}", out);
}

#[test]
fn do_end_closure_preserved() {
    // `let v = do … end` is a no-param closure bound to `v`; the formatter
    // must keep the `do…end` wrapper rather than rewriting it to braces.
    let src = fmt_fixture("do_end_block_expression_preserved");
    let out = assert_reparses(&src);
    assert!(out.contains("= do"), "do...end wrapper lost:\n{}", out);
    assert!(out.contains("end"), "{}", out);
}

#[test]
fn move_closure_keeps_move() {
    let src = fmt_fixture("move_closure_keeps_move");
    let out = assert_reparses(&src);
    assert!(
        out.contains("move {") || out.contains("move do"),
        "move keyword lost:\n{}",
        out
    );
}

#[test]
fn never_type_spelled_never() {
    let src = fmt_fixture("never_type_spelled_never");
    let out = assert_reparses(&src);
    assert!(out.contains("-> Never"), "Never emitted as `!`:\n{}", out);
    assert!(!out.contains("-> !"), "{}", out);
}

#[test]
fn multi_statement_match_arm_has_no_end() {
    let src = fmt_fixture("multi_statement_match_arm_has_no_end");
    let out = assert_reparses(&src);
    // Exactly two `end`s: the match and the def. A per-arm `end` would make 3.
    assert_eq!(
        out.matches("end").count(),
        2,
        "spurious arm `end`:\n{}",
        out
    );
}
