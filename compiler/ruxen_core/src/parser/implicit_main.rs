use super::ast::{Block, FuncDef, Program, Statement, TopLevelItem, TypeExpr, Visibility};

/// Move top-level executable expressions into a compiler-synthesized `main`.
///
/// An explicit top-level `main` makes this pass a no-op. This preserves the
/// existing entry point exactly and leaves any stray top-level expressions for
/// the resolver to reject with E0728. A declaration-only program is also left
/// unchanged so the existing no-entry-point behavior remains authoritative.
pub fn synthesize_implicit_main(program: &mut Program) {
    if !needs_implicit_main(program) {
        return;
    }

    let items = std::mem::take(&mut program.items);
    let mut declarations = Vec::with_capacity(items.len() + 1);
    let mut statements = Vec::new();
    let mut first_span = None;

    for item in items {
        match item {
            TopLevelItem::Expr(expression) => {
                first_span.get_or_insert_with(|| expression.span.clone());
                statements.push(Statement::Expression(expression));
            }
            declaration => declarations.push(declaration),
        }
    }

    let span = first_span.expect("top-level expression presence checked before collection");
    declarations.push(TopLevelItem::Function(FuncDef {
        visibility: Visibility::Private,
        is_async: false,
        self_mode: None,
        is_class_method: false,
        name: "main".to_string(),
        generic_params: None,
        params: Vec::new(),
        return_type: Some(TypeExpr::Tuple {
            elements: Vec::new(),
            span: span.clone(),
        }),
        where_clause: None,
        body: Block {
            statements,
            span: span.clone(),
        },
        doc_comments: Vec::new(),
        span,
    }));
    program.items = declarations;
}

pub(crate) fn needs_implicit_main(program: &Program) -> bool {
    let mut has_main = false;
    let mut has_expression = false;
    for item in &program.items {
        match item {
            TopLevelItem::Function(function) if function.name == "main" => has_main = true,
            TopLevelItem::Expr(_) => has_expression = true,
            _ => {}
        }
    }
    has_expression && !has_main
}

#[cfg(test)]
mod tests {
    use super::synthesize_implicit_main;
    use crate::lexer::Lexer;
    use crate::parser::ast::{Statement, TopLevelItem};
    use crate::parser::Parser;
    use crate::resolve::Resolver;

    fn parse(source: &str) -> crate::parser::ast::Program {
        let tokens = Lexer::new(source).tokenize().expect("lex");
        Parser::new(tokens).parse().expect("parse")
    }

    #[test]
    fn resolve_implicit_main_collects_expressions_in_source_order() {
        let mut program =
            parse("x = compute()\nputs x\ndef compute -> Int\n  42\nend\ny = x + 1\n");
        let expression_spans: Vec<_> = program
            .items
            .iter()
            .filter_map(|item| match item {
                TopLevelItem::Expr(expression) => Some(expression.span.clone()),
                _ => None,
            })
            .collect();

        synthesize_implicit_main(&mut program);

        assert_eq!(program.items.len(), 2, "compute plus synthesized main");
        let TopLevelItem::Function(compute) = &program.items[0] else {
            panic!("definition must remain top-level");
        };
        assert_eq!(compute.name, "compute");
        let TopLevelItem::Function(main) = &program.items[1] else {
            panic!("expected synthesized main");
        };
        assert_eq!(main.name, "main");
        assert!(main.params.is_empty());
        assert_eq!(main.body.statements.len(), 3);
        let body_spans: Vec<_> = main
            .body
            .statements
            .iter()
            .map(|statement| match statement {
                Statement::Expression(expression) => expression.span.clone(),
                Statement::Let(_) => panic!("implicit main body must contain moved expressions"),
            })
            .collect();
        assert_eq!(body_spans, expression_spans);
        assert_eq!(main.span, expression_spans[0]);

        let result = Resolver::new().resolve(&program);
        assert!(
            result
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code.as_deref() != Some("E0728")),
            "implicit main must consume every top-level expression: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn resolve_explicit_main_without_stray_expressions_is_byte_for_byte_ast_noop() {
        let mut program = parse("def main\n  puts \"explicit\"\nend\n");
        let original = program.clone();

        synthesize_implicit_main(&mut program);

        assert_eq!(program, original);
    }

    #[test]
    fn resolve_explicit_main_with_stray_expression_keeps_e0728() {
        let mut program = parse("def main\n  puts \"explicit\"\nend\nputs \"stray\"\n");
        let original = program.clone();

        synthesize_implicit_main(&mut program);

        assert_eq!(program, original, "explicit main must disable synthesis");
        let result = Resolver::new().resolve(&program);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("E0728")),
            "expected explicit-main ambiguity to retain E0728: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn resolve_declaration_only_program_is_unchanged() {
        let mut program = parse("def helper -> Int\n  42\nend\n");
        let original = program.clone();

        synthesize_implicit_main(&mut program);

        assert_eq!(program, original);
    }
}
