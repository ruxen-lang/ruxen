use crate::borrow_check::borrow_check;
use crate::borrow_check::errors::ErrorCode;
use crate::hir::nodes::*;
use crate::hir::types::{MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;
use crate::resolve::symbols::{DefKind, SymbolTable};

fn span(line: u32, col: u32) -> Span {
    Span::new(line as usize * 10, line as usize * 10 + 5, line, col)
}

fn make_program(body_stmts: Vec<HirStatement>) -> (HirProgram, SymbolTable) {
    let symbols = SymbolTable::new();
    let func = HirFuncDef {
        def_id: 100, // use a high id to avoid collisions with test bindings
        name: "test_fn".to_string(),
        visibility: Visibility::Private,
        is_async: false,
        self_mode: None,
        is_class_method: false,
        generic_params: vec![],
        params: vec![],
        return_ty: Ty::Unit,
        body: Box::new(HirExpr {
            kind: HirExprKind::Block(body_stmts, None),
            ty: Ty::Unit,
            span: span(1, 1),
        }),
        doc_comments: vec![],
        span: span(1, 1),
    };
    let program = HirProgram {
        items: vec![HirItem::Function(func)],
        span: span(1, 1),
        ffi_libs: vec![],
        prelude_item_count: 0,
    };
    (program, symbols)
}

/// Test 1: Use-after-move is detected.
///
/// ```ruxen
/// let s = "hello"   // def_id 0, Ty::String
/// let t = s                      // def_id 1, moves from 0
/// puts s                          // ERROR E1001
/// ```
#[test]
fn detects_use_after_move() {
    let stmts = vec![
        // let s = "hello"
        HirStatement::Let {
            def_id: 0,
            pattern: HirPattern::Binding {
                def_id: 0,
                name: "s".to_string(),
                mutable: false,
                span: span(1, 5),
            },
            ty: Ty::String,
            value: Some(HirExpr {
                kind: HirExprKind::StringLiteral("hello".to_string()),
                ty: Ty::String,
                span: span(1, 9),
            }),
            mutable: false,
            span: span(1, 1),
        },
        // let t = s  (move)
        HirStatement::Let {
            def_id: 1,
            pattern: HirPattern::Binding {
                def_id: 1,
                name: "t".to_string(),
                mutable: false,
                span: span(2, 5),
            },
            ty: Ty::String,
            value: Some(HirExpr {
                kind: HirExprKind::VarRef(0),
                ty: Ty::String,
                span: span(2, 9),
            }),
            mutable: false,
            span: span(2, 1),
        },
        // puts s  (use after move)
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::FnCall {
                callee: 99,
                callee_name: "puts".to_string(),
                args: vec![HirExpr {
                    kind: HirExprKind::VarRef(0),
                    ty: Ty::String,
                    span: span(3, 6),
                }],
            },
            ty: Ty::Unit,
            span: span(3, 1),
        }),
    ];

    let (program, mut symbols) = make_program(stmts);

    // Register variables in the symbol table so the checker can look up names
    symbols.define(
        "s".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::String,
        },
        Visibility::Private,
        span(1, 5),
    ); // def_id 0
    symbols.define(
        "t".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::String,
        },
        Visibility::Private,
        span(2, 5),
    ); // def_id 1

    // The function def_id must match — adjust to use the symbols table properly
    // Since symbols.define allocates sequentially starting from 0, our def_ids 0 and 1
    // are already taken. Update the function's def_id to avoid overlap.
    // Actually, it's fine — the function def_id of 100 won't collide.

    let errors = borrow_check(&program, &symbols);

    assert!(
        !errors.is_empty(),
        "expected use-after-move error, got no errors"
    );
    assert_eq!(
        errors[0].code,
        ErrorCode::E1001,
        "expected E1001 (use after move), got {:?}",
        errors[0].code
    );
}

/// Regression pin (Task WS): items in the bootstrap/prelude prefix
/// (`HirProgram::prelude_item_count`) are NOT borrow-checked. The CLI merges
/// the trusted, e2e-verified prelude in front of user code; its generic
/// combinator bodies (`Enumerable#select`'s `pred.(x)` over abstract `T`,
/// then `out.push(x)`) trip false-positive moves. The borrow checker must
/// skip that prefix and check only user code — this is the "`ruxen compile`
/// hello-world" fix. (The e2e harness skips borrow-check entirely, which is
/// why this was a "green tests / broken CLI" gap.) Same body as
/// `detects_use_after_move`, but marked as prelude ⇒ no error.
#[test]
fn prelude_prefix_items_are_skipped() {
    let stmts = vec![
        // let s = "hello"
        HirStatement::Let {
            def_id: 0,
            pattern: HirPattern::Binding {
                def_id: 0,
                name: "s".to_string(),
                mutable: false,
                span: span(1, 5),
            },
            ty: Ty::String,
            value: Some(HirExpr {
                kind: HirExprKind::StringLiteral("hello".to_string()),
                ty: Ty::String,
                span: span(1, 9),
            }),
            mutable: false,
            span: span(1, 1),
        },
        // let t = s  (move)
        HirStatement::Let {
            def_id: 1,
            pattern: HirPattern::Binding {
                def_id: 1,
                name: "t".to_string(),
                mutable: false,
                span: span(2, 5),
            },
            ty: Ty::String,
            value: Some(HirExpr {
                kind: HirExprKind::VarRef(0),
                ty: Ty::String,
                span: span(2, 9),
            }),
            mutable: false,
            span: span(2, 1),
        },
        // puts s  (would be use-after-move IF checked)
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::FnCall {
                callee: 99,
                callee_name: "puts".to_string(),
                args: vec![HirExpr {
                    kind: HirExprKind::VarRef(0),
                    ty: Ty::String,
                    span: span(3, 6),
                }],
            },
            ty: Ty::Unit,
            span: span(3, 1),
        }),
    ];

    let (mut program, mut symbols) = make_program(stmts);
    symbols.define(
        "s".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::String,
        },
        Visibility::Private,
        span(1, 5),
    );
    symbols.define(
        "t".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::String,
        },
        Visibility::Private,
        span(2, 5),
    );

    // Mark the sole item as belonging to the prepended prelude prefix.
    program.prelude_item_count = 1;

    let errors = borrow_check(&program, &symbols);
    assert!(
        errors.is_empty(),
        "prelude-prefix items must be skipped by the borrow checker, got {:?}",
        errors
    );
}

/// Test 2: Copy types do not trigger use-after-move.
///
/// ```ruxen
/// let x = 42   // def_id 0, Ty::Int (Copy)
/// let y = x    // def_id 1, Copy — no move
/// puts x       // OK
/// ```
#[test]
fn copy_type_no_error() {
    let stmts = vec![
        // let x = 42
        HirStatement::Let {
            def_id: 0,
            pattern: HirPattern::Binding {
                def_id: 0,
                name: "x".to_string(),
                mutable: false,
                span: span(1, 5),
            },
            ty: Ty::Int,
            value: Some(HirExpr {
                kind: HirExprKind::IntLiteral(42),
                ty: Ty::Int,
                span: span(1, 9),
            }),
            mutable: false,
            span: span(1, 1),
        },
        // let y = x  (copy, no move)
        HirStatement::Let {
            def_id: 1,
            pattern: HirPattern::Binding {
                def_id: 1,
                name: "y".to_string(),
                mutable: false,
                span: span(2, 5),
            },
            ty: Ty::Int,
            value: Some(HirExpr {
                kind: HirExprKind::VarRef(0),
                ty: Ty::Int,
                span: span(2, 9),
            }),
            mutable: false,
            span: span(2, 1),
        },
        // puts x  (should be fine — Int is Copy)
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::FnCall {
                callee: 99,
                callee_name: "puts".to_string(),
                args: vec![HirExpr {
                    kind: HirExprKind::VarRef(0),
                    ty: Ty::Int,
                    span: span(3, 6),
                }],
            },
            ty: Ty::Unit,
            span: span(3, 1),
        }),
    ];

    let (program, mut symbols) = make_program(stmts);

    symbols.define(
        "x".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::Int,
        },
        Visibility::Private,
        span(1, 5),
    );
    symbols.define(
        "y".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::Int,
        },
        Visibility::Private,
        span(2, 5),
    );

    let errors = borrow_check(&program, &symbols);

    assert!(
        errors.is_empty(),
        "expected no errors for copy types, got: {:?}",
        errors
    );
}

/// Test 4: Mutable borrow of immutable variable is detected (E1007).
///
/// ```ruxen
/// let x = 42       // def_id 0, immutable, Ty::Int
/// let r = &mut x   // ERROR E1007 — can't mut-borrow an immutable var
/// ```
#[test]
fn detects_mut_borrow_of_immutable() {
    let stmts = vec![
        // let x = 42
        HirStatement::Let {
            def_id: 0,
            pattern: HirPattern::Binding {
                def_id: 0,
                name: "x".to_string(),
                mutable: false,
                span: span(1, 5),
            },
            ty: Ty::Int,
            value: Some(HirExpr {
                kind: HirExprKind::IntLiteral(42),
                ty: Ty::Int,
                span: span(1, 9),
            }),
            mutable: false,
            span: span(1, 1),
        },
        // let r = &mut x  (mutable borrow of immutable variable)
        HirStatement::Let {
            def_id: 1,
            pattern: HirPattern::Binding {
                def_id: 1,
                name: "r".to_string(),
                mutable: false,
                span: span(2, 5),
            },
            ty: Ty::RefMut(Box::new(Ty::Int)),
            value: Some(HirExpr {
                kind: HirExprKind::Borrow {
                    mutable: true,
                    expr: Box::new(HirExpr {
                        kind: HirExprKind::VarRef(0),
                        ty: Ty::Int,
                        span: span(2, 14),
                    }),
                },
                ty: Ty::RefMut(Box::new(Ty::Int)),
                span: span(2, 9),
            }),
            mutable: false,
            span: span(2, 1),
        },
    ];

    let (program, mut symbols) = make_program(stmts);

    symbols.define(
        "x".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::Int,
        },
        Visibility::Private,
        span(1, 5),
    ); // def_id 0
    symbols.define(
        "r".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::RefMut(Box::new(Ty::Int)),
        },
        Visibility::Private,
        span(2, 5),
    ); // def_id 1

    let errors = borrow_check(&program, &symbols);

    assert!(
        !errors.is_empty(),
        "expected E1007 (mut borrow of immutable variable), got no errors"
    );
    assert!(
        errors.iter().any(|e| e.code == ErrorCode::E1007),
        "expected at least one E1007 error, got: {:?}",
        errors.iter().map(|e| e.code).collect::<Vec<_>>()
    );
}

/// Test 3: Assignment to immutable variable is detected.
///
/// ```ruxen
/// let x = 42   // def_id 0, mutable: false
/// x = 43       // ERROR E1006
/// ```
#[test]
fn detects_immutable_assignment() {
    let stmts = vec![
        // let x = 42
        HirStatement::Let {
            def_id: 0,
            pattern: HirPattern::Binding {
                def_id: 0,
                name: "x".to_string(),
                mutable: false,
                span: span(1, 5),
            },
            ty: Ty::Int,
            value: Some(HirExpr {
                kind: HirExprKind::IntLiteral(42),
                ty: Ty::Int,
                span: span(1, 9),
            }),
            mutable: false,
            span: span(1, 1),
        },
        // x = 43  (assign to immutable)
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::Assign {
                target: Box::new(HirExpr {
                    kind: HirExprKind::VarRef(0),
                    ty: Ty::Int,
                    span: span(2, 1),
                }),
                value: Box::new(HirExpr {
                    kind: HirExprKind::IntLiteral(43),
                    ty: Ty::Int,
                    span: span(2, 5),
                }),
                semantics: MoveSemantics::Copy,
            },
            ty: Ty::Int,
            span: span(2, 1),
        }),
    ];

    let (program, mut symbols) = make_program(stmts);

    symbols.define(
        "x".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::Int,
        },
        Visibility::Private,
        span(1, 5),
    );

    let errors = borrow_check(&program, &symbols);

    assert!(
        !errors.is_empty(),
        "expected immutable assignment error, got no errors"
    );
    assert_eq!(
        errors[0].code,
        ErrorCode::E1006,
        "expected E1006 (assign to immutable), got {:?}",
        errors[0].code
    );
}

/// A block borrows its captures, so binding one to a `Send`-bounded
/// parameter is E1013 (a proc or lambda owns them and passes).
#[test]
fn send_required_closure_rejects_borrow_capture() {
    let closure_ty = Ty::Fn {
        params: vec![],
        ret: Box::new(Ty::Unit),
    };
    let require_send_sig = crate::resolve::symbols::FnSignature {
        self_mode: None,
        is_class_method: false,
        generic_params: vec![crate::resolve::symbols::GenericParamInfo::type_param(
            "T".to_string(),
            vec![crate::hir::types::MixinRef {
                name: "Send".to_string(),
                generic_args: vec![],
            }],
        )],
        params: vec![crate::resolve::symbols::ParamInfo {
            name: "value".to_string(),
            ty: Ty::TypeParam {
                name: "T".to_string(),
                bounds: vec![crate::hir::types::MixinRef {
                    name: "Send".to_string(),
                    generic_args: vec![],
                }],
            },
            auto_assign: false,
            default: None,
        }],
        return_ty: Ty::Unit,
        is_async: false,
        c_symbol: None,
    };

    let stmts = vec![
        HirStatement::Let {
            def_id: 0,
            pattern: HirPattern::Binding {
                def_id: 0,
                name: "x".to_string(),
                mutable: false,
                span: span(1, 5),
            },
            ty: Ty::Int,
            value: Some(HirExpr {
                kind: HirExprKind::IntLiteral(42),
                ty: Ty::Int,
                span: span(1, 9),
            }),
            mutable: false,
            span: span(1, 1),
        },
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::FnCall {
                callee: 10,
                callee_name: "require_send".to_string(),
                args: vec![HirExpr {
                    kind: HirExprKind::Closure {
                        params: vec![],
                        body: Box::new(HirExpr {
                            kind: HirExprKind::VarRef(0),
                            ty: Ty::Int,
                            span: span(2, 18),
                        }),
                        captures: vec![Capture {
                            def_id: 0,
                            name: "x".to_string(),
                            by_move: false,
                            ty: Ty::Int,
                        }],
                        is_async: false,
                        is_move: false,
                        kind: crate::parser::ast::ClosureKind::Block,
                    },
                    ty: closure_ty,
                    span: span(2, 3),
                }],
            },
            ty: Ty::Unit,
            span: span(2, 1),
        }),
    ];

    let (program, mut symbols) = make_program(stmts);
    symbols.define(
        "x".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::Int,
        },
        Visibility::Private,
        span(1, 5),
    );
    symbols.define(
        "require_send".to_string(),
        DefKind::Function {
            signature: require_send_sig,
        },
        Visibility::Private,
        span(2, 1),
    );

    let errors = borrow_check(&program, &symbols);
    assert!(
        errors.iter().any(|err| err.code == ErrorCode::E1013),
        "expected E1013 for borrowed capture in send-required closure, got {:?}",
        errors
    );
}
