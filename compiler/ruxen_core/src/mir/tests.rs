#[cfg(test)]
mod tests {
    use crate::hir::types::Ty;
    use crate::mir::nodes::MirFunction;

    #[test]
    fn mir_function_creates_entry_block() {
        let func = MirFunction::new("test_fn", Ty::Unit);
        // A freshly created function must have exactly one block.
        assert_eq!(
            func.blocks.len(),
            1,
            "expected exactly 1 block (the entry block)"
        );
        assert_eq!(func.entry_block, 0, "entry_block should be index 0");
    }

    #[test]
    fn new_local_returns_sequential_ids() {
        let mut func = MirFunction::new("test_fn", Ty::Unit);
        let id0 = func.new_local("a", Ty::Int, false);
        let id1 = func.new_local("b", Ty::Bool, true);
        let id2 = func.new_local("c", Ty::Float, false);

        assert_eq!(id0, 0, "first local should have id 0");
        assert_eq!(id1, 1, "second local should have id 1");
        assert_eq!(id2, 2, "third local should have id 2");
    }

    #[test]
    fn new_block_returns_sequential_ids() {
        let mut func = MirFunction::new("test_fn", Ty::Unit);
        // Block 0 is the entry block, created automatically.
        let b1 = func.new_block();
        let b2 = func.new_block();
        let b3 = func.new_block();

        assert_eq!(b1, 1, "first added block should have id 1");
        assert_eq!(b2, 2);
        assert_eq!(b3, 3);
        assert_eq!(func.blocks.len(), 4, "4 blocks total (entry + 3 added)");
    }

    #[test]
    fn new_temp_generates_names() {
        let mut func = MirFunction::new("test_fn", Ty::Unit);
        let t0 = func.new_temp(Ty::Int);
        let t1 = func.new_temp(Ty::Bool);
        let t2 = func.new_temp(Ty::Float);

        assert_eq!(func.locals[t0 as usize].name, "_t0");
        assert_eq!(func.locals[t1 as usize].name, "_t1");
        assert_eq!(func.locals[t2 as usize].name, "_t2");
    }
}

// ─── MIR Lowering Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod lowering_tests {
    use crate::hir::nodes::*;
    use crate::hir::types::Ty;
    use crate::lexer::token::Span;
    use crate::mir::lower::Lowerer;
    use crate::mir::nodes::*;
    use crate::parser::ast::{BinOp, Visibility};
    use crate::resolve::symbols::SymbolTable;

    /// Helper: create a dummy span for test nodes.
    fn span() -> Span {
        Span::new(0, 0, 1, 1)
    }

    /// Helper: build a minimal HirProgram with one function.
    fn program_with_fn(func: HirFuncDef) -> HirProgram {
        HirProgram {
            items: vec![HirItem::Function(func)],
            span: span(),
            ffi_libs: vec![],
            prelude_item_count: 0,
        }
    }

    /// Helper: wrap an HirExprKind into an HirExpr with a given type.
    fn expr(kind: HirExprKind, ty: Ty) -> HirExpr {
        HirExpr {
            kind,
            ty,
            span: span(),
        }
    }

    /// Helper: build a simple function def (no params, no self).
    fn simple_func(
        def_id: DefId,
        name: &str,
        params: Vec<HirParam>,
        return_ty: Ty,
        body: HirExpr,
    ) -> HirFuncDef {
        HirFuncDef {
            def_id,
            name: name.to_string(),
            visibility: Visibility::Public,
            is_async: false,
            self_mode: None,
            is_class_method: false,
            generic_params: vec![],
            params,
            return_ty,
            body: Box::new(body),
            doc_comments: vec![],
            span: span(),
        }
    }

    // ── Test 1: empty main ──────────────────────────────────────────────

    #[test]
    fn lower_empty_main() {
        let symbols = SymbolTable::new();
        let func = simple_func(
            0,
            "main",
            vec![],
            Ty::Unit,
            // Body is an empty block: `{ }`
            expr(HirExprKind::Block(vec![], None), Ty::Unit),
        );
        let program = program_with_fn(func);

        let mut lowerer = Lowerer::new(&symbols);
        let mir = lowerer.lower_program(&program).expect("lowering failed");

        assert_eq!(
            mir.entry,
            Some("main".to_string()),
            "entry should be 'main'"
        );
        // Phase 2 #06.D2.S1: 1 user function + 5 unconditionally-emitted
        // primitive `_fmt` synth functions (Char_fmt / Int_fmt / Float_fmt /
        // Bool_fmt / String_fmt).
        assert_eq!(
            mir.functions.len(),
            6,
            "should have 1 user fn + 5 primitive _fmt synth fns"
        );

        let main_fn = mir
            .functions
            .iter()
            .find(|f| f.name == "main")
            .expect("main not found");
        assert_eq!(main_fn.params.len(), 0, "main takes no params");

        // The entry block should have a Return(None) terminator.
        let entry = &main_fn.blocks[main_fn.entry_block];
        assert!(
            matches!(entry.terminator, Terminator::Return(None)),
            "empty main should end with Return(None), got {:?}",
            entry.terminator
        );
    }

    // ── Test 2: let x = 42 ─────────────────────────────────────────────

    #[test]
    fn lower_int_literal_binding() {
        let mut symbols = SymbolTable::new();

        // DefId 0 = the function "test"
        let _fn_id = symbols.define(
            "test".to_string(),
            crate::resolve::symbols::DefKind::Function {
                signature: crate::resolve::symbols::FnSignature {
                    self_mode: None,
                    is_class_method: false,
                    is_async: false,
                    generic_params: vec![],
                    params: vec![],
                    return_ty: Ty::Unit,
                    c_symbol: None,
                },
            },
            Visibility::Public,
            span(),
        );

        // DefId 1 = the variable "x"
        let x_def = symbols.define(
            "x".to_string(),
            crate::resolve::symbols::DefKind::Variable {
                mutable: false,
                ty: Ty::Int,
            },
            Visibility::Private,
            span(),
        );

        let body = expr(
            HirExprKind::Block(
                vec![HirStatement::Let {
                    def_id: x_def,
                    pattern: HirPattern::Binding {
                        def_id: x_def,
                        name: "x".to_string(),
                        mutable: false,
                        span: span(),
                    },
                    ty: Ty::Int,
                    value: Some(expr(HirExprKind::IntLiteral(42), Ty::Int)),
                    mutable: false,
                    span: span(),
                }],
                None,
            ),
            Ty::Unit,
        );

        let func = simple_func(0, "test", vec![], Ty::Unit, body);
        let program = program_with_fn(func);

        let mut lowerer = Lowerer::new(&symbols);
        let mir = lowerer.lower_program(&program).expect("lowering failed");
        let test_fn = mir
            .functions
            .iter()
            .find(|f| f.name == "test")
            .expect("expected fn `test` in MIR");

        // Should have locals: _t0 (the literal temp) and x (the let binding).
        assert!(
            test_fn.locals.len() >= 2,
            "expected at least 2 locals, got {}",
            test_fn.locals.len()
        );

        // Find the local named "x".
        let x_local = test_fn
            .locals
            .iter()
            .find(|l| l.name == "x")
            .expect("should have a local named 'x'");
        assert_eq!(x_local.ty, Ty::Int);

        // The entry block should have at least 2 instructions:
        // 1. Assign { dest: _t0, value: Literal(Int(42)) }
        // 2. Assign { dest: x, value: Use(_t0) }
        let entry = &test_fn.blocks[test_fn.entry_block];
        assert!(
            entry.instructions.len() >= 2,
            "expected at least 2 instructions, got {}",
            entry.instructions.len()
        );

        // Verify the first instruction is the literal assignment.
        match &entry.instructions[0] {
            MirInst::Assign {
                value: MirValue::Literal(Literal::Int(42)),
                ..
            } => {} // correct
            other => panic!("expected Assign with Literal(Int(42)), got {:?}", other),
        }
    }

    // ── Test 3: def add(a: Int, b: Int) -> Int { a + b } ───────────────

    #[test]
    fn lower_function_with_params_and_return() {
        let mut symbols = SymbolTable::new();

        // DefId 0 = function "add"
        let _fn_id = symbols.define(
            "add".to_string(),
            crate::resolve::symbols::DefKind::Function {
                signature: crate::resolve::symbols::FnSignature {
                    self_mode: None,
                    is_class_method: false,
                    is_async: false,
                    generic_params: vec![],
                    params: vec![
                        crate::resolve::symbols::ParamInfo {
                            name: "a".to_string(),
                            ty: Ty::Int,
                            auto_assign: false,
                            default: None,
                        },
                        crate::resolve::symbols::ParamInfo {
                            name: "b".to_string(),
                            ty: Ty::Int,
                            auto_assign: false,
                            default: None,
                        },
                    ],
                    return_ty: Ty::Int,
                    c_symbol: None,
                },
            },
            Visibility::Public,
            span(),
        );

        // DefId 1 = param "a"
        let a_def = symbols.define(
            "a".to_string(),
            crate::resolve::symbols::DefKind::Param {
                ty: Ty::Int,
                auto_assign: false,
            },
            Visibility::Private,
            span(),
        );

        // DefId 2 = param "b"
        let b_def = symbols.define(
            "b".to_string(),
            crate::resolve::symbols::DefKind::Param {
                ty: Ty::Int,
                auto_assign: false,
            },
            Visibility::Private,
            span(),
        );

        // Body: a + b  (a Block with a tail expression)
        let body = expr(
            HirExprKind::Block(
                vec![],
                Some(Box::new(expr(
                    HirExprKind::BinaryOp {
                        op: BinOp::Add,
                        left: Box::new(expr(HirExprKind::VarRef(a_def), Ty::Int)),
                        right: Box::new(expr(HirExprKind::VarRef(b_def), Ty::Int)),
                    },
                    Ty::Int,
                ))),
            ),
            Ty::Int,
        );

        let params = vec![
            HirParam {
                def_id: a_def,
                name: "a".to_string(),
                ty: Ty::Int,
                auto_assign: false,
                default: None,
                span: span(),
            },
            HirParam {
                def_id: b_def,
                name: "b".to_string(),
                ty: Ty::Int,
                auto_assign: false,
                default: None,
                span: span(),
            },
        ];

        let func = simple_func(0, "add", params, Ty::Int, body);
        let program = program_with_fn(func);

        let mut lowerer = Lowerer::new(&symbols);
        let mir = lowerer.lower_program(&program).expect("lowering failed");

        let add_fn = mir
            .functions
            .iter()
            .find(|f| f.name == "add")
            .expect("expected fn `add` in MIR");
        assert_eq!(add_fn.name, "add");
        assert_eq!(add_fn.params.len(), 2, "add takes 2 params");
        assert_eq!(add_fn.return_ty, Ty::Int);

        // Params should be local ids 0 and 1.
        assert_eq!(add_fn.params[0], 0);
        assert_eq!(add_fn.params[1], 1);

        // Check that param locals have correct names.
        assert_eq!(add_fn.locals[0].name, "a");
        assert_eq!(add_fn.locals[1].name, "b");

        // Entry block should contain a BinOp(Add) instruction.
        let entry = &add_fn.blocks[add_fn.entry_block];
        let has_binop = entry
            .instructions
            .iter()
            .any(|inst| matches!(inst, MirInst::BinOp { op: BinOp::Add, .. }));
        assert!(has_binop, "entry block should contain a BinOp(Add)");

        // Terminator should be Return(Some(...)).
        match &entry.terminator {
            Terminator::Return(Some(_)) => {} // correct
            other => panic!("expected Return(Some(...)), got {:?}", other),
        }
    }

    // ── Test 4: if/else control flow ────────────────────────────────────

    #[test]
    fn lower_if_else_produces_branch() {
        let symbols = SymbolTable::new();

        // Body: if true { 1 } else { 2 }
        let body = expr(
            HirExprKind::Block(
                vec![],
                Some(Box::new(expr(
                    HirExprKind::If {
                        cond: Box::new(expr(HirExprKind::BoolLiteral(true), Ty::Bool)),
                        then_branch: Box::new(expr(
                            HirExprKind::Block(
                                vec![],
                                Some(Box::new(expr(HirExprKind::IntLiteral(1), Ty::Int))),
                            ),
                            Ty::Int,
                        )),
                        else_branch: Some(Box::new(expr(
                            HirExprKind::Block(
                                vec![],
                                Some(Box::new(expr(HirExprKind::IntLiteral(2), Ty::Int))),
                            ),
                            Ty::Int,
                        ))),
                    },
                    Ty::Int,
                ))),
            ),
            Ty::Int,
        );

        let func = simple_func(0, "test_if", vec![], Ty::Int, body);
        let program = program_with_fn(func);

        let mut lowerer = Lowerer::new(&symbols);
        let mir = lowerer.lower_program(&program).expect("lowering failed");

        let test_fn = mir
            .functions
            .iter()
            .find(|f| f.name == "test_if")
            .expect("expected fn `test_if` in MIR");
        assert_eq!(test_fn.name, "test_if");

        // Should have multiple blocks: entry, then, else, merge.
        assert!(
            test_fn.blocks.len() >= 4,
            "if/else should create at least 4 blocks, got {}",
            test_fn.blocks.len()
        );

        // Entry block terminator should be a Branch.
        let entry = &test_fn.blocks[test_fn.entry_block];
        assert!(
            matches!(entry.terminator, Terminator::Branch { .. }),
            "entry block should end with Branch, got {:?}",
            entry.terminator
        );
    }

    // ── Test 5: drops inserted for String locals ────────────────────────

    #[test]
    fn drops_inserted_for_owned_locals() {
        let mut symbols = SymbolTable::new();

        // DefId 0 = the function "test"
        let _fn_id = symbols.define(
            "test".to_string(),
            crate::resolve::symbols::DefKind::Function {
                signature: crate::resolve::symbols::FnSignature {
                    self_mode: None,
                    is_class_method: false,
                    is_async: false,
                    generic_params: vec![],
                    params: vec![],
                    return_ty: Ty::Unit,
                    c_symbol: None,
                },
            },
            Visibility::Public,
            span(),
        );

        let enum_ty = Ty::Enum {
            name: "Color".to_string(),
            generic_args: vec![],
        };

        // DefId 1 = the variable "c" (Enum — a Move type, always heap-allocated)
        let c_def = symbols.define(
            "c".to_string(),
            crate::resolve::symbols::DefKind::Variable {
                mutable: false,
                ty: enum_ty.clone(),
            },
            Visibility::Private,
            span(),
        );

        // Body: let c = <enum variant construction>
        // We use IntLiteral(0) as a stand-in since we just need some init value.
        let body = expr(
            HirExprKind::Block(
                vec![HirStatement::Let {
                    def_id: c_def,
                    pattern: HirPattern::Binding {
                        def_id: c_def,
                        name: "c".to_string(),
                        mutable: false,
                        span: span(),
                    },
                    ty: enum_ty.clone(),
                    value: Some(expr(
                        HirExprKind::EnumVariant {
                            type_def: 99,
                            type_name: "Color".to_string(),
                            variant_name: "Red".to_string(),
                            variant_idx: 0,
                            fields: vec![],
                        },
                        enum_ty.clone(),
                    )),
                    mutable: false,
                    span: span(),
                }],
                None,
            ),
            Ty::Unit,
        );

        let func = simple_func(0, "test", vec![], Ty::Unit, body);
        let program = program_with_fn(func);

        let mut lowerer = Lowerer::new(&symbols);
        let mir = lowerer.lower_program(&program).expect("lowering failed");
        let test_fn = mir
            .functions
            .iter()
            .find(|f| f.name == "test")
            .expect("expected fn `test` in MIR (drops_inserted_for_owned_locals)");

        // Find the block that ends with Return.
        let return_block = test_fn
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Return(_)))
            .expect("should have a Return block");

        // The return block should contain at least one Drop instruction.
        let has_drop = return_block
            .instructions
            .iter()
            .any(|inst| matches!(inst, MirInst::Drop { .. }));
        assert!(
            has_drop,
            "Enum local should have a Drop instruction before Return"
        );

        // Verify the Drop targets the enum local, not a Copy local.
        let drop_locals: Vec<u32> = return_block
            .instructions
            .iter()
            .filter_map(|inst| match inst {
                MirInst::Drop { local } => Some(*local),
                _ => None,
            })
            .collect();

        // All dropped locals should be Move-type (Enum).
        for &lid in &drop_locals {
            let local = &test_fn.locals[lid as usize];
            assert!(
                local.ty.is_move(),
                "dropped local '{}' (ty={:?}) should be Move type",
                local.name,
                local.ty
            );
        }

        // Verify no Copy-type locals are dropped.
        let int_local_dropped = return_block.instructions.iter().any(|inst| {
            if let MirInst::Drop { local } = inst {
                test_fn.locals[*local as usize].ty.is_copy()
            } else {
                false
            }
        });
        assert!(!int_local_dropped, "Copy-type locals should not be dropped");
    }

    #[test]
    fn derive_copy_struct_is_dropped_when_heap_allocated() {
        // Post ruby-naming.spec.md §3.6: `Copy` is implicit for structs
        // whose every field is Copy. The Copy include changes
        // copy-on-assign semantics but does NOT change codegen's
        // heap-allocation strategy — `Point.new(...)` is still lowered
        // through `MirInst::Alloc`. Therefore drop-elaboration must
        // still emit `MirInst::Drop` for heap-allocated Copy aggregates
        // so the matching `ruxen_dealloc` runs at scope exit (otherwise
        // the heap leaks). The pre-spec behaviour ("Copy locals are
        // never dropped") only held while codegen also stack-allocated
        // such values; that invariant is gone.
        derive_copy_struct_is_dropped_inner();
    }

    fn derive_copy_struct_is_dropped_inner() {
        let mut symbols = SymbolTable::new();

        let _point_def = symbols.define(
            "Point".to_string(),
            crate::resolve::symbols::DefKind::Struct {
                info: crate::resolve::symbols::StructInfo {
                    generic_params: vec![],
                    fields: vec![],
                    derive_traits: vec!["Copy".to_string(), "Clone".to_string()],
                    layout: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                    const_predicates: vec![],
                },
            },
            Visibility::Public,
            span(),
        );

        let point_ty = Ty::Struct {
            name: "Point".to_string(),
            generic_args: vec![],
        };

        let point_def = symbols.define(
            "p".to_string(),
            crate::resolve::symbols::DefKind::Variable {
                mutable: false,
                ty: point_ty.clone(),
            },
            Visibility::Private,
            span(),
        );

        let body = expr(
            HirExprKind::Block(
                vec![HirStatement::Let {
                    def_id: point_def,
                    pattern: HirPattern::Binding {
                        def_id: point_def,
                        name: "p".to_string(),
                        mutable: false,
                        span: span(),
                    },
                    ty: point_ty.clone(),
                    value: Some(expr(
                        HirExprKind::Construct {
                            type_def: 0,
                            type_name: "Point".to_string(),
                            fields: vec![],
                        },
                        point_ty.clone(),
                    )),
                    mutable: false,
                    span: span(),
                }],
                None,
            ),
            Ty::Unit,
        );

        let func = simple_func(0, "test", vec![], Ty::Unit, body);
        let program = program_with_fn(func);

        let mut lowerer = Lowerer::new(&symbols);
        let mir = lowerer.lower_program(&program).expect("lowering failed");
        let test_fn = mir
            .functions
            .iter()
            .find(|f| f.name == "test")
            .expect("expected fn `test` in MIR (derive_copy_struct_is_not_dropped)");
        let return_block = test_fn
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Return(_)))
            .expect("should have a Return block");

        assert!(
            return_block
                .instructions
                .iter()
                .any(|inst| matches!(inst, MirInst::Drop { .. })),
            "heap-allocated Copy struct local must still receive a Drop \
             instruction so codegen emits the matching `ruxen_dealloc`"
        );
    }

    #[test]
    fn owned_rebind_lowers_to_move() {
        let mut symbols = SymbolTable::new();

        // ruby-naming.spec.md §3.6 makes Copy implicit when *every* field
        // is Copy. To keep this fixture firmly in "non-Copy" territory we
        // register a `String` field on `Pair` — String is owned and not
        // Copy, so the struct cannot be implicitly Copy.
        let string_field = symbols.define(
            "label".to_string(),
            crate::resolve::symbols::DefKind::Field {
                parent: 0,
                ty: Ty::String,
                index: 0,
            },
            Visibility::Public,
            span(),
        );

        symbols.define(
            "Pair".to_string(),
            crate::resolve::symbols::DefKind::Struct {
                info: crate::resolve::symbols::StructInfo {
                    generic_params: vec![],
                    fields: vec![string_field],
                    derive_traits: vec![],
                    layout: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                    const_predicates: vec![],
                },
            },
            Visibility::Public,
            span(),
        );

        let pair_ty = Ty::Struct {
            name: "Pair".to_string(),
            generic_args: vec![],
        };

        let a_def = symbols.define(
            "a".to_string(),
            crate::resolve::symbols::DefKind::Variable {
                mutable: false,
                ty: pair_ty.clone(),
            },
            Visibility::Private,
            span(),
        );
        let b_def = symbols.define(
            "b".to_string(),
            crate::resolve::symbols::DefKind::Variable {
                mutable: false,
                ty: pair_ty.clone(),
            },
            Visibility::Private,
            span(),
        );

        let body = expr(
            HirExprKind::Block(
                vec![
                    HirStatement::Let {
                        def_id: a_def,
                        pattern: HirPattern::Binding {
                            def_id: a_def,
                            name: "a".to_string(),
                            mutable: false,
                            span: span(),
                        },
                        ty: pair_ty.clone(),
                        value: Some(expr(
                            HirExprKind::Construct {
                                type_def: 0,
                                type_name: "Pair".to_string(),
                                fields: vec![],
                            },
                            pair_ty.clone(),
                        )),
                        mutable: false,
                        span: span(),
                    },
                    HirStatement::Let {
                        def_id: b_def,
                        pattern: HirPattern::Binding {
                            def_id: b_def,
                            name: "b".to_string(),
                            mutable: false,
                            span: span(),
                        },
                        ty: pair_ty.clone(),
                        value: Some(expr(HirExprKind::VarRef(a_def), pair_ty.clone())),
                        mutable: false,
                        span: span(),
                    },
                ],
                None,
            ),
            Ty::Unit,
        );

        let func = simple_func(0, "test", vec![], Ty::Unit, body);
        let program = program_with_fn(func);

        let mut lowerer = Lowerer::new(&symbols);
        let mir = lowerer.lower_program(&program).expect("lowering failed");
        let test_fn = mir
            .functions
            .iter()
            .find(|f| f.name == "test")
            .expect("expected fn `test` in MIR (owned_rebind_lowers_to_move)");
        let a_local = test_fn
            .locals
            .iter()
            .find(|local| local.name == "a")
            .expect("missing local a")
            .id;
        let b_local = test_fn
            .locals
            .iter()
            .find(|local| local.name == "b")
            .expect("missing local b")
            .id;

        assert!(
            test_fn.blocks.iter().any(|block| {
                block.instructions.iter().any(|inst| {
                matches!(inst, MirInst::Move { dest, src } if *dest == b_local && *src == a_local)
            })
            }),
            "owned let rebinding should lower to MirInst::Move"
        );
    }

    // ── Test 6: function call lowering ──────────────────────────────────

    #[test]
    fn lower_function_call() {
        let symbols = SymbolTable::new();

        // Body: foo(10, 20) — we just reference by name, no DefId resolution needed.
        let body = expr(
            HirExprKind::Block(
                vec![],
                Some(Box::new(expr(
                    HirExprKind::FnCall {
                        callee: 99, // doesn't matter for lowering
                        callee_name: "foo".to_string(),
                        args: vec![
                            expr(HirExprKind::IntLiteral(10), Ty::Int),
                            expr(HirExprKind::IntLiteral(20), Ty::Int),
                        ],
                    },
                    Ty::Int,
                ))),
            ),
            Ty::Int,
        );

        let func = simple_func(0, "caller", vec![], Ty::Int, body);
        let program = program_with_fn(func);

        let mut lowerer = Lowerer::new(&symbols);
        let mir = lowerer.lower_program(&program).expect("lowering failed");

        let caller_fn = mir
            .functions
            .iter()
            .find(|f| f.name == "caller")
            .expect("expected fn `caller` in MIR");
        let entry = &caller_fn.blocks[caller_fn.entry_block];

        // Should have a Call instruction to "foo".
        let has_call = entry
            .instructions
            .iter()
            .any(|inst| matches!(inst, MirInst::Call { callee, .. } if callee == "foo"));
        assert!(has_call, "should emit a Call to 'foo'");
    }
}

// ─── Lowerer helper tests ──────────────────────────────────────────────────
//
// These tests target private `Lowerer` helpers that cannot be reached from
// integration tests in `crates/ruxen-core/tests/`. They drive the helper
// after running the real lex / parse / typeck / lower pipeline, so the
// `Lowerer`'s internal state (e.g. `trait_impls`) is populated as it would
// be during a real compilation.

#[cfg(test)]
mod helper_tests {
    use crate::diagnostics::DiagnosticLevel;
    use crate::hir::types::Ty;
    use crate::lexer::Lexer;
    use crate::mir::lower::Lowerer;
    use crate::parser::Parser;
    use crate::typeck;

    fn rx(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ruxen")
            .join(format!("{name}.rx"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
    }

    /// Phase 2 #06.D2.S2: direct unit test for `Lowerer::user_has_impl_display`.
    ///
    /// Covers the three observable cases of the helper:
    /// 1. A user `class Money` with `impl Display for Money` → returns
    ///    `Some("Money")`.
    /// 2. A primitive `Ty::Int` → returns `None` (the early-return arm for
    ///    types that are not Struct/Class/Enum/Ref/Alias/Newtype).
    /// 3. `Ty::Ref(Box::new(Class { name: "Money", .. }))` → returns
    ///    `Some("Money")` (proves the Ref-recursion path).
    ///
    /// The helper was previously only validated indirectly through the
    /// Stage-3 behaviour test `user_impl_display_lowers_t_fmt_function` in
    /// `tests/stdlib_fmt_display_dispatch.rs`. This direct test closes the
    /// spec gap surfaced in the Stage 2 spec review.
    #[test]
    fn user_has_impl_display_resolves_class_int_and_ref() {
        // Canonical impl-Display fixture (mirrors `tests/stdlib_fmt.rs`).
        // `class Money` declares a single `cents: Int` field and an
        // `impl Display` block whose `def fmt` matches the
        // `fmt(&mut Formatter) -> Result[(), FmtError]` contract.
        let src = rx("user_has_impl_display_resolves_class_int_and_ref");

        // Drive the real pipeline so `trait_impls` is populated exactly as
        // it would be in compilation.
        let mut lexer = Lexer::new(&src);
        let tokens = lexer.tokenize().expect("lex should succeed");
        let mut parser = Parser::new(tokens);
        let program = parser.parse().expect("parse should succeed");
        let tc = typeck::type_check(&program);
        assert!(
            tc.diagnostics
                .iter()
                .all(|d| d.level != DiagnosticLevel::Error),
            "fixture must typecheck cleanly, got: {:?}",
            tc.diagnostics
        );

        // `lower_program` calls `collect_trait_impls`, which is the
        // source-of-truth that `user_has_impl_display` consults.
        let mut lowerer = Lowerer::new(&tc.symbols);
        lowerer
            .lower_program(&tc.program)
            .expect("lower_program should succeed");

        // Case 1: `Money` class with an `impl Display` block.
        let money_ty = Ty::Class {
            name: "Money".to_string(),
            generic_args: vec![],
        };
        assert_eq!(
            lowerer.user_has_impl_display(&money_ty),
            Some("Money".to_string()),
            "class Money with impl Display must resolve to Some(\"Money\")"
        );

        // Case 2: primitive `Int` — not a Struct/Class/Enum, hits the
        // `_ => return None` arm directly.
        assert_eq!(
            lowerer.user_has_impl_display(&Ty::Int),
            None,
            "primitive Int has no impl Display, must return None"
        );

        // Case 3: `&Money` — exercises the `Ty::Ref(inner)` recursive
        // arm; the helper must peel the ref and find the impl on the
        // inner Class.
        let ref_money_ty = Ty::Ref(Box::new(Ty::Class {
            name: "Money".to_string(),
            generic_args: vec![],
        }));
        assert_eq!(
            lowerer.user_has_impl_display(&ref_money_ty),
            Some("Money".to_string()),
            "&Money must resolve via Ref-recursion to Some(\"Money\")"
        );
    }
}
