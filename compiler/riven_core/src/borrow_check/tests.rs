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
    };
    (program, symbols)
}

/// Test 1: Use-after-move is detected.
///
/// ```riven
/// let s = String.from("hello")   // def_id 0, Ty::String
/// let t = s                      // def_id 1, moves from 0
/// puts s                          // ERROR E1001
/// ```
#[test]
fn detects_use_after_move() {
    let stmts = vec![
        // let s = String.from("hello")
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

/// Test 2: Copy types do not trigger use-after-move.
///
/// ```riven
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
/// ```riven
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

/// Test 5: Move while borrowed is detected (E1009).
///
/// ```riven
/// let mut s = String.from("hello")  // def_id 0, mutable, Ty::String
/// let r = &s                         // def_id 1, shared borrow of s
/// s = String.from("world")           // ERROR E1009 — can't mutate s while borrowed
/// ```
///
/// This uses an assignment to `s` (which calls `check_mutation`) while the
/// shared borrow held by `r` is still active, triggering E1009.
#[test]
fn detects_move_while_borrowed() {
    let stmts = vec![
        // let mut s = "hello"
        HirStatement::Let {
            def_id: 0,
            pattern: HirPattern::Binding {
                def_id: 0,
                name: "s".to_string(),
                mutable: true,
                span: span(1, 9),
            },
            ty: Ty::String,
            value: Some(HirExpr {
                kind: HirExprKind::StringLiteral("hello".to_string()),
                ty: Ty::String,
                span: span(1, 13),
            }),
            mutable: true,
            span: span(1, 1),
        },
        // let r = &s  (shared borrow of s)
        HirStatement::Let {
            def_id: 1,
            pattern: HirPattern::Binding {
                def_id: 1,
                name: "r".to_string(),
                mutable: false,
                span: span(2, 5),
            },
            ty: Ty::Ref(Box::new(Ty::String)),
            value: Some(HirExpr {
                kind: HirExprKind::Borrow {
                    mutable: false,
                    expr: Box::new(HirExpr {
                        kind: HirExprKind::VarRef(0),
                        ty: Ty::String,
                        span: span(2, 10),
                    }),
                },
                ty: Ty::Ref(Box::new(Ty::String)),
                span: span(2, 9),
            }),
            mutable: false,
            span: span(2, 1),
        },
        // s = "world"  (assignment to s while r borrows it — E1009)
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::Assign {
                target: Box::new(HirExpr {
                    kind: HirExprKind::VarRef(0),
                    ty: Ty::String,
                    span: span(3, 1),
                }),
                value: Box::new(HirExpr {
                    kind: HirExprKind::StringLiteral("world".to_string()),
                    ty: Ty::String,
                    span: span(3, 5),
                }),
                semantics: MoveSemantics::Move,
            },
            ty: Ty::String,
            span: span(3, 1),
        }),
        // puts r  (keeps the borrow alive through line 4)
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::FnCall {
                callee: 51,
                callee_name: "puts".to_string(),
                args: vec![HirExpr {
                    kind: HirExprKind::VarRef(1),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    span: span(4, 6),
                }],
            },
            ty: Ty::Unit,
            span: span(4, 1),
        }),
    ];

    let (program, mut symbols) = make_program(stmts);

    symbols.define(
        "s".to_string(),
        DefKind::Variable {
            mutable: true,
            ty: Ty::String,
        },
        Visibility::Private,
        span(1, 9),
    ); // def_id 0
    symbols.define(
        "r".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::Ref(Box::new(Ty::String)),
        },
        Visibility::Private,
        span(2, 5),
    ); // def_id 1

    let errors = borrow_check(&program, &symbols);

    assert!(
        !errors.is_empty(),
        "expected E1009 (move while borrowed), got no errors"
    );
    assert!(
        errors.iter().any(|e| e.code == ErrorCode::E1009),
        "expected at least one E1009 error, got: {:?}",
        errors.iter().map(|e| e.code).collect::<Vec<_>>()
    );
}

/// Test 3: Assignment to immutable variable is detected.
///
/// ```riven
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

/// Test 6: Mutable + immutable borrow conflict is detected (E1002).
///
/// ```riven
/// let mut v = vec![1, 2, 3]  // def_id 0, mutable, Vec[Int]
/// let r = &v                  // def_id 1, creates shared borrow of v
/// v.push(4)                   // needs &mut v — ERROR E1002 (shared borrow still active)
/// puts r                      // shared borrow is still used here (after push attempt)
/// ```
///
/// The borrow of `v` is alive when the mutable borrow is attempted at line 3,
/// because `r` is used at line 4 (after the push). The `v.push(4)` call is modelled
/// with the receiver as `Borrow { mutable: true, VarRef(0) }`, which goes through
/// `check_borrow` and detects the conflict with the active shared borrow.
#[test]
fn detects_mut_immut_borrow_conflict() {
    let vec_ty = Ty::Array(Box::new(Ty::Int));
    let ref_vec_ty = Ty::Ref(Box::new(vec_ty.clone()));

    let stmts = vec![
        // let mut v = vec![1, 2, 3]  (def_id 0)
        HirStatement::Let {
            def_id: 0,
            pattern: HirPattern::Binding {
                def_id: 0,
                name: "v".to_string(),
                mutable: true,
                span: span(1, 9),
            },
            ty: vec_ty.clone(),
            value: Some(HirExpr {
                kind: HirExprKind::MacroCall {
                    name: "vec".to_string(),
                    args: vec![
                        HirExpr {
                            kind: HirExprKind::IntLiteral(1),
                            ty: Ty::Int,
                            span: span(1, 14),
                        },
                        HirExpr {
                            kind: HirExprKind::IntLiteral(2),
                            ty: Ty::Int,
                            span: span(1, 17),
                        },
                        HirExpr {
                            kind: HirExprKind::IntLiteral(3),
                            ty: Ty::Int,
                            span: span(1, 20),
                        },
                    ],
                },
                ty: vec_ty.clone(),
                span: span(1, 13),
            }),
            mutable: true,
            span: span(1, 1),
        },
        // let r = &v  (def_id 1, creates shared borrow of v)
        HirStatement::Let {
            def_id: 1,
            pattern: HirPattern::Binding {
                def_id: 1,
                name: "r".to_string(),
                mutable: false,
                span: span(2, 5),
            },
            ty: ref_vec_ty.clone(),
            value: Some(HirExpr {
                kind: HirExprKind::Borrow {
                    mutable: false,
                    expr: Box::new(HirExpr {
                        kind: HirExprKind::VarRef(0),
                        ty: vec_ty.clone(),
                        span: span(2, 10),
                    }),
                },
                ty: ref_vec_ty.clone(),
                span: span(2, 9),
            }),
            mutable: false,
            span: span(2, 1),
        },
        // v.push(4)  — receiver is &mut v, triggers E1002 because shared borrow is active.
        // Modelled as MethodCall where the object is Borrow { mutable: true, VarRef(0) }
        // to represent the implicit &mut self receiver of a mutating method.
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::MethodCall {
                object: Box::new(HirExpr {
                    kind: HirExprKind::Borrow {
                        mutable: true,
                        expr: Box::new(HirExpr {
                            kind: HirExprKind::VarRef(0),
                            ty: vec_ty.clone(),
                            span: span(3, 1),
                        }),
                    },
                    ty: Ty::RefMut(Box::new(vec_ty.clone())),
                    span: span(3, 1),
                }),
                method: 50,
                method_name: "push".to_string(),
                generic_args: vec![],
                args: vec![HirExpr {
                    kind: HirExprKind::IntLiteral(4),
                    ty: Ty::Int,
                    span: span(3, 8),
                }],
                block: None,
            },
            ty: Ty::Unit,
            span: span(3, 1),
        }),
        // puts r  — shared borrow is still used here (line 4 comes after line 3)
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::FnCall {
                callee: 51,
                callee_name: "puts".to_string(),
                args: vec![HirExpr {
                    kind: HirExprKind::VarRef(1),
                    ty: ref_vec_ty.clone(),
                    span: span(4, 6),
                }],
            },
            ty: Ty::Unit,
            span: span(4, 1),
        }),
    ];

    let (program, mut symbols) = make_program(stmts);

    symbols.define(
        "v".to_string(),
        DefKind::Variable {
            mutable: true,
            ty: Ty::Array(Box::new(Ty::Int)),
        },
        Visibility::Private,
        span(1, 9),
    ); // def_id 0
    symbols.define(
        "r".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::Ref(Box::new(Ty::Array(Box::new(Ty::Int)))),
        },
        Visibility::Private,
        span(2, 5),
    ); // def_id 1

    let errors = borrow_check(&program, &symbols);

    assert!(
        !errors.is_empty(),
        "expected a borrow conflict error, got no errors"
    );
    assert!(
        errors.iter().any(|e| e.code == ErrorCode::E1002),
        "expected at least one E1002 (cannot borrow writably — already borrowed read-only), \
         got: {:?}",
        errors.iter().map(|e| e.code).collect::<Vec<_>>()
    );
}

/// Test 7: NLL — borrow ends at last use, allowing subsequent mutation.
///
/// ```riven
/// let mut v = vec![1, 2, 3]  // def_id 0, mutable, Vec[Int]
/// let first = &v              // def_id 1, creates shared borrow of v
/// puts first                  // last use of first (line 3)
/// v.push(4)                   // line 4 — OK, borrow has expired by NLL
/// ```
///
/// With NLL semantics, the borrow of `v` (held via `first`) expires at its last use,
/// which is line 3 (span.start = 30). The mutation at line 4 (span.start = 40) does
/// not conflict because `expire_before` is called at the start of each expression with
/// the current span, and once the borrow's last_use is before the mutation point the
/// borrow is expired. The checker therefore correctly accepts this code.
#[test]
fn nll_borrow_ends_at_last_use() {
    let vec_ty = Ty::Array(Box::new(Ty::Int));
    let ref_vec_ty = Ty::Ref(Box::new(vec_ty.clone()));

    let stmts = vec![
        // let mut v = vec![1, 2, 3]  (def_id 0)
        HirStatement::Let {
            def_id: 0,
            pattern: HirPattern::Binding {
                def_id: 0,
                name: "v".to_string(),
                mutable: true,
                span: span(1, 9),
            },
            ty: vec_ty.clone(),
            value: Some(HirExpr {
                kind: HirExprKind::MacroCall {
                    name: "vec".to_string(),
                    args: vec![
                        HirExpr {
                            kind: HirExprKind::IntLiteral(1),
                            ty: Ty::Int,
                            span: span(1, 14),
                        },
                        HirExpr {
                            kind: HirExprKind::IntLiteral(2),
                            ty: Ty::Int,
                            span: span(1, 17),
                        },
                        HirExpr {
                            kind: HirExprKind::IntLiteral(3),
                            ty: Ty::Int,
                            span: span(1, 20),
                        },
                    ],
                },
                ty: vec_ty.clone(),
                span: span(1, 13),
            }),
            mutable: true,
            span: span(1, 1),
        },
        // let first = &v  (def_id 1, creates shared borrow of v at line 2)
        HirStatement::Let {
            def_id: 1,
            pattern: HirPattern::Binding {
                def_id: 1,
                name: "first".to_string(),
                mutable: false,
                span: span(2, 5),
            },
            ty: ref_vec_ty.clone(),
            value: Some(HirExpr {
                kind: HirExprKind::Borrow {
                    mutable: false,
                    expr: Box::new(HirExpr {
                        kind: HirExprKind::VarRef(0),
                        ty: vec_ty.clone(),
                        span: span(2, 13),
                    }),
                },
                ty: ref_vec_ty.clone(),
                span: span(2, 12),
            }),
            mutable: false,
            span: span(2, 1),
        },
        // puts first  — last use of the borrow (line 3, span.start = 30)
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::FnCall {
                callee: 51,
                callee_name: "puts".to_string(),
                args: vec![HirExpr {
                    kind: HirExprKind::VarRef(1),
                    ty: ref_vec_ty.clone(),
                    span: span(3, 6),
                }],
            },
            ty: Ty::Unit,
            span: span(3, 1),
        }),
        // v.push(4)  — line 4 (span.start = 40).
        // The shared borrow of v has expired via NLL (last_use at start=30 < current start=40).
        // Modelled as a plain MethodCall with VarRef(0) as receiver (no explicit &mut v),
        // consistent with how the checker handles non-consuming method calls.
        HirStatement::Expr(HirExpr {
            kind: HirExprKind::MethodCall {
                object: Box::new(HirExpr {
                    kind: HirExprKind::VarRef(0),
                    ty: vec_ty.clone(),
                    span: span(4, 1),
                }),
                method: 50,
                method_name: "push".to_string(),
                generic_args: vec![],
                args: vec![HirExpr {
                    kind: HirExprKind::IntLiteral(4),
                    ty: Ty::Int,
                    span: span(4, 8),
                }],
                block: None,
            },
            ty: Ty::Unit,
            span: span(4, 1),
        }),
    ];

    let (program, mut symbols) = make_program(stmts);

    symbols.define(
        "v".to_string(),
        DefKind::Variable {
            mutable: true,
            ty: Ty::Array(Box::new(Ty::Int)),
        },
        Visibility::Private,
        span(1, 9),
    ); // def_id 0
    symbols.define(
        "first".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: Ty::Ref(Box::new(Ty::Array(Box::new(Ty::Int)))),
        },
        Visibility::Private,
        span(2, 5),
    ); // def_id 1

    let errors = borrow_check(&program, &symbols);

    // No borrow conflict errors expected — the borrow of v expires before the mutation.
    let conflict_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.code == ErrorCode::E1002 || e.code == ErrorCode::E1003)
        .collect();
    assert!(
        conflict_errors.is_empty(),
        "expected no borrow conflict errors (NLL allows mutation after last borrow use), \
         got: {:?}",
        conflict_errors.iter().map(|e| e.code).collect::<Vec<_>>()
    );
}

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
        }],
        return_ty: Ty::Unit,
        is_async: false,
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
