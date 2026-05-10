#[cfg(test)]
mod tests {
    use crate::lexer::Lexer;
    use crate::parser::ast::*;
    use crate::parser::Parser;

    fn parse(input: &str) -> Program {
        let mut lexer = Lexer::new(input);
        let tokens = lexer.tokenize().expect("lexer failed");
        let mut parser = Parser::new(tokens);
        parser.parse().expect("parser failed")
    }

    fn parse_expr(input: &str) -> Expr {
        let wrapped = format!("def _test_\n  {}\nend", input);
        let program = parse(&wrapped);
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", std::mem::discriminant(other)),
        };
        match &func.body.statements[0] {
            Statement::Expression(e) => e.clone(),
            other => panic!("expected expression statement, got {:?}", other),
        }
    }

    fn parse_stmt(input: &str) -> Statement {
        let wrapped = format!("def _test_\n  {}\nend", input);
        let program = parse(&wrapped);
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", std::mem::discriminant(other)),
        };
        func.body.statements[0].clone()
    }

    fn parse_err(input: &str) -> Vec<crate::diagnostics::Diagnostic> {
        let mut lexer = Lexer::new(input);
        let tokens = lexer.tokenize().expect("lexer failed");
        let mut parser = Parser::new(tokens);
        parser.parse().expect_err("parser should fail")
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Let Bindings
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn let_simple() {
        let stmt = parse_stmt("let x = 42");
        match stmt {
            Statement::Let(binding) => {
                assert!(!binding.mutable);
                assert!(binding.type_annotation.is_none());
                assert!(binding.value.is_some());
                match &binding.pattern {
                    Pattern::Identifier { name, mutable, .. } => {
                        assert_eq!(name, "x");
                        assert!(!mutable);
                    }
                    other => panic!("expected identifier pattern, got {:?}", other),
                }
                match &binding.value.as_ref().unwrap().kind {
                    ExprKind::IntLiteral(42, None) => {}
                    other => panic!("expected IntLiteral(42), got {:?}", other),
                }
            }
            other => panic!("expected let binding, got {:?}", other),
        }
    }

    #[test]
    fn let_mutable_with_type() {
        let stmt = parse_stmt("let mut y: Int = 0");
        match stmt {
            Statement::Let(binding) => {
                assert!(binding.mutable);
                match &binding.pattern {
                    Pattern::Identifier { name, .. } => assert_eq!(name, "y"),
                    other => panic!("expected identifier pattern, got {:?}", other),
                }
                match &binding.type_annotation {
                    Some(TypeExpr::Named(path)) => {
                        assert_eq!(path.segments, vec!["Int"]);
                    }
                    other => panic!("expected Int type annotation, got {:?}", other),
                }
                match &binding.value.as_ref().unwrap().kind {
                    ExprKind::IntLiteral(0, None) => {}
                    other => panic!("expected IntLiteral(0), got {:?}", other),
                }
            }
            other => panic!("expected let binding, got {:?}", other),
        }
    }

    #[test]
    fn let_destructuring_tuple() {
        let stmt = parse_stmt("let (a, b) = (1, 2)");
        match stmt {
            Statement::Let(binding) => {
                assert!(!binding.mutable);
                match &binding.pattern {
                    Pattern::Tuple { elements, .. } => {
                        assert_eq!(elements.len(), 2);
                        match &elements[0] {
                            Pattern::Identifier { name, .. } => assert_eq!(name, "a"),
                            other => panic!("expected ident 'a', got {:?}", other),
                        }
                        match &elements[1] {
                            Pattern::Identifier { name, .. } => assert_eq!(name, "b"),
                            other => panic!("expected ident 'b', got {:?}", other),
                        }
                    }
                    other => panic!("expected tuple pattern, got {:?}", other),
                }
                match &binding.value.as_ref().unwrap().kind {
                    ExprKind::TupleLiteral(elems) => {
                        assert_eq!(elems.len(), 2);
                    }
                    other => panic!("expected tuple literal, got {:?}", other),
                }
            }
            other => panic!("expected let binding, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Functions
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn func_basic_with_return_type() {
        let program = parse("def foo(x: Int) -> Int\n  x + 1\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
        assert_eq!(func.name, "foo");
        assert!(!func.is_class_method);
        assert_eq!(func.self_mode, None);
        assert_eq!(func.params.len(), 1);
        assert_eq!(func.params[0].name, "x");
        assert!(!func.params[0].auto_assign);
        assert!(func.return_type.is_some());
        match &func.return_type {
            Some(TypeExpr::Named(path)) => assert_eq!(path.segments, vec!["Int"]),
            other => panic!("expected Int return type, got {:?}", other),
        }
        assert_eq!(func.body.statements.len(), 1);
    }

    #[test]
    fn func_mutable_self_mode() {
        let program = parse("def mut set_name(name: String)\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
        assert_eq!(func.name, "set_name");
        assert_eq!(func.self_mode, Some(SelfMode::Mutable));
        assert!(!func.is_class_method);
        assert_eq!(func.params.len(), 1);
        assert_eq!(func.params[0].name, "name");
    }

    #[test]
    fn func_consuming_self_mode() {
        let program = parse("def consume into_string -> String\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
        assert_eq!(func.name, "into_string");
        assert_eq!(func.self_mode, Some(SelfMode::Consuming));
        match &func.return_type {
            Some(TypeExpr::Named(path)) => assert_eq!(path.segments, vec!["String"]),
            other => panic!("expected String return type, got {:?}", other),
        }
    }

    #[test]
    fn func_class_method() {
        let program = parse("def self.create -> Self\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
        assert_eq!(func.name, "create");
        assert!(func.is_class_method);
        match &func.return_type {
            Some(TypeExpr::Named(path)) => assert_eq!(path.segments, vec!["Self"]),
            other => panic!("expected Self return type, got {:?}", other),
        }
    }

    #[test]
    fn func_init_with_auto_assign() {
        let program = parse("def init(@name: String, @age: Int)\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
        assert_eq!(func.name, "init");
        assert_eq!(func.params.len(), 2);
        assert!(func.params[0].auto_assign);
        assert_eq!(func.params[0].name, "name");
        assert!(func.params[1].auto_assign);
        assert_eq!(func.params[1].name, "age");
    }

    #[test]
    fn func_generic() {
        let program = parse("def find[T: Comparable](list: &Vec[T]) -> Option[&T]\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
        assert_eq!(func.name, "find");
        let gp = func
            .generic_params
            .as_ref()
            .expect("expected generic params");
        assert_eq!(gp.params.len(), 1);
        match &gp.params[0] {
            GenericParam::Type { name, bounds, .. } => {
                assert_eq!(name, "T");
                assert_eq!(bounds.len(), 1);
                assert_eq!(bounds[0].path.segments, vec!["Comparable"]);
            }
            other => panic!("expected type param, got {:?}", other),
        }
        // Check param type is a reference
        match &func.params[0].type_expr {
            TypeExpr::Reference { inner, mutable, .. } => {
                assert!(!mutable);
                match inner.as_ref() {
                    TypeExpr::Named(path) => {
                        assert_eq!(path.segments, vec!["Vec"]);
                        assert!(path.generic_args.is_some());
                    }
                    other => panic!("expected Named(Vec[T]), got {:?}", other),
                }
            }
            other => panic!("expected reference type, got {:?}", other),
        }
        // Check return type Option[&T]
        match &func.return_type {
            Some(TypeExpr::Named(path)) => {
                assert_eq!(path.segments, vec!["Option"]);
                let args = path.generic_args.as_ref().unwrap();
                assert_eq!(args.len(), 1);
                match &args[0] {
                    TypeExpr::Reference { inner, .. } => match inner.as_ref() {
                        TypeExpr::Named(p) => assert_eq!(p.segments, vec!["T"]),
                        other => panic!("expected Named(T), got {:?}", other),
                    },
                    other => panic!("expected reference type, got {:?}", other),
                }
            }
            other => panic!("expected Option return type, got {:?}", other),
        }
    }

    #[test]
    fn async_func_basic() {
        let program = parse("async def fetch(id: Int) -> Int\n  id\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
        assert!(func.is_async);
        assert_eq!(func.name, "fetch");
        assert_eq!(func.params.len(), 1);
    }

    #[test]
    fn pub_async_func_basic() {
        let program = parse("pub async def fetch\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
        assert_eq!(func.visibility, Visibility::Public);
        assert!(func.is_async);
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Classes
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn class_with_fields_and_methods() {
        let src = "\
class Person
  name: String
  age: Int

  def init(@name: String, @age: Int)
  end

  def greet -> String
  end
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };
        assert_eq!(class.name, "Person");
        assert_eq!(class.fields.len(), 2);
        assert_eq!(class.fields[0].name, "name");
        assert_eq!(class.fields[1].name, "age");
        assert_eq!(class.methods.len(), 2);
        assert_eq!(class.methods[0].name, "init");
        assert_eq!(class.methods[1].name, "greet");
    }

    #[test]
    fn class_with_inheritance() {
        let src = "\
class Child < Parent
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };
        assert_eq!(class.name, "Child");
        let parent = class.parent.as_ref().expect("expected parent");
        assert_eq!(parent.segments, vec!["Parent"]);
    }

    #[test]
    fn class_with_generics() {
        let src = "\
class Container[T: Displayable]
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };
        assert_eq!(class.name, "Container");
        let gp = class
            .generic_params
            .as_ref()
            .expect("expected generic params");
        assert_eq!(gp.params.len(), 1);
        match &gp.params[0] {
            GenericParam::Type { name, bounds, .. } => {
                assert_eq!(name, "T");
                assert_eq!(bounds[0].path.segments, vec!["Displayable"]);
            }
            other => panic!("expected type param, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Enums
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn enum_simple_unit_variants() {
        let src = "\
enum Color
  Red
  Green
  Blue
end";
        let program = parse(src);
        let en = match &program.items[0] {
            TopLevelItem::Enum(e) => e,
            other => panic!("expected enum, got {:?}", other),
        };
        assert_eq!(en.name, "Color");
        assert!(en.generic_params.is_none());
        assert_eq!(en.variants.len(), 3);
        assert_eq!(en.variants[0].name, "Red");
        assert!(matches!(en.variants[0].fields, VariantKind::Unit));
        assert_eq!(en.variants[1].name, "Green");
        assert_eq!(en.variants[2].name, "Blue");
    }

    #[test]
    fn enum_with_data_and_generics() {
        let src = "\
enum Result[T]
  Success(T)
  Failure(String)
end";
        let program = parse(src);
        let en = match &program.items[0] {
            TopLevelItem::Enum(e) => e,
            other => panic!("expected enum, got {:?}", other),
        };
        assert_eq!(en.name, "Result");
        let gp = en.generic_params.as_ref().expect("expected generic params");
        assert_eq!(gp.params.len(), 1);
        assert_eq!(en.variants.len(), 2);
        assert_eq!(en.variants[0].name, "Success");
        match &en.variants[0].fields {
            VariantKind::Tuple(fields) => {
                assert_eq!(fields.len(), 1);
                assert!(fields[0].name.is_none());
            }
            other => panic!("expected tuple variant, got {:?}", other),
        }
        assert_eq!(en.variants[1].name, "Failure");
        match &en.variants[1].fields {
            VariantKind::Tuple(fields) => {
                assert_eq!(fields.len(), 1);
            }
            other => panic!("expected tuple variant, got {:?}", other),
        }
    }

    #[test]
    fn enum_with_named_fields() {
        let src = "\
enum Status
  InProgress(assignee: String)
end";
        let program = parse(src);
        let en = match &program.items[0] {
            TopLevelItem::Enum(e) => e,
            other => panic!("expected enum, got {:?}", other),
        };
        assert_eq!(en.variants.len(), 1);
        assert_eq!(en.variants[0].name, "InProgress");
        match &en.variants[0].fields {
            VariantKind::Struct(fields) => {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].name.as_deref(), Some("assignee"));
            }
            other => panic!("expected struct variant, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Traits
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn trait_with_method_signature() {
        let src = "\
trait Greetable
  def greet -> String
end";
        let program = parse(src);
        let tr = match &program.items[0] {
            TopLevelItem::Trait(t) => t,
            other => panic!("expected trait, got {:?}", other),
        };
        assert_eq!(tr.name, "Greetable");
        assert_eq!(tr.items.len(), 1);
        match &tr.items[0] {
            TraitItem::MethodSig(sig) => {
                assert_eq!(sig.name, "greet");
                assert!(sig.return_type.is_some());
            }
            other => panic!("expected method signature, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Impl Blocks
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn impl_trait_for_type() {
        let src = "\
impl Greetable for Person
  def greet -> String
  end
end";
        let program = parse(src);
        let imp = match &program.items[0] {
            TopLevelItem::Impl(i) => i,
            other => panic!("expected impl, got {:?}", other),
        };
        let trait_name = imp.trait_name.as_ref().expect("expected trait name");
        assert_eq!(trait_name.segments, vec!["Greetable"]);
        match &imp.target_type {
            TypeExpr::Named(path) => assert_eq!(path.segments, vec!["Person"]),
            other => panic!("expected Named type, got {:?}", other),
        }
        assert_eq!(imp.items.len(), 1);
    }

    #[test]
    fn impl_inherent() {
        let src = "\
impl Person
  def hello -> String
  end
end";
        let program = parse(src);
        let imp = match &program.items[0] {
            TopLevelItem::Impl(i) => i,
            other => panic!("expected impl, got {:?}", other),
        };
        assert!(imp.trait_name.is_none());
        match &imp.target_type {
            TypeExpr::Named(path) => assert_eq!(path.segments, vec!["Person"]),
            other => panic!("expected Named type, got {:?}", other),
        }
        assert_eq!(imp.items.len(), 1);
    }

    #[test]
    fn unsafe_impl_trait_for_type() {
        let src = "\
unsafe impl Send for Buffer
end";
        let program = parse(src);
        let imp = match &program.items[0] {
            TopLevelItem::Impl(i) => i,
            other => panic!("expected impl, got {:?}", other),
        };
        assert!(imp.is_unsafe);
        assert!(!imp.negative_trait);
        let trait_name = imp.trait_name.as_ref().expect("expected trait name");
        assert_eq!(trait_name.segments, vec!["Send"]);
    }

    #[test]
    fn negative_impl_trait_for_type() {
        let src = "\
impl !Sync for Buffer
end";
        let program = parse(src);
        let imp = match &program.items[0] {
            TopLevelItem::Impl(i) => i,
            other => panic!("expected impl, got {:?}", other),
        };
        assert!(!imp.is_unsafe);
        assert!(imp.negative_trait);
        let trait_name = imp.trait_name.as_ref().expect("expected trait name");
        assert_eq!(trait_name.segments, vec!["Sync"]);
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Binary Precedence
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn binary_precedence_mul_over_add() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        let expr = parse_expr("1 + 2 * 3");
        match &expr.kind {
            ExprKind::BinaryOp { left, op, right } => {
                assert_eq!(*op, BinOp::Add);
                match &left.kind {
                    ExprKind::IntLiteral(1, _) => {}
                    other => panic!("expected 1, got {:?}", other),
                }
                match &right.kind {
                    ExprKind::BinaryOp {
                        left: l2,
                        op: op2,
                        right: r2,
                    } => {
                        assert_eq!(*op2, BinOp::Mul);
                        match &l2.kind {
                            ExprKind::IntLiteral(2, _) => {}
                            other => panic!("expected 2, got {:?}", other),
                        }
                        match &r2.kind {
                            ExprKind::IntLiteral(3, _) => {}
                            other => panic!("expected 3, got {:?}", other),
                        }
                    }
                    other => panic!("expected BinaryOp(Mul), got {:?}", other),
                }
            }
            other => panic!("expected BinaryOp(Add), got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Method Chain
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn method_chain() {
        // a.b.c — should parse as (a.b).c
        let expr = parse_expr("a.b.c");
        match &expr.kind {
            ExprKind::FieldAccess { object, field } => {
                assert_eq!(field, "c");
                match &object.kind {
                    ExprKind::FieldAccess {
                        object: inner,
                        field: f2,
                    } => {
                        assert_eq!(f2, "b");
                        match &inner.kind {
                            ExprKind::Identifier(name) => assert_eq!(name, "a"),
                            other => panic!("expected Identifier(a), got {:?}", other),
                        }
                    }
                    other => panic!("expected FieldAccess(b), got {:?}", other),
                }
            }
            other => panic!("expected FieldAccess(c), got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Method Call With Block
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn method_call_with_block() {
        let expr = parse_expr("items.each { |x| x }");
        match &expr.kind {
            ExprKind::MethodCall {
                object,
                method,
                generic_args,
                args,
                block,
            } => {
                assert_eq!(method, "each");
                assert!(generic_args.is_empty());
                match &object.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "items"),
                    other => panic!("expected Identifier(items), got {:?}", other),
                }
                assert!(args.is_empty());
                assert!(block.is_some());
                match &block.as_ref().unwrap().kind {
                    ExprKind::Closure(c) => {
                        assert_eq!(c.params.len(), 1);
                        assert_eq!(c.params[0].name, "x");
                    }
                    other => panic!("expected Closure, got {:?}", other),
                }
            }
            other => panic!("expected MethodCall, got {:?}", other),
        }
    }

    #[test]
    fn reserved_keyword_method_name_after_dot() {
        let expr = parse_expr("Thread.spawn({ || 42 })");
        match &expr.kind {
            ExprKind::MethodCall {
                object,
                method,
                generic_args,
                args,
                block,
            } => {
                assert_eq!(method, "spawn");
                assert!(generic_args.is_empty());
                match &object.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "Thread"),
                    other => panic!("expected Identifier(Thread), got {:?}", other),
                }
                assert_eq!(args.len(), 1);
                assert!(block.is_none());
            }
            other => panic!("expected MethodCall, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Safe Navigation
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn safe_navigation() {
        let expr = parse_expr("user?.name");
        match &expr.kind {
            ExprKind::SafeNav { object, field } => {
                assert_eq!(field, "name");
                match &object.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "user"),
                    other => panic!("expected Identifier(user), got {:?}", other),
                }
            }
            other => panic!("expected SafeNav, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Try Operator
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn try_operator() {
        let expr = parse_expr("file.read?");
        match &expr.kind {
            ExprKind::Try(inner) => match &inner.kind {
                ExprKind::FieldAccess { object, field } => {
                    assert_eq!(field, "read");
                    match &object.kind {
                        ExprKind::Identifier(name) => assert_eq!(name, "file"),
                        other => panic!("expected Identifier(file), got {:?}", other),
                    }
                }
                other => panic!("expected FieldAccess, got {:?}", other),
            },
            other => panic!("expected Try, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Closure
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn closure_do_end() {
        let expr = parse_expr("do |x|\n      x + 1\n    end");
        match &expr.kind {
            ExprKind::Closure(c) => {
                assert!(!c.is_async);
                assert!(!c.is_move);
                assert_eq!(c.params.len(), 1);
                assert_eq!(c.params[0].name, "x");
                match &c.body {
                    ClosureBody::Block(block) => {
                        assert_eq!(block.statements.len(), 1);
                    }
                    other => panic!("expected Block closure body, got {:?}", other),
                }
            }
            other => panic!("expected Closure, got {:?}", other),
        }
    }

    #[test]
    fn async_do_closure() {
        let expr = parse_expr("async do |x|\n      x + 1\n    end");
        match &expr.kind {
            ExprKind::Closure(c) => {
                assert!(c.is_async);
                assert!(!c.is_move);
                assert_eq!(c.params.len(), 1);
            }
            other => panic!("expected Closure, got {:?}", other),
        }
    }

    #[test]
    fn async_move_brace_closure() {
        let expr = parse_expr("async move { |x| x }");
        match &expr.kind {
            ExprKind::Closure(c) => {
                assert!(c.is_async);
                assert!(c.is_move);
                assert_eq!(c.params.len(), 1);
            }
            other => panic!("expected Closure, got {:?}", other),
        }
    }

    #[test]
    fn await_prefix_expr_rejected() {
        let diags = parse_err("def _test_\n  await fetch_user(42)\nend");
        assert!(
            diags
                .iter()
                .any(|diag| diag.message.contains("postfix `.await`")),
            "expected postfix-await guidance, got {:?}",
            diags
        );
    }

    #[test]
    fn await_postfix_expr() {
        let expr = parse_expr("fetch_user(42).await");
        match expr.kind {
            ExprKind::Await(inner) => match inner.kind {
                ExprKind::Call { .. } => {}
                other => panic!("expected call inside await, got {:?}", other),
            },
            other => panic!("expected await, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Range
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn range_exclusive() {
        let expr = parse_expr("0..10");
        match &expr.kind {
            ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                assert!(!inclusive);
                assert!(start.is_some());
                assert!(end.is_some());
                match &start.as_ref().unwrap().kind {
                    ExprKind::IntLiteral(0, _) => {}
                    other => panic!("expected 0, got {:?}", other),
                }
                match &end.as_ref().unwrap().kind {
                    ExprKind::IntLiteral(10, _) => {}
                    other => panic!("expected 10, got {:?}", other),
                }
            }
            other => panic!("expected Range, got {:?}", other),
        }
    }

    #[test]
    fn range_inclusive() {
        let expr = parse_expr("0..=10");
        match &expr.kind {
            ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                assert!(*inclusive);
                assert!(start.is_some());
                assert!(end.is_some());
            }
            other => panic!("expected Range, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Array Literal
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn array_literal() {
        let expr = parse_expr("[1, 2, 3]");
        match &expr.kind {
            ExprKind::ArrayLiteral(elems) => {
                assert_eq!(elems.len(), 3);
                match &elems[0].kind {
                    ExprKind::IntLiteral(1, _) => {}
                    other => panic!("expected 1, got {:?}", other),
                }
            }
            other => panic!("expected ArrayLiteral, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Tuple
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn tuple_literal() {
        let expr = parse_expr("(a, b, c)");
        match &expr.kind {
            ExprKind::TupleLiteral(elems) => {
                assert_eq!(elems.len(), 3);
                match &elems[0].kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "a"),
                    other => panic!("expected Identifier(a), got {:?}", other),
                }
            }
            other => panic!("expected TupleLiteral, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Control Flow — if/elsif/else
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn if_elsif_else() {
        let expr = parse_expr("if x\n    1\n  elsif y\n    2\n  else\n    3\n  end");
        match &expr.kind {
            ExprKind::If(if_expr) => {
                match &if_expr.condition.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "x"),
                    other => panic!("expected Identifier(x), got {:?}", other),
                }
                assert_eq!(if_expr.then_body.statements.len(), 1);
                assert_eq!(if_expr.elsif_clauses.len(), 1);
                match &if_expr.elsif_clauses[0].condition.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "y"),
                    other => panic!("expected Identifier(y), got {:?}", other),
                }
                assert!(if_expr.else_body.is_some());
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Control Flow — match
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn match_with_multiple_arms() {
        let expr = parse_expr("match x\n    1 -> true\n    2 -> false\n    _ -> false\n  end");
        match &expr.kind {
            ExprKind::Match(m) => {
                match &m.subject.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "x"),
                    other => panic!("expected Identifier(x), got {:?}", other),
                }
                assert_eq!(m.arms.len(), 3);
                // First arm: pattern is literal 1
                match &m.arms[0].pattern {
                    Pattern::Literal { expr, .. } => {
                        matches!(&expr.kind, ExprKind::IntLiteral(1, _));
                    }
                    other => panic!("expected literal pattern, got {:?}", other),
                }
                // Last arm: wildcard
                assert!(matches!(&m.arms[2].pattern, Pattern::Wildcard { .. }));
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Control Flow — for loop
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn for_loop() {
        let expr = parse_expr("for i in items\n    i\n  end");
        match &expr.kind {
            ExprKind::For(f) => {
                match &f.pattern {
                    Pattern::Identifier { name, .. } => assert_eq!(name, "i"),
                    other => panic!("expected identifier pattern, got {:?}", other),
                }
                match &f.iterable.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "items"),
                    other => panic!("expected Identifier(items), got {:?}", other),
                }
                assert_eq!(f.body.statements.len(), 1);
            }
            other => panic!("expected For, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Control Flow — while loop
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn while_loop() {
        let expr = parse_expr("while x\n    x\n  end");
        match &expr.kind {
            ExprKind::While(w) => {
                match &w.condition.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "x"),
                    other => panic!("expected Identifier(x), got {:?}", other),
                }
                assert_eq!(w.body.statements.len(), 1);
            }
            other => panic!("expected While, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Control Flow — loop
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn loop_expr() {
        let expr = parse_expr("loop\n    break\n  end");
        match &expr.kind {
            ExprKind::Loop(l) => {
                assert_eq!(l.body.statements.len(), 1);
            }
            other => panic!("expected Loop, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Patterns
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn pattern_wildcard() {
        let expr = parse_expr("match x\n    _ -> 0\n  end");
        match &expr.kind {
            ExprKind::Match(m) => {
                assert!(matches!(&m.arms[0].pattern, Pattern::Wildcard { .. }));
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn pattern_or() {
        let expr = parse_expr("match x\n    1 | 2 | 3 -> true\n  end");
        match &expr.kind {
            ExprKind::Match(m) => match &m.arms[0].pattern {
                Pattern::Or { patterns, .. } => {
                    assert_eq!(patterns.len(), 3);
                    assert!(matches!(&patterns[0], Pattern::Literal { .. }));
                }
                other => panic!("expected Or pattern, got {:?}", other),
            },
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn pattern_enum_variant() {
        let expr = parse_expr("match x\n    Status.Pending -> 0\n  end");
        match &expr.kind {
            ExprKind::Match(m) => match &m.arms[0].pattern {
                Pattern::Enum {
                    path,
                    variant,
                    fields,
                    ..
                } => {
                    assert_eq!(path, &vec!["Status".to_string()]);
                    assert_eq!(variant, "Pending");
                    assert!(fields.is_empty());
                }
                other => panic!("expected Enum pattern, got {:?}", other),
            },
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn pattern_ref() {
        let expr = parse_expr("match x\n    ref y -> y\n  end");
        match &expr.kind {
            ExprKind::Match(m) => match &m.arms[0].pattern {
                Pattern::Ref { mutable, name, .. } => {
                    assert!(!mutable);
                    assert_eq!(name, "y");
                }
                other => panic!("expected Ref pattern, got {:?}", other),
            },
            other => panic!("expected Match, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Additional edge cases
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn empty_program() {
        let program = parse("");
        assert!(program.items.is_empty());
    }

    #[test]
    fn bool_literal() {
        let expr = parse_expr("true");
        assert!(matches!(&expr.kind, ExprKind::BoolLiteral(true)));
    }

    #[test]
    fn string_literal() {
        let expr = parse_expr("\"hello\"");
        match &expr.kind {
            ExprKind::StringLiteral(s) => assert_eq!(s, "hello"),
            other => panic!("expected StringLiteral, got {:?}", other),
        }
    }

    #[test]
    fn unary_negation() {
        let expr = parse_expr("-42");
        match &expr.kind {
            ExprKind::UnaryOp { op, operand } => {
                assert_eq!(*op, UnaryOp::Neg);
                match &operand.kind {
                    ExprKind::IntLiteral(42, _) => {}
                    other => panic!("expected 42, got {:?}", other),
                }
            }
            other => panic!("expected UnaryOp(Neg), got {:?}", other),
        }
    }

    #[test]
    fn return_expression() {
        let expr = parse_expr("return 42");
        match &expr.kind {
            ExprKind::Return(Some(val)) => match &val.kind {
                ExprKind::IntLiteral(42, _) => {}
                other => panic!("expected 42, got {:?}", other),
            },
            other => panic!("expected Return, got {:?}", other),
        }
    }

    #[test]
    fn struct_def() {
        let src = "\
struct Point
  x: Int
  y: Int
end";
        let program = parse(src);
        let s = match &program.items[0] {
            TopLevelItem::Struct(s) => s,
            other => panic!("expected struct, got {:?}", other),
        };
        assert_eq!(s.name, "Point");
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.fields[0].name, "x");
        assert_eq!(s.fields[1].name, "y");
    }

    /// Regression: `impl`/`def` inside a struct body must produce a
    /// bounded diagnostic, not infinite-loop while pushing placeholder
    /// FieldDecls until OOM (≈1.25 GiB observed before fix). The
    /// `expect_identifier` helper does NOT advance on a non-identifier
    /// token, so without explicit recovery in `parse_struct_def` the
    /// outer loop repeatedly invokes `parse_field_decl` on the same
    /// `Impl` / `Def` token.
    ///
    /// Use `class` if you want method bodies inline (Ruby-style); the
    /// `struct` keyword is intentionally fields-only.
    #[test]
    fn struct_with_impl_inside_errors_without_oom() {
        let src = "\
struct Money
  cents: Int

  impl Display
    def fmt
      Ok(())
    end
  end
end";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize().expect("lex");
        let mut parser = Parser::new(tokens);
        // 50ms is generous — well-formed input parses in <1ms; the
        // pre-fix bug ran until OOM (multiple seconds + GiBs of RAM).
        let start = std::time::Instant::now();
        let result = parser.parse();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "parse took {:?}; expected <50ms — guarding against the OOM-loop regression",
            elapsed
        );
        let diags = result.expect_err("expected parse to fail with diagnostics");
        let msg = diags
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            msg.contains("`impl` blocks are not allowed inside `struct` bodies"),
            "missing `impl` rejection diagnostic; got: {}",
            msg
        );
    }

    /// Regression: the universal `Parser::ensure_loop_progress` guard
    /// must bound any body-loop where the per-iteration parser fails
    /// to advance the cursor. Tests trait-body recovery here — without
    /// the guard, `parse_trait_item` calls `synchronize()` on a
    /// `class` keyword (itself a sync point) which is a no-op, and
    /// the outer body-loop spins. The guard's `error + advance`
    /// fallback bounds the parse.
    #[test]
    fn trait_with_rogue_token_inside_errors_without_oom() {
        let src = "\
trait MyTrait
  class Bogus
end";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize().expect("lex");
        let mut parser = Parser::new(tokens);
        let start = std::time::Instant::now();
        let result = parser.parse();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "parse took {:?}; expected <50ms — guarding against the OOM-loop regression",
            elapsed
        );
        // Parse must have produced diagnostics. We don't pin the
        // exact message because either the trait-body recovery
        // diagnostic OR the universal guard diagnostic is an
        // acceptable signal — the invariant is bounded time +
        // non-empty diagnostic set.
        let diags = result.expect_err("expected parse to fail with diagnostics");
        assert!(
            !diags.is_empty(),
            "expected at least one diagnostic from guard or parse_trait_item recovery"
        );
    }

    /// Regression: `def` (non-impl) inside a struct body must also
    /// produce a bounded diagnostic. Same root cause as the
    /// `impl`-inside-struct case — the outer loop must not blindly
    /// retry `parse_field_decl` on a non-identifier keyword.
    #[test]
    fn struct_with_def_inside_errors_without_oom() {
        let src = "\
struct Money
  cents: Int

  def total
    self.cents
  end
end";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize().expect("lex");
        let mut parser = Parser::new(tokens);
        let start = std::time::Instant::now();
        let result = parser.parse();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "parse took {:?}; expected <50ms — guarding against the OOM-loop regression",
            elapsed
        );
        let diags = result.expect_err("expected parse to fail with diagnostics");
        let msg = diags
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            msg.contains("method definitions are not allowed inside `struct` bodies"),
            "missing `def` rejection diagnostic; got: {}",
            msg
        );
    }

    #[test]
    fn use_simple() {
        let program = parse("use Collections.Vec");
        let u = match &program.items[0] {
            TopLevelItem::Use(u) => u,
            other => panic!("expected use, got {:?}", other),
        };
        assert_eq!(u.path, vec!["Collections", "Vec"]);
        assert!(matches!(u.kind, UseKind::Simple));
    }

    #[test]
    fn use_simple_lowercase_std_path() {
        let program = parse("use std.io");
        let u = match &program.items[0] {
            TopLevelItem::Use(u) => u,
            other => panic!("expected use, got {:?}", other),
        };
        assert_eq!(u.path, vec!["std", "io"]);
        assert!(matches!(u.kind, UseKind::Simple));
    }

    #[test]
    fn use_group_mixed_case_segments() {
        let program = parse("use std.io.{read_line, Stdin}");
        let u = match &program.items[0] {
            TopLevelItem::Use(u) => u,
            other => panic!("expected use, got {:?}", other),
        };
        assert_eq!(u.path, vec!["std", "io"]);
        match &u.kind {
            UseKind::Group(names) => assert_eq!(names, &vec!["read_line", "Stdin"]),
            other => panic!("expected group import, got {:?}", other),
        }
    }

    #[test]
    fn method_call_with_args() {
        let expr = parse_expr("list.push(42)");
        match &expr.kind {
            ExprKind::MethodCall {
                object,
                method,
                generic_args,
                args,
                block,
            } => {
                assert_eq!(method, "push");
                assert!(generic_args.is_empty());
                assert_eq!(args.len(), 1);
                assert!(block.is_none());
                match &object.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "list"),
                    other => panic!("expected Identifier(list), got {:?}", other),
                }
            }
            other => panic!("expected MethodCall, got {:?}", other),
        }
    }

    #[test]
    fn method_call_with_generic_args() {
        let expr = parse_expr("v.iter.collect[Vec[Int]]");
        match &expr.kind {
            ExprKind::MethodCall {
                object,
                method,
                generic_args,
                args,
                block,
            } => {
                assert_eq!(method, "collect");
                assert_eq!(generic_args.len(), 1);
                assert!(args.is_empty());
                assert!(block.is_none());
                match &object.kind {
                    ExprKind::FieldAccess { field, .. } => assert_eq!(field, "iter"),
                    other => panic!("expected FieldAccess(iter), got {:?}", other),
                }
            }
            other => panic!("expected MethodCall, got {:?}", other),
        }
    }

    #[test]
    fn function_call() {
        let expr = parse_expr("foo(1, 2)");
        match &expr.kind {
            ExprKind::Call { callee, args, .. } => {
                match &callee.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "foo"),
                    other => panic!("expected Identifier(foo), got {:?}", other),
                }
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected Call, got {:?}", other),
        }
    }

    #[test]
    fn index_expression() {
        let expr = parse_expr("arr[0]");
        match &expr.kind {
            ExprKind::Index { object, index } => {
                match &object.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "arr"),
                    other => panic!("expected Identifier(arr), got {:?}", other),
                }
                match &index.kind {
                    ExprKind::IntLiteral(0, _) => {}
                    other => panic!("expected 0, got {:?}", other),
                }
            }
            other => panic!("expected Index, got {:?}", other),
        }
    }

    #[test]
    fn assignment() {
        let expr = parse_expr("x = 5");
        match &expr.kind {
            ExprKind::Assign { target, value } => {
                match &target.kind {
                    ExprKind::Identifier(name) => assert_eq!(name, "x"),
                    other => panic!("expected Identifier(x), got {:?}", other),
                }
                match &value.kind {
                    ExprKind::IntLiteral(5, _) => {}
                    other => panic!("expected 5, got {:?}", other),
                }
            }
            other => panic!("expected Assign, got {:?}", other),
        }
    }
}
