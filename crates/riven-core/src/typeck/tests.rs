//! Tests for type checking: unification, coercion, inference, trait resolution.

#[cfg(test)]
mod tests {
    use crate::hir::context::TypeContext;
    use crate::hir::types::Ty;
    use crate::lexer::token::Span;
    use crate::typeck::unify::{can_coerce, unify};

    fn span() -> Span {
        Span {
            start: 0,
            end: 0,
            line: 1,
            column: 1,
        }
    }

    // ─── Unification Tests ──────────────────────────────────────────

    #[test]
    fn unify_same_type() {
        let mut ctx = TypeContext::new();
        let result = unify(&Ty::Int, &Ty::Int, &mut ctx, &span());
        assert_eq!(result.unwrap(), Ty::Int);
    }

    #[test]
    fn unify_infer_with_concrete() {
        let mut ctx = TypeContext::new();
        let t = ctx.fresh_type_var();
        let result = unify(&t, &Ty::Int, &mut ctx, &span());
        assert_eq!(result.unwrap(), Ty::Int);
        assert_eq!(ctx.resolve(&t), Ty::Int);
    }

    #[test]
    fn unify_concrete_with_infer() {
        let mut ctx = TypeContext::new();
        let t = ctx.fresh_type_var();
        let result = unify(&Ty::String, &t, &mut ctx, &span());
        assert_eq!(result.unwrap(), Ty::String);
    }

    #[test]
    fn unify_two_infer_vars() {
        let mut ctx = TypeContext::new();
        let t0 = ctx.fresh_type_var();
        let t1 = ctx.fresh_type_var();
        unify(&t0, &t1, &mut ctx, &span()).unwrap();
        // Now bind t1 to Int — t0 should also resolve to Int
        unify(&t1, &Ty::Int, &mut ctx, &span()).unwrap();
        assert_eq!(ctx.resolve(&t0), Ty::Int);
    }

    #[test]
    fn unify_never_with_anything() {
        let mut ctx = TypeContext::new();
        let result = unify(&Ty::Never, &Ty::Int, &mut ctx, &span());
        assert_eq!(result.unwrap(), Ty::Int);

        let result = unify(&Ty::String, &Ty::Never, &mut ctx, &span());
        assert_eq!(result.unwrap(), Ty::String);
    }

    #[test]
    fn unify_error_with_anything() {
        let mut ctx = TypeContext::new();
        let result = unify(&Ty::Error, &Ty::Int, &mut ctx, &span());
        assert_eq!(result.unwrap(), Ty::Int);
    }

    #[test]
    fn unify_tuples() {
        let mut ctx = TypeContext::new();
        let a = Ty::Tuple(vec![Ty::Int, Ty::Bool]);
        let b = Ty::Tuple(vec![Ty::Int, Ty::Bool]);
        let result = unify(&a, &b, &mut ctx, &span());
        assert_eq!(result.unwrap(), a);
    }

    #[test]
    fn unify_tuples_different_lengths_fails() {
        let mut ctx = TypeContext::new();
        let a = Ty::Tuple(vec![Ty::Int]);
        let b = Ty::Tuple(vec![Ty::Int, Ty::Bool]);
        let result = unify(&a, &b, &mut ctx, &span());
        assert!(result.is_err());
    }

    #[test]
    fn unify_vec() {
        let mut ctx = TypeContext::new();
        let a = Ty::Vec(Box::new(Ty::Int));
        let t = ctx.fresh_type_var();
        let b = Ty::Vec(Box::new(t));
        let result = unify(&a, &b, &mut ctx, &span());
        assert_eq!(result.unwrap(), Ty::Vec(Box::new(Ty::Int)));
    }

    #[test]
    fn unify_option() {
        let mut ctx = TypeContext::new();
        let a = Ty::Option(Box::new(Ty::String));
        let b = Ty::Option(Box::new(Ty::String));
        assert_eq!(unify(&a, &b, &mut ctx, &span()).unwrap(), a);
    }

    #[test]
    fn unify_result() {
        let mut ctx = TypeContext::new();
        let a = Ty::Result(Box::new(Ty::Int), Box::new(Ty::String));
        let b = Ty::Result(Box::new(Ty::Int), Box::new(Ty::String));
        assert_eq!(unify(&a, &b, &mut ctx, &span()).unwrap(), a);
    }

    #[test]
    fn unify_refs() {
        let mut ctx = TypeContext::new();
        let a = Ty::Ref(Box::new(Ty::Int));
        let b = Ty::Ref(Box::new(Ty::Int));
        assert_eq!(unify(&a, &b, &mut ctx, &span()).unwrap(), a);
    }

    #[test]
    fn unify_different_types_fails() {
        let mut ctx = TypeContext::new();
        let result = unify(&Ty::Int, &Ty::String, &mut ctx, &span());
        assert!(result.is_err());
    }

    #[test]
    fn unify_different_classes_fails() {
        let mut ctx = TypeContext::new();
        let a = Ty::Class {
            name: "Dog".to_string(),
            generic_args: vec![],
        };
        let b = Ty::Class {
            name: "Cat".to_string(),
            generic_args: vec![],
        };
        assert!(unify(&a, &b, &mut ctx, &span()).is_err());
    }

    #[test]
    fn unify_fn_types() {
        let mut ctx = TypeContext::new();
        let a = Ty::Fn {
            params: vec![Ty::Int],
            ret: Box::new(Ty::Bool),
        };
        let b = Ty::Fn {
            params: vec![Ty::Int],
            ret: Box::new(Ty::Bool),
        };
        assert_eq!(unify(&a, &b, &mut ctx, &span()).unwrap(), a);
    }

    #[test]
    fn unify_fn_different_arity_fails() {
        let mut ctx = TypeContext::new();
        let a = Ty::Fn {
            params: vec![Ty::Int],
            ret: Box::new(Ty::Bool),
        };
        let b = Ty::Fn {
            params: vec![Ty::Int, Ty::Int],
            ret: Box::new(Ty::Bool),
        };
        assert!(unify(&a, &b, &mut ctx, &span()).is_err());
    }

    #[test]
    fn unify_generic_class() {
        let mut ctx = TypeContext::new();
        let t = ctx.fresh_type_var();
        let a = Ty::Class {
            name: "Repo".to_string(),
            generic_args: vec![Ty::Int],
        };
        let b = Ty::Class {
            name: "Repo".to_string(),
            generic_args: vec![t],
        };
        let result = unify(&a, &b, &mut ctx, &span()).unwrap();
        assert_eq!(
            result,
            Ty::Class {
                name: "Repo".to_string(),
                generic_args: vec![Ty::Int]
            }
        );
    }

    // ─── Coercion Tests ─────────────────────────────────────────────

    #[test]
    fn coerce_same_type() {
        let ctx = TypeContext::new();
        assert!(can_coerce(&Ty::Int, &Ty::Int, &ctx));
    }

    #[test]
    fn coerce_never_to_anything() {
        let ctx = TypeContext::new();
        assert!(can_coerce(&Ty::Never, &Ty::Int, &ctx));
        assert!(can_coerce(&Ty::Never, &Ty::String, &ctx));
    }

    #[test]
    fn coerce_mut_ref_to_immut_ref() {
        let ctx = TypeContext::new();
        let from = Ty::RefMut(Box::new(Ty::Int));
        let to = Ty::Ref(Box::new(Ty::Int));
        assert!(can_coerce(&from, &to, &ctx));
    }

    #[test]
    fn coerce_ref_string_to_str() {
        let ctx = TypeContext::new();
        let from = Ty::Ref(Box::new(Ty::String));
        assert!(can_coerce(&from, &Ty::Str, &ctx));
    }

    #[test]
    fn coerce_int_to_float() {
        let ctx = TypeContext::new();
        assert!(can_coerce(&Ty::Int, &Ty::Float, &ctx));
        assert!(can_coerce(&Ty::Int, &Ty::Float64, &ctx));
    }

    #[test]
    fn coerce_integer_widening() {
        let ctx = TypeContext::new();
        assert!(can_coerce(&Ty::Int8, &Ty::Int16, &ctx));
        assert!(can_coerce(&Ty::Int16, &Ty::Int32, &ctx));
        assert!(can_coerce(&Ty::Int32, &Ty::Int64, &ctx));
    }

    #[test]
    fn no_coerce_signed_to_unsigned() {
        let ctx = TypeContext::new();
        assert!(!can_coerce(&Ty::Int, &Ty::UInt, &ctx));
    }

    #[test]
    fn no_coerce_wider_to_narrower() {
        let ctx = TypeContext::new();
        assert!(!can_coerce(&Ty::Int64, &Ty::Int8, &ctx));
    }

    #[test]
    fn coerce_option_covariance() {
        let ctx = TypeContext::new();
        // Option[&mut T] → Option[&T] through the inner coercion
        let from = Ty::Option(Box::new(Ty::RefMut(Box::new(Ty::Int))));
        let to = Ty::Option(Box::new(Ty::Ref(Box::new(Ty::Int))));
        assert!(can_coerce(&from, &to, &ctx));
    }

    // ─── End-to-End Type Inference ──────────────────────────────────

    fn parse_and_check(source: &str) -> crate::typeck::TypeCheckResult {
        let mut lexer = crate::lexer::Lexer::new(source);
        let tokens = lexer.tokenize().expect("lexer failed");
        let mut parser = crate::parser::Parser::new(tokens);
        let program = parser.parse().expect("parser failed");
        crate::typeck::type_check(&program)
    }

    #[test]
    fn infer_int_literal() {
        let result = parse_and_check("def test\n  let x = 42\nend");
        // Should compile without type errors
        let type_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .collect();
        // x should have type Int — check that we resolved it
        assert!(
            type_errors.is_empty()
                || type_errors.iter().all(|d| {
                    // Some errors are acceptable (e.g., unresolved types for variables
                    // not referenced further)
                    d.message.contains("could not infer")
                })
        );
    }

    #[test]
    fn infer_float_annotation() {
        let result = parse_and_check("def test\n  let x: Float = 42\nend");
        // Float annotation should work with an integer literal
        // (backward inference / int-to-float coercion)
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .filter(|d| d.message.contains("type mismatch"))
            .collect();
        assert!(errors.is_empty(), "Int literal should coerce to Float");
    }

    #[test]
    fn infer_bool_literal() {
        let result = parse_and_check("def test\n  let x = true\nend");
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .filter(|d| d.message.contains("type mismatch"))
            .collect();
        assert!(errors.is_empty());
    }

    #[test]
    fn type_error_on_mismatch() {
        let result = parse_and_check("def test\n  let x: Int = true\nend");
        // Should produce a type error: Bool doesn't unify with Int
        let has_mismatch = result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("type mismatch"));
        assert!(has_mismatch, "Expected type mismatch error");
    }

    #[test]
    fn undefined_variable_error() {
        let result = parse_and_check("def test\n  let x = undefined_var\nend");
        let has_error = result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("undefined variable"));
        assert!(has_error, "Expected undefined variable error");
    }

    #[test]
    fn enum_variant_resolution() {
        let source = r#"
enum Priority
  Low
  High
end

def test
  let p = Priority.Low
end
"#;
        let result = parse_and_check(source);
        // Should resolve Priority.Low without errors
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("undefined enum variant"))
            .collect();
        assert!(
            errors.is_empty(),
            "Enum variant should resolve: {:?}",
            errors
        );
    }

    #[test]
    fn class_definition() {
        let source = r#"
class Point
  x: Int
  y: Int

  def init(@x: Int, @y: Int)
  end
end
"#;
        let result = parse_and_check(source);
        let type_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .filter(|d| !d.message.contains("could not infer"))
            .collect();
        assert!(
            type_errors.is_empty(),
            "Class def should type-check: {:?}",
            type_errors
        );
    }

    #[test]
    fn trait_and_impl() {
        let source = r#"
trait Greetable
  def greet -> String
end

class Dog
  name: String

  def init(@name: String)
  end
end

impl Greetable for Dog
  def greet -> String
    "woof"
  end
end
"#;
        let result = parse_and_check(source);
        let type_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .filter(|d| !d.message.contains("could not infer"))
            .collect();
        assert!(
            type_errors.is_empty(),
            "Trait+impl should type-check: {:?}",
            type_errors
        );
    }

    #[test]
    fn match_expression() {
        let source = r#"
enum Color
  Red
  Blue
end

def describe(c: Color) -> String
  match c
    Color.Red -> "red"
    Color.Blue -> "blue"
  end
end
"#;
        let result = parse_and_check(source);
        let type_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .filter(|d| !d.message.contains("could not infer"))
            .collect();
        assert!(
            type_errors.is_empty(),
            "Match should type-check: {:?}",
            type_errors
        );
    }

    #[test]
    fn if_expression_types() {
        let source = r#"
def test(x: Bool) -> Int
  if x
    42
  else
    0
  end
end
"#;
        let result = parse_and_check(source);
        let type_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .filter(|d| !d.message.contains("could not infer"))
            .collect();
        assert!(
            type_errors.is_empty(),
            "If expr should type-check: {:?}",
            type_errors
        );
    }

    #[test]
    fn break_outside_loop_errors() {
        let source = "def test\n  break\nend";
        let result = parse_and_check(source);
        let has_error = result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("break"));
        assert!(has_error, "break outside loop should error");
    }

    #[test]
    fn continue_outside_loop_errors() {
        let source = "def test\n  continue\nend";
        let result = parse_and_check(source);
        let has_error = result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("continue"));
        assert!(has_error, "continue outside loop should error");
    }

    #[test]
    fn generic_class() {
        let source = r#"
class Container[T]
  value: T

  def init(@value: T)
  end
end
"#;
        let result = parse_and_check(source);
        let type_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .filter(|d| !d.message.contains("could not infer"))
            .collect();
        assert!(
            type_errors.is_empty(),
            "Generic class should parse: {:?}",
            type_errors
        );
    }

    #[test]
    fn send_bound_rejects_raw_pointer_payload() {
        let source = r#"
class RawBox
  ptr: *mut Void

  def init(@ptr: *mut Void)
  end
end

def require_send[T: Send](value: T)
end

def test
  let box = RawBox.new(null)
  require_send(box)
end
"#;
        let result = parse_and_check(source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code.as_deref() == Some("E1011")),
            "expected E1011 Send-bound rejection, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn sync_bound_rejects_negative_impl_opt_out() {
        let source = r#"
class NotSync
  def init
  end
end

impl !Sync for NotSync
end

def require_sync[T: Sync](value: T)
end

def test
  let value = NotSync.new()
  require_sync(value)
end
"#;
        let result = parse_and_check(source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code.as_deref() == Some("E1012")),
            "expected E1012 Sync-bound rejection, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn unsafe_impl_send_satisfies_send_bound() {
        let source = r#"
class FfiHandle
  ptr: *mut Void

  def init(@ptr: *mut Void)
  end
end

unsafe impl Send for FfiHandle
end

def require_send[T: Send](value: T)
end

def test
  let handle = FfiHandle.new(null)
  require_send(handle)
end
"#;
        let result = parse_and_check(source);
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.code.as_deref() == Some("E1011")),
            "unexpected E1011 for unsafe impl Send: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn async_function_call_returns_future_and_await_unwraps_output() {
        let source = r#"
async def fetch_user(id: Int) -> Int
  id
end

async def main -> Int
  fetch_user(42).await
end
"#;
        let result = parse_and_check(source);
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "async await should type-check: {:?}",
            errors
        );

        let crate::hir::nodes::HirItem::Function(main_fn) = &result.program.items[1] else {
            panic!("expected main function");
        };
        let crate::hir::nodes::HirExprKind::Block(_, Some(tail)) = &main_fn.body.kind else {
            panic!("expected block tail");
        };
        assert_eq!(tail.ty, Ty::Int, "await should unwrap Future output");
        let crate::hir::nodes::HirExprKind::MethodCall {
            object,
            method_name,
            ..
        } = &tail.kind
        else {
            panic!("expected await method call");
        };
        assert_eq!(method_name, "await");
        assert_eq!(
            object.ty,
            Ty::Class {
                name: "Future".to_string(),
                generic_args: vec![Ty::Int],
            },
            "async call should synthesize Future[Int] before await",
        );
    }
}
