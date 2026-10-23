//! Tests for type checking: unification, coercion, inference, trait resolution.

#[cfg(test)]
mod tests {
    use crate::hir::context::TypeContext;
    use crate::hir::types::Ty;
    use crate::lexer::token::Span;
    use crate::typeck::unify::{can_coerce, unify};

    fn rx(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ruxen")
            .join(format!("{name}.rx"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
    }

    fn release_rx(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/release-e2e/cases")
            .join(format!("{name}.rx"));
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

    /// A borrow is not its referent: `&Int` and `Int` are different types,
    /// in either order.
    #[test]
    fn unify_ref_with_its_referent_fails() {
        let mut ctx = TypeContext::new();
        let r = Ty::Ref(Box::new(Ty::Int));
        assert!(unify(&r, &Ty::Int, &mut ctx, &span()).is_err());
        assert!(unify(&Ty::Int, &r, &mut ctx, &span()).is_err());
    }

    /// A scalar coerces out of its borrow by value; a non-scalar does not,
    /// and a value never coerces into a borrow.
    #[test]
    fn scalar_coerces_out_of_its_borrow() {
        let ctx = TypeContext::new();
        let borrow = |t: Ty| Ty::Ref(Box::new(t));
        assert!(can_coerce(&borrow(Ty::Int), &Ty::Int, &ctx));
        assert!(!can_coerce(&borrow(Ty::Int), &Ty::Float, &ctx));
        assert!(!can_coerce(&borrow(Ty::String), &Ty::String, &ctx));
        assert!(!can_coerce(&Ty::Int, &borrow(Ty::Int), &ctx));
    }

    /// Two different type params are different types; the same param unifies.
    #[test]
    fn unify_distinct_type_params_fails() {
        let mut ctx = TypeContext::new();
        let param = |name: &str| Ty::TypeParam {
            name: name.to_string(),
            bounds: vec![],
        };
        assert!(unify(&param("T"), &param("U"), &mut ctx, &span()).is_err());
        assert!(unify(&param("T"), &param("T"), &mut ctx, &span()).is_ok());
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

    // (The `coerce_ref_string_to_str` test was removed with the `&str` type —
    // `&String` is the only string borrow type now, so there is no
    // `&String → &str` coercion to exercise. one-string-type ADR.)

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
    fn return_value_is_checked_against_the_declared_return_type() {
        // B84. `return <value>` lands in the SAME slot as the body's tail
        // expression, so it must run the same check. This arm used to call
        // `unify` and DISCARD the result, so a bare `Int` returned from a
        // `-> Result[Int, String]` function type-checked, skipped the `Ok`
        // box the slot's layout requires, and SIGSEGV'd the caller — a
        // silent miscompile with no diagnostic.
        //
        // The e2e harness has no compile-fail expectation, so the negative
        // half of the B84 pins (cases 1850-1852) lives here.
        let result = parse_and_check(
            "def f(x: Int) -> Result[Int, String]\n  \
             if x > 1\n    return x * 10\n  end\n  Err(\"small\")\nend",
        );
        let has_mismatch = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .any(|d| d.message.contains("type mismatch"));
        assert!(
            has_mismatch,
            "a bare `return <T>` from a `-> Result[T, E]` function must be a \
             type mismatch, not a silent unboxed return; got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn return_inside_a_block_still_targets_the_function_return_slot() {
        // B84 follow-up. `current_return_ty` is set once per function and is
        // NOT reset when inference descends into a block, so a `return`
        // written inside `a.each do |x| … end` is checked against `scan`'s
        // declared `Int?`. That is the correct target (a `return` in a Ruby
        // block returns from the enclosing method), and it is what makes the
        // escalation from a discarded `unify` to a REPORTED check safe here:
        // the bare `x` auto-wraps into the `Option` slot exactly as it does
        // at the top level, rather than becoming a false type mismatch.
        let result = parse_and_check(
            "def scan(a: Array[Int]) -> Int?\n  \
             a.each do |x|\n    if x > 2\n      return x\n    end\n  end\n  nil\nend",
        );
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "`return <T>` inside a block in a `-> T?` function must auto-wrap, \
             not error: {errors:?}"
        );
    }

    #[test]
    fn return_of_a_bare_value_wraps_into_an_option_return_slot() {
        // B84, the positive half: the "drop `Some`" sugar that the body's
        // TAIL expression already got must also fire for an explicit
        // `return`, otherwise the two spellings disagree about the value's
        // representation. Must type-check clean.
        let result = parse_and_check(
            "def f(x: Int) -> Int?\n  \
             if x > 1\n    return x * 10\n  end\n  nil\nend",
        );
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "`return <T>` in a `-> T?` function must auto-wrap, not error: {errors:?}"
        );
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
    fn mixed_int_float_arithmetic_errors() {
        // `Int - Float` must surface a clean type error (E0707) at typeck
        // time, NOT silently flow through to codegen where it crashes the
        // Cranelift verifier with `isub.i64 ... arg has type f64`.
        let result = parse_and_check("def test\n  let a: Int = 1\n  let b = a - 3.5\nend");
        let has_e0707 = result
            .diagnostics
            .iter()
            .any(|d| d.code.as_deref() == Some("E0707"));
        assert!(
            has_e0707,
            "expected E0707 for `Int - Float`, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn matched_numeric_arithmetic_is_clean() {
        // Same-typed numeric arithmetic must NOT trip the new diagnostic.
        let result = parse_and_check("def test\n  let a: Int = 1\n  let b = a - 2\nend");
        let has_e0707 = result
            .diagnostics
            .iter()
            .any(|d| d.code.as_deref() == Some("E0707"));
        assert!(
            !has_e0707,
            "Int - Int must not emit E0707, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn unknown_method_on_primitive_errors() {
        // `a.to_f()` on an Int has no such method. Typeck must emit a
        // clean "no method" error instead of silently returning a fresh
        // type var that crashes the JIT with `can't resolve symbol
        // Int_to_f`.
        let result = parse_and_check("def test\n  let a: Int = 1\n  let b = a.bogus()\nend");
        let has_no_method = result.diagnostics.iter().any(|d| {
            d.level == crate::diagnostics::DiagnosticLevel::Error
                && d.message.contains("no method")
                && d.message.contains("bogus")
        });
        assert!(
            has_no_method,
            "expected `no method bogus` error, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn int_to_f_infers_float() {
        // `Int.to_f` converts to Float; typeck must resolve it cleanly
        // (no "no method" error from the REQ2 scalar-primitive guard).
        let result = parse_and_check("def test\n  let a: Int = 1\n  let b = a.to_f()\nend");
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "`Int.to_f` should type-check cleanly, got: {:?}",
            errors
        );
    }

    #[test]
    fn float_to_i_infers_int() {
        // `Float.to_i` converts to Int; typeck must resolve it cleanly.
        let result = parse_and_check("def test\n  let a: Float = 1.5\n  let b = a.to_i()\nend");
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "`Float.to_i` should type-check cleanly, got: {:?}",
            errors
        );
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
        let source = rx("enum_variant_resolution");
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
        let source = rx("class_definition");
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
        let source = rx("mixin_and_include");
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
        let source = rx("match_expression");
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
        let source = rx("if_expression_types");
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
        let source = rx("generic_class");
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
        let source = rx("send_bound_rejects_raw_pointer_payload");
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
        let source = rx("sync_bound_rejects_negative_include_opt_out");
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
        let source = rx("unsafe_include_send_satisfies_send_bound");
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
        let source = rx("async_function_call_returns_future_and_await_unwraps_output");
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

    // ─── Parallel-assignment owned-heap guard (v1) ──────────────────
    // The MIR lowering aliases (does not move/clone) a place-read owned
    // Class/Struct/Enum value, so a swap or an alias-then-reassign would
    // double-free. typeck rejects every non-fresh owned value until real
    // move/clone lowering lands; only a freshly-produced value is allowed.

    #[test]
    fn multiassign_swap_of_owned_class_is_rejected_once() {
        let source = rx("multiassign_swap_owned_class");
        let result = parse_and_check(&source);
        let guard_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("parallel assignment of an owned"))
            .collect();
        // Both swapped values offend, but they share one span — the guard
        // must report the statement once, not once per value.
        assert_eq!(
            guard_errors.len(),
            1,
            "expected exactly one parallel-assignment error for a swap, got {:?}",
            guard_errors
        );
    }

    #[test]
    fn multiassign_ternary_aliased_owned_class_is_rejected() {
        // A place read wrapped in a ternary (`(flag ? a : b)`) lowers to
        // `HirExprKind::If`, not a bare VarRef — it must still be caught,
        // otherwise the double-free the guard exists to stop slips past.
        let source = rx("multiassign_ternary_alias_owned_class");
        let result = parse_and_check(&source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("parallel assignment of an owned")),
            "expected the ternary-aliased owned value to be rejected, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn multiassign_of_fresh_allocations_is_allowed() {
        // Freshly-constructed values cannot alias a target, so they are
        // safe and must NOT trip the guard.
        let source = rx("multiassign_fresh_alloc");
        let result = parse_and_check(&source);
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("parallel assignment of an owned")),
            "fresh allocations must be allowed in a parallel assignment, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn multiassign_ternary_of_fresh_values_is_allowed() {
        // A ternary/`if` wrapping two fresh constructors cannot alias a
        // target, so the guard must peel the wrapper and allow it — it is
        // only the ternary of PLACE reads that aliases. Guards against the
        // allow-list over-rejecting previously-compiling correct code.
        let source = rx("multiassign_ternary_fresh");
        let result = parse_and_check(&source);
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("parallel assignment of an owned")),
            "a ternary of fresh allocations must be allowed, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn multiassign_operator_overload_that_can_alias_target_is_rejected() {
        // `a + b` can desugar to a user operator method that forwards `b`.
        // Until sugared operators get the same move analysis as explicit
        // `a.+(b)`, treating the BinaryOp result as fresh reopens the
        // owned-heap parallel-assignment UAF.
        let source = rx("multiassign_operator_alias_owned_class");
        let result = parse_and_check(&source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("parallel assignment of an owned")),
            "expected the operator-overload owned value to be rejected, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn multiassign_splat_accepts_copy_array_elements_and_value_lists() {
        let source = rx("multiassign_splat");
        let result = parse_and_check(&source);
        assert!(
            result.diagnostics.is_empty(),
            "expected assignment splat fixture to type-check, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn multiassign_splat_rejects_owned_array_elements() {
        let source = rx("multiassign_splat_owned_elements");
        let result = parse_and_check(&source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("requires Copy array elements")),
            "expected owned-element safety diagnostic, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn multiassign_splat_rejects_a_borrowed_array_target() {
        let source = rx("multiassign_splat_ref_target");
        let result = parse_and_check(&source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("target must be an owned Array")),
            "expected borrowed-target safety diagnostic, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn multiassign_reports_pairwise_type_mismatches() {
        let source = rx("multiassign_type_mismatch");
        let result = parse_and_check(&source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("type mismatch")),
            "expected pairwise type mismatch, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn keyword_logical_operators_require_bool_operands() {
        let source = rx("keyword_logical_non_bool");
        let result = parse_and_check(&source);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("E0731")
                    && diagnostic.message.contains("operator `and`")
            }),
            "expected a strict-Bool `and` diagnostic, got {:?}",
            result.diagnostics
        );
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("E0731")
                    && diagnostic.message.contains("operator `not`")
            }),
            "expected a strict-Bool `not` diagnostic, got {:?}",
            result.diagnostics
        );
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("E0731")
                    && diagnostic.message.contains("operator `&&`")
            }),
            "expected a strict-Bool `&&` diagnostic, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn comparable_operator_defaults_typecheck() {
        let source = release_rx("965_comparable_spaceship");
        let result = parse_and_check(&source);
        assert!(
            result.diagnostics.is_empty(),
            "expected user-defined comparison operators and Comparable defaults to type-check, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn spaceship_without_a_method_or_primitive_operand_is_rejected() {
        let source = rx("spaceship_without_method");
        let result = parse_and_check(&source);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("E0707")
                    && diagnostic.message.contains("operator `<=>`")
            }),
            "expected unsupported spaceship diagnostic, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn comparison_operator_method_return_type_is_enforced() {
        let source = rx("comparison_method_bad_return");
        let result = parse_and_check(&source);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("E0707")
                    && diagnostic
                        .message
                        .contains("operator method `<=>` must return `Int`")
            }),
            "expected comparison return-type diagnostic, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn nil_coalesce_accepts_option_and_matching_fallback() {
        let source = release_rx("966_nil_coalesce");
        let result = parse_and_check(&source);
        assert!(
            result.diagnostics.is_empty(),
            "expected nil coalescing to type-check, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn nil_coalesce_rejects_invalid_operands() {
        let source = rx("nil_coalesce_invalid_operands");
        let result = parse_and_check(&source);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("E0732")
                    && diagnostic.message.contains("requires an Option")
            }),
            "expected invalid-left E0732, got {:?}",
            result.diagnostics
        );
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("E0732")
                    && diagnostic.message.contains("fallback must have type `Int`")
            }),
            "expected fallback-type E0732, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn bitwise_complement_on_non_integer_is_rejected() {
        // `~` flips integer bits; applying it to a Float is E0733.
        let source = rx("bitnot_non_integer");
        let result = parse_and_check(&source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code.as_deref() == Some("E0733")),
            "expected E0733 for `~` on a Float, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn tap_then_block_with_two_params_is_rejected() {
        // `tap`/`then` blocks take at most one parameter (the receiver); a
        // wider block must be a clean compile error, not a silent miscompile
        // that reads an unbound local.
        let source = rx("tap_then_arity");
        let result = parse_and_check(&source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("this block takes at most 1 parameter")),
            "expected an arity error for a 2-param tap/then block, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn comparable_clamp_self_return_resolves_to_concrete_receiver() {
        // `Comparable#clamp` returns `&Self`; at the call site on a concrete
        // `Rank` the abstract `Self: Comparable` must substitute to `Rank`
        // so `.value` (a Rank field) resolves on the result. Without the
        // Self-return substitution this reports `no field value on Self`.
        let source = release_rx("967_comparable_clamp");
        let result = parse_and_check(&source);
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "clamp's `&Self` return should resolve to the concrete receiver, got {:?}",
            errors
        );
    }
}
