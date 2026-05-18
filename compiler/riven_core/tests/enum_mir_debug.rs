//! MIR lowering integration tests for milestone fixtures.

use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;

fn typecheck_source(name: &str) -> riven_core::typeck::TypeCheckResult {
    let source = std::fs::read_to_string(format!("tests/fixtures/{}", name))
        .unwrap_or_else(|e| panic!("failed to read {}: {}", name, e));
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");
    typeck::type_check(&program)
}

/// Milestone 5: enums.rvn type-checks and lowers to MIR without errors.
#[test]
fn enum_mir_lowering() {
    let result = typecheck_source("enums.rvn");
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");
    assert!(
        mir.functions.len() >= 2,
        "expected at least 2 functions (describe + main)"
    );
    assert_eq!(mir.entry, Some("main".to_string()));
}

/// Enum data payloads: enum_data.rvn type-checks and lowers to MIR without errors.
#[test]
fn enum_data_mir_lowering() {
    let result = typecheck_source("enum_data.rvn");
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");
    assert!(
        mir.functions.len() >= 2,
        "expected at least 2 functions (area + main)"
    );
    assert_eq!(mir.entry, Some("main".to_string()));

    // Verify the area function has payload extraction (GetPayload + GetField)
    let area_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "area")
        .expect("should have 'area' function");
    let has_get_payload = area_fn.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|i| matches!(i, riven_core::mir::nodes::MirInst::GetPayload { .. }))
    });
    assert!(
        has_get_payload,
        "area function should contain GetPayload for enum data extraction"
    );

    let has_get_field = area_fn.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|i| matches!(i, riven_core::mir::nodes::MirInst::GetField { .. }))
    });
    assert!(
        has_get_field,
        "area function should contain GetField for payload field access"
    );
}

/// Milestone 6: classes.rvn type-checks and lowers to MIR without errors.
#[test]
fn classes_mir_lowering() {
    let result = typecheck_source("classes.rvn");
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");
    assert!(mir.functions.len() >= 2, "expected at least 2 functions");
    assert_eq!(mir.entry, Some("main".to_string()));
}
