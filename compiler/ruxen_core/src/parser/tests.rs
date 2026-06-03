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
        let stmt = parse_stmt("var y: Int = 0");
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
        let program = parse("def var set_name(name: String)\nend");
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
    fn async_func_parses() {
        // ruby-naming.spec.md removes the `pub` prefix; visibility is
        // controlled via section markers in declaring scopes. The flip
        // of the top-level default to `Public` lands in a follow-up; for
        // now this test only asserts that `async def` parses.
        let program = parse("async def fetch\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
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

    // ─── Section markers (ruby-naming.spec.md §3.2) ──────────────────
    //
    // Inside a `class` / `struct` / `module` / `mixin` body, bare
    // `public` / `private` / `protected` lines act as section markers
    // that switch the visibility of every subsequent declaration until
    // the next marker. Public is the default.

    #[test]
    fn class_section_markers_set_visibility() {
        let src = "\
class Foo
  pub_field: Int

  def pub_method
  end

  private

  priv_field: Int

  def priv_method
  end

  protected

  def proto_method
  end

  public

  def back_to_pub
  end
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };

        // Fields: pub_field is Public (default), priv_field is Private.
        let by_field = |name: &str| -> Visibility {
            class
                .fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("missing field {name}"))
                .visibility
        };
        assert_eq!(by_field("pub_field"), Visibility::Public);
        assert_eq!(by_field("priv_field"), Visibility::Private);

        // Methods: each picks up the section visibility in effect.
        let by_method = |name: &str| -> Visibility {
            class
                .methods
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("missing method {name}"))
                .visibility
        };
        assert_eq!(by_method("pub_method"), Visibility::Public);
        assert_eq!(by_method("priv_method"), Visibility::Private);
        assert_eq!(by_method("proto_method"), Visibility::Protected);
        assert_eq!(by_method("back_to_pub"), Visibility::Public);
    }

    #[test]
    fn class_public_name_list_overrides_private_section() {
        // ruby-naming.spec.md §3.2: the Ruby-style `public :name_a`
        // re-marks an already-declared method, overriding the section
        // marker that was in effect when it was defined. This is the
        // spec-sanctioned way to override a section without inventing
        // a prefix form.
        let src = "\
class Bar
  private

  def helper
  end

  def force_public
  end

  public :force_public
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };
        let by_method = |name: &str| -> Visibility {
            class
                .methods
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("missing method {name}"))
                .visibility
        };
        assert_eq!(by_method("helper"), Visibility::Private);
        assert_eq!(by_method("force_public"), Visibility::Public);
    }

    #[test]
    fn class_private_name_list_overrides_section() {
        // Ruby-style `private :a, :b` re-marks already-declared methods
        // after the body is parsed, overriding any section marker they
        // were under (ruby-naming.spec.md §3.2 trailing paragraph).
        let src = "\
class User
  def helper_a
  end

  def helper_b
  end

  def public_thing
  end

  private :helper_a, :helper_b
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };
        let by_method = |name: &str| -> Visibility {
            class
                .methods
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("missing method {name}"))
                .visibility
        };
        assert_eq!(by_method("helper_a"), Visibility::Private);
        assert_eq!(by_method("helper_b"), Visibility::Private);
        assert_eq!(by_method("public_thing"), Visibility::Public);
    }

    #[test]
    fn struct_section_markers_set_field_visibility() {
        let src = "\
struct User
  name: String
  email: String

  private

  audit_id: Int
  audit_log: String
end";
        let program = parse(src);
        let s = match &program.items[0] {
            TopLevelItem::Struct(s) => s,
            other => panic!("expected struct, got {:?}", other),
        };
        let by_field = |name: &str| -> Visibility {
            s.fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("missing field {name}"))
                .visibility
        };
        assert_eq!(by_field("name"), Visibility::Public);
        assert_eq!(by_field("email"), Visibility::Public);
        assert_eq!(by_field("audit_id"), Visibility::Private);
        assert_eq!(by_field("audit_log"), Visibility::Private);
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
    //  Mixins (ruby-naming.spec.md §3.4 — `mixin` replaces `trait`)
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn mixin_with_method_signature() {
        let src = "\
mixin Greetable
  def greet -> String
end";
        let program = parse(src);
        let tr = match &program.items[0] {
            TopLevelItem::Mixin(t) => t,
            other => panic!("expected mixin, got {:?}", other),
        };
        assert_eq!(tr.name, "Greetable");
        assert_eq!(tr.items.len(), 1);
        match &tr.items[0] {
            MixinItem::MethodSig(sig) => {
                assert_eq!(sig.name, "greet");
                assert!(sig.return_type.is_some());
            }
            other => panic!("expected method signature, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Mixin Inclusion via `include` (ruby-naming.spec.md §3.4)
    //
    //  The legacy top-level `impl Trait for Type ... end` block is gone.
    //  Spec §10a maps it to an `include Trait` directive inside the
    //  type's body, with the methods scattered alongside. The class's
    //  `inner_impls` records the inclusion; the methods live on
    //  `class.methods` like any other.
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn include_directive_in_class_body() {
        let src = "\
class Person
  include Greetable

  def greet -> String
  end
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };
        assert_eq!(class.inner_impls.len(), 1);
        let inc = &class.inner_impls[0];
        assert_eq!(inc.trait_name.segments, vec!["Greetable"]);
        assert!(!inc.is_unsafe);
        assert!(!inc.negative_trait);
        assert_eq!(class.methods.len(), 1);
        assert_eq!(class.methods[0].name, "greet");
    }

    #[test]
    fn class_inherent_methods_no_include() {
        // ruby-naming.spec.md §10a: a legacy `impl Person ... end`
        // inherent block lowers to methods directly inside the class.
        let src = "\
class Person
  def hello -> String
  end
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };
        assert!(class.inner_impls.is_empty());
        assert_eq!(class.methods.len(), 1);
        assert_eq!(class.methods[0].name, "hello");
    }

    #[test]
    fn unsafe_include_in_class_body() {
        // Legacy `unsafe impl Send for Buffer` migrates to an
        // `unsafe include Send` directive in the type's body.
        let src = "\
class Buffer
  unsafe include Send
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };
        assert_eq!(class.inner_impls.len(), 1);
        let inc = &class.inner_impls[0];
        assert!(inc.is_unsafe);
        assert!(!inc.negative_trait);
        assert_eq!(inc.trait_name.segments, vec!["Send"]);
    }

    #[test]
    fn negative_include_in_class_body() {
        // Legacy `impl !Sync for Buffer` migrates to a `include !Sync`
        // opt-out directive in the type's body.
        let src = "\
class Buffer
  include !Sync
end";
        let program = parse(src);
        let class = match &program.items[0] {
            TopLevelItem::Class(c) => c,
            other => panic!("expected class, got {:?}", other),
        };
        assert_eq!(class.inner_impls.len(), 1);
        let inc = &class.inner_impls[0];
        assert!(!inc.is_unsafe);
        assert!(inc.negative_trait);
        assert_eq!(inc.trait_name.segments, vec!["Sync"]);
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
    //  Async sub-phase 1 (docs/specs/stdlib/async.spec.md B7–B9)
    //  — surface parses; no lowering yet.
    // ═══════════════════════════════════════════════════════════════════

    /// B7: `async def foo` parses with `is_async` set. Sub-phase 1 does
    /// NOT lower the body to a state machine — the function still
    /// type-checks as if it were synchronous, only the `is_async`
    /// flag is recorded. Sub-phase 2 lifts the return to `some Future`.
    #[test]
    fn async_def_parses_subphase1_no_lowering() {
        let program = parse("async def fetch(url: Int) -> Int\n  url\nend");
        let func = match &program.items[0] {
            TopLevelItem::Function(f) => f,
            other => panic!("expected function, got {:?}", other),
        };
        assert!(func.is_async, "is_async flag must be set on the FuncDef");
        assert_eq!(func.name, "fetch");
        // Sub-phase 1: return type is preserved AS WRITTEN; no
        // `some Future` lift, no state-machine return shape.
        match &func.return_type {
            Some(TypeExpr::Named(path)) => assert_eq!(path.segments, vec!["Int"]),
            other => panic!("expected named return type `Int`, got {:?}", other),
        }
    }

    /// B8: `async { 42 }` parses as an async (closure) block. The
    /// async block lowers to an empty-param-list async closure
    /// expression; sub-phase 1 records `is_async` on the closure but
    /// otherwise leaves the body untouched (it executes synchronously
    /// at runtime). Sub-phase 2 lifts this to a `some Future`.
    #[test]
    fn async_block_parses_subphase1_no_lowering() {
        let expr = parse_expr("async { 42 }");
        match expr.kind {
            ExprKind::Closure(c) => {
                assert!(c.is_async, "is_async must be set on the closure");
                assert!(
                    c.params.is_empty(),
                    "async block has no explicit params; got {:?}",
                    c.params
                );
            }
            other => panic!("expected closure, got {:?}", other),
        }
    }

    /// B9: `expr.await` parses as a postfix `Await` AST node. In
    /// sub-phase 1 the lowering elides — the resolver wires it
    /// through as a method call against the expression's type
    /// (effectively a no-op for the synchronous bridge). Sub-phase 2
    /// rewrites the desugaring into a `match self.poll(cx) { Ready(v)
    /// -> v, Pending -> return Pending }` suspension point.
    #[test]
    fn dot_await_parses_subphase1_elides_to_value() {
        let expr = parse_expr("some_future.await");
        match expr.kind {
            ExprKind::Await(inner) => match inner.kind {
                ExprKind::Identifier(ref name) => assert_eq!(name, "some_future"),
                other => panic!("expected identifier under .await, got {:?}", other),
            },
            other => panic!("expected Await AST node, got {:?}", other),
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Expressions — Range
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn range_exclusive() {
        // ruby-naming.spec.md §3.10b: `...` is the EXCLUSIVE range.
        let expr = parse_expr("0...10");
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
        // ruby-naming.spec.md §3.10b: `..` is the INCLUSIVE range.
        let expr = parse_expr("0..10");
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
    fn struct_with_legacy_impl_inside_errors_without_oom() {
        // ruby-naming.spec.md: `impl` is no longer a keyword (legacy form
        // — `include Mixin` is the replacement). A legacy `impl Display`
        // block inside a struct body now lexes `impl` as an identifier and
        // mis-parses, but the parse must still terminate in bounded time
        // with diagnostics — never enter the OOM-loop the original guard
        // was added for.
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
        let start = std::time::Instant::now();
        let result = parser.parse();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "parse took {:?}; expected <50ms — guarding against the OOM-loop regression",
            elapsed
        );
        let diags = result.expect_err("expected parse to fail with diagnostics");
        assert!(
            !diags.is_empty(),
            "expected at least one diagnostic from struct-body recovery"
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

    /// Post ruby-naming.spec.md §3.4a: `def` IS allowed inside a struct
    /// body — structs accept inline methods the same way classes do. The
    /// old "reject + bounded diagnostic" guard now becomes a positive
    /// test: a struct with a `def` parses, the method is collected onto
    /// `StructDef::methods`, and parsing terminates promptly.
    #[test]
    fn struct_with_inline_def_is_accepted() {
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
        let program = parser.parse().expect("struct with inline def must parse");
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "parse took {:?}; expected <50ms — guarding against the OOM-loop regression",
            elapsed
        );
        let s = match &program.items[0] {
            TopLevelItem::Struct(s) => s,
            other => panic!("expected struct, got {:?}", other),
        };
        assert_eq!(s.methods.len(), 1);
        assert_eq!(s.methods[0].name, "total");
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

    // ── std.regex (Phase 4) ────────────────────────────────────────

    /// `/foo/i` parses as a `RegexLiteral` atom.
    #[test]
    fn parse_regex_literal_atom() {
        let expr = parse_expr("/foo/i");
        match expr.kind {
            ExprKind::RegexLiteral { pattern, flags } => {
                assert_eq!(pattern, "foo");
                assert_eq!(flags, "i");
            }
            other => panic!("expected RegexLiteral, got {:?}", other),
        }
    }

    /// `s ~= /error/` parses as `BinaryOp { op: MatchOp, .. }`.
    #[test]
    fn parse_regex_tilde_eq_binop() {
        let expr = parse_expr(r#"s ~= /error/"#);
        match expr.kind {
            ExprKind::BinaryOp { op, left, right } => {
                assert_eq!(op, BinOp::MatchOp);
                match left.kind {
                    ExprKind::Identifier(ref n) => assert_eq!(n, "s"),
                    other => panic!("expected Identifier(s) on LHS, got {:?}", other),
                }
                match right.kind {
                    ExprKind::RegexLiteral { ref pattern, .. } => assert_eq!(pattern, "error"),
                    other => panic!("expected RegexLiteral on RHS, got {:?}", other),
                }
            }
            other => panic!("expected BinaryOp(MatchOp), got {:?}", other),
        }
    }

    /// `~=` is at the same precedence as `==`/`!=`. `a == b ~= /x/`
    /// must parse as `(a == b) ~= /x/` (left-associative within the
    /// equality tier).
    #[test]
    fn parse_regex_tilde_eq_same_precedence_as_equality() {
        let expr = parse_expr(r#"a == b ~= /x/"#);
        // Outermost should be MatchOp with LHS = (a == b)
        match expr.kind {
            ExprKind::BinaryOp { op, left, .. } => {
                assert_eq!(op, BinOp::MatchOp);
                match left.kind {
                    ExprKind::BinaryOp { op, .. } => assert_eq!(op, BinOp::Eq),
                    other => panic!("expected nested BinaryOp(Eq) on LHS, got {:?}", other),
                }
            }
            other => panic!("expected BinaryOp(MatchOp) at top, got {:?}", other),
        }
    }

    /// ruby-naming.spec.md §3.10: `()` is not Ruxen syntax — the unit
    /// type and value are both spelled `nil`. The parser rejects `()`
    /// in both positions with a fix-it pointing at `nil`.
    fn parse_errors(input: &str) -> Vec<crate::diagnostics::Diagnostic> {
        let mut lexer = Lexer::new(input);
        let tokens = lexer.tokenize().expect("lexer failed");
        let mut parser = Parser::new(tokens);
        match parser.parse() {
            Ok(_) => vec![],
            Err(diags) => diags,
        }
    }

    #[test]
    fn unit_paren_type_is_rejected_use_nil() {
        let diags = parse_errors("def f(x: Int) -> ()\n  x\nend\n");
        assert!(
            diags.iter().any(|d| d.message.contains("use `nil`")),
            "`-> ()` should be rejected with a `nil` fix-it; got: {diags:?}"
        );
    }

    #[test]
    fn unit_paren_value_is_rejected_use_nil() {
        let diags = parse_errors("def f\n  let x = ()\n  x\nend\n");
        assert!(
            diags.iter().any(|d| d.message.contains("nil")),
            "`()` value should be rejected with a `nil` fix-it; got: {diags:?}"
        );
    }
}
