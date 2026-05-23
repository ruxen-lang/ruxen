//! Pin: tuple-field access with multi-digit indices preserves the index.
//! Regression: `t.0.10` used to lex `0.10` as `FloatLiteral(0.1)` then
//! parser format!-split it into ["0", "1"] — silent miscompile to field 1.
use riven_core::lexer::Lexer;
use riven_core::parser::ast::ExprKind;
use riven_core::parser::Parser;

#[test]
fn tuple_field_multi_digit_index_preserves_value() {
    // We need a wrapping context so the parser sees a tuple field
    // access as a statement-level expr.
    let source = "def main\n  let x = t.0.10\nend\n";
    let tokens = Lexer::new(source).tokenize().expect("lex");
    let prog = Parser::new(tokens).parse().expect("parse");
    // Find the let value: should be FieldAccess(FieldAccess(t, "0"), "10").
    let main = prog
        .items
        .iter()
        .find_map(|item| {
            if let riven_core::parser::ast::TopLevelItem::Function(f) = item {
                if f.name == "main" {
                    Some(f)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .expect("main fn");
    let body = &main.body;
    let stmt = body.statements.first().expect("a statement");
    let value = match stmt {
        riven_core::parser::ast::Statement::Let(l) => l.value.as_ref().expect("value"),
        _ => panic!("expected let stmt, got {:?}", stmt),
    };
    let (outer_obj, outer_field) = match &value.kind {
        ExprKind::FieldAccess { object, field } => (object, field),
        other => panic!("expected outer FieldAccess, got {:?}", other),
    };
    assert_eq!(outer_field, "10", "outer tuple field must be `10`, not `1`");
    let (_inner_obj, inner_field) = match &outer_obj.kind {
        ExprKind::FieldAccess { object, field } => (object, field),
        other => panic!("expected inner FieldAccess, got {:?}", other),
    };
    assert_eq!(inner_field, "0", "inner tuple field must be `0`");
}
