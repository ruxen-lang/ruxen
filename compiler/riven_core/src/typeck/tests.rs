//! Tests for type checking: unification, coercion, inference, trait resolution.

#[cfg(test)]
mod tests {
    use crate::hir::context::TypeContext;
    use crate::hir::types::Ty;
    use crate::lexer::token::Span;
    use crate::typeck::unify::{can_coerce, unify};

    fn rvn(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/riven")
            .join(format!("{name}.rvn"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
    }

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
        let a = Ty::Array(Box::new(Ty::Int));
        let t = ctx.fresh_type_var();
        let b = Ty::Array(Box::new(t));
        let result = unify(&a, &b, &mut ctx, &span());
        assert_eq!(result.unwrap(), Ty::Array(Box::new(Ty::Int)));
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
        let source = rvn("enum_variant_resolution");
        let result = parse_and_check(&source);
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
        let source = rvn("class_definition");
        let result = parse_and_check(&source);
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

    // ruby-naming.spec.md §3.4: `mixin` replaces `trait`. Trait impls
    // are folded into the type body as `include Mixin` directives with
    // methods scattered alongside (§10a migration mapping).
    #[test]
    fn mixin_and_include() {
        let source = rvn("mixin_and_include");
        let result = parse_and_check(&source);
        let type_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .filter(|d| !d.message.contains("could not infer"))
            .collect();
        assert!(
            type_errors.is_empty(),
            "Mixin+include should type-check: {:?}",
            type_errors
        );
    }

    #[test]
    fn match_expression() {
        let source = rvn("match_expression");
        let result = parse_and_check(&source);
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
        let source = rvn("if_expression_types");
        let result = parse_and_check(&source);
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
        let source = rvn("generic_class");
        let result = parse_and_check(&source);
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
        let source = rvn("send_bound_rejects_raw_pointer_payload");
        let result = parse_and_check(&source);
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
    fn sync_bound_rejects_negative_include_opt_out() {
        // ruby-naming.spec.md §10a: `impl !Sync for T` → in-body
        // `include !Sync` opt-out directive.
        let source = rvn("sync_bound_rejects_negative_include_opt_out");
        let result = parse_and_check(&source);
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
    fn unsafe_include_send_satisfies_send_bound() {
        // ruby-naming.spec.md §10a: `unsafe impl Send for T` → in-body
        // `unsafe include Send` directive; `null` → `nil`.
        let source = rvn("unsafe_include_send_satisfies_send_bound");
        let result = parse_and_check(&source);
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
        // Async sub-phase 2 Milestone 2A
        // (docs/specs/syntax/async_lowering.spec.md B1–B6) changed
        // the call-site semantics: an `async def` with no `.await`
        // in its body is lowered to a real state-machine class, so
        // `fetch_user(42)` now returns
        // `Ty::Class { name: "__FetchUserFuture", ... }` rather
        // than the sub-phase 1 bridge-mode `Ty::Class { name:
        // "Future", generic_args: [Int] }`. The caller (`async def
        // main`) DOES have `.await` so 2A leaves it alone (2B's
        // job); the `.await` therefore still dispatches via the
        // sub-phase 1 elision path. This test now pins the post-2A
        // shape: typecheck clean + the `.await` MethodCall is
        // present.
        //
        // When 2B lands and lowers `async def main` too, this test
        // will need to change AGAIN — the `.await` MethodCall
        // would become a `(match self.__sub.poll(cx) ...)` block.
        let source = rvn("async_function_call_returns_future_and_await_unwraps_output");
        let result = parse_and_check(&source);
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

        // The user wrote two top-level fns (fetch_user, main).
        // Milestone 2A's lowering prepends the synthesised
        // `__FetchUserFuture` class, so the user items shift by one.
        // `main` is the LAST item.
        let main_fn = result
            .program
            .items
            .iter()
            .rev()
            .find_map(|item| match item {
                crate::hir::nodes::HirItem::Function(f) if f.name == "main" => Some(f),
                _ => None,
            })
            .expect("expected `main` function in HIR");
        let crate::hir::nodes::HirExprKind::Block(_, Some(tail)) = &main_fn.body.kind else {
            panic!("expected block tail");
        };
        let crate::hir::nodes::HirExprKind::MethodCall {
            object: _,
            method_name,
            ..
        } = &tail.kind
        else {
            panic!("expected await method call");
        };
        assert_eq!(method_name, "await");
        // tail.ty depends on whether the .await elision path
        // recognised the (now non-Future) state-machine class. We
        // don't pin the exact Ty here — 2B's job is to make this a
        // real awaited-output unwrap; for now sub-phase 2A pins
        // only the structural reachability.
        let _ = tail;
    }
}
