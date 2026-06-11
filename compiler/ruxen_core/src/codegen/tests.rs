#[cfg(test)]
mod tests {
    use crate::codegen::cranelift::CodeGen;
    use crate::codegen::layout::{layout_of, layout_struct_fields, TypeLayout};
    use crate::hir::types::Ty;
    use crate::mir::nodes::*;
    use crate::parser::ast::BinOp;
    use crate::resolve::symbols::SymbolTable;

    fn symbols() -> SymbolTable {
        SymbolTable::new()
    }

    // ─── Primitive types ────────────────────────────────────────────────────

    #[test]
    fn int_layout() {
        let layout = layout_of(&Ty::Int, &symbols());
        assert_eq!(layout.size, 8);
        assert_eq!(layout.alignment, 8);
        assert!(layout.field_offsets.is_empty());
    }

    #[test]
    fn bool_layout() {
        let layout = layout_of(&Ty::Bool, &symbols());
        assert_eq!(layout.size, 1);
        assert_eq!(layout.alignment, 1);
    }

    #[test]
    fn float64_layout() {
        let layout = layout_of(&Ty::Float64, &symbols());
        assert_eq!(layout.size, 8);
        assert_eq!(layout.alignment, 8);
    }

    #[test]
    fn unit_layout() {
        let layout = layout_of(&Ty::Unit, &symbols());
        assert_eq!(layout.size, 0);
        assert_eq!(layout.alignment, 1);
    }

    #[test]
    fn never_layout() {
        let layout = layout_of(&Ty::Never, &symbols());
        assert_eq!(layout.size, 0);
        assert_eq!(layout.alignment, 1);
    }

    #[test]
    fn char_layout() {
        let layout = layout_of(&Ty::Char, &symbols());
        assert_eq!(layout.size, 4);
        assert_eq!(layout.alignment, 4);
    }

    #[test]
    fn int8_layout() {
        let layout = layout_of(&Ty::Int8, &symbols());
        assert_eq!(layout.size, 1);
        assert_eq!(layout.alignment, 1);
    }

    #[test]
    fn int16_layout() {
        let layout = layout_of(&Ty::Int16, &symbols());
        assert_eq!(layout.size, 2);
        assert_eq!(layout.alignment, 2);
    }

    #[test]
    fn int32_layout() {
        let layout = layout_of(&Ty::Int32, &symbols());
        assert_eq!(layout.size, 4);
        assert_eq!(layout.alignment, 4);
    }

    #[test]
    fn float32_layout() {
        let layout = layout_of(&Ty::Float32, &symbols());
        assert_eq!(layout.size, 4);
        assert_eq!(layout.alignment, 4);
    }

    // ─── Reference types ────────────────────────────────────────────────────

    #[test]
    fn ref_layout() {
        // &Int is a thin pointer: 8 bytes, align 8
        let layout = layout_of(&Ty::Ref(Box::new(Ty::Int)), &symbols());
        assert_eq!(layout.size, 8);
        assert_eq!(layout.alignment, 8);
    }

    #[test]
    fn ref_mut_layout() {
        let layout = layout_of(&Ty::RefMut(Box::new(Ty::Bool)), &symbols());
        assert_eq!(layout.size, 8);
        assert_eq!(layout.alignment, 8);
    }

    #[test]
    fn ref_lifetime_layout() {
        let layout = layout_of(
            &Ty::RefLifetime("a".to_string(), Box::new(Ty::Int)),
            &symbols(),
        );
        assert_eq!(layout.size, 8);
        assert_eq!(layout.alignment, 8);
    }

    // ─── String types ────────────────────────────────────────────────────────

    #[test]
    fn string_layout() {
        // String = ptr + len + cap → 24 bytes, align 8
        let layout = layout_of(&Ty::String, &symbols());
        assert_eq!(layout.size, 24);
        assert_eq!(layout.alignment, 8);
    }

    // (The `str_layout` test was removed with the `&str` type — there is one
    // string type, `String`, exercised by `string_layout` above.
    // one-string-type ADR.)

    // ─── Collection types ────────────────────────────────────────────────────

    #[test]
    fn vec_layout() {
        // Vec[Int] = ptr + len + cap → 24 bytes, align 8
        let layout = layout_of(&Ty::Array(Box::new(Ty::Int)), &symbols());
        assert_eq!(layout.size, 24);
        assert_eq!(layout.alignment, 8);
    }

    #[test]
    fn hash_layout() {
        let layout = layout_of(
            &Ty::Map(Box::new(Ty::String), Box::new(Ty::Int)),
            &symbols(),
        );
        assert_eq!(layout.size, 48);
        assert_eq!(layout.alignment, 8);
    }

    #[test]
    fn set_layout() {
        let layout = layout_of(&Ty::Set(Box::new(Ty::Int)), &symbols());
        assert_eq!(layout.size, 48);
        assert_eq!(layout.alignment, 8);
    }

    // ─── Tuple types ────────────────────────────────────────────────────────

    #[test]
    fn tuple_layout_with_padding() {
        // (Bool, Int) → Bool(1) + 7 bytes padding + Int(8) = 16 bytes
        // offsets: [0, 8], align 8
        let layout = layout_of(&Ty::Tuple(vec![Ty::Bool, Ty::Int]), &symbols());
        assert_eq!(layout.size, 16);
        assert_eq!(layout.alignment, 8);
        assert_eq!(layout.field_offsets, vec![0, 8]);
    }

    #[test]
    fn tuple_no_padding() {
        // (Int, Int) → 8 + 8 = 16 bytes, offsets [0, 8], align 8
        let layout = layout_of(&Ty::Tuple(vec![Ty::Int, Ty::Int]), &symbols());
        assert_eq!(layout.size, 16);
        assert_eq!(layout.alignment, 8);
        assert_eq!(layout.field_offsets, vec![0, 8]);
    }

    #[test]
    fn tuple_empty() {
        // () is just Unit
        let layout = layout_of(&Ty::Tuple(vec![]), &symbols());
        assert_eq!(layout.size, 0);
        assert_eq!(layout.alignment, 1);
    }

    #[test]
    fn tuple_single() {
        // (Int,) → 8 bytes, align 8, offsets [0]
        let layout = layout_of(&Ty::Tuple(vec![Ty::Int]), &symbols());
        assert_eq!(layout.size, 8);
        assert_eq!(layout.alignment, 8);
        assert_eq!(layout.field_offsets, vec![0]);
    }

    // ─── Array types ────────────────────────────────────────────────────────

    #[test]
    fn array_layout() {
        // [Int; 4] → 8 * 4 = 32 bytes, align 8
        let layout = layout_of(
            &Ty::FixedArray(Box::new(Ty::Int), crate::hir::types::ConstExpr::Lit(4)),
            &symbols(),
        );
        assert_eq!(layout.size, 32);
        assert_eq!(layout.alignment, 8);
    }

    #[test]
    fn array_zero_size() {
        // [Int; 0] → 0 bytes, align 8
        let layout = layout_of(
            &Ty::FixedArray(Box::new(Ty::Int), crate::hir::types::ConstExpr::Lit(0)),
            &symbols(),
        );
        assert_eq!(layout.size, 0);
        assert_eq!(layout.alignment, 8);
    }

    #[test]
    fn array_bool() {
        // [Bool; 8] → 8 bytes, align 1
        let layout = layout_of(
            &Ty::FixedArray(Box::new(Ty::Bool), crate::hir::types::ConstExpr::Lit(8)),
            &symbols(),
        );
        assert_eq!(layout.size, 8);
        assert_eq!(layout.alignment, 1);
    }

    // ─── Option / Result ─────────────────────────────────────────────────────

    #[test]
    fn option_bool_layout() {
        // Option[Bool]: tag(4) + pad(0) + Bool(1) → aligned to 4 → size 8, align 4
        let layout = layout_of(&Ty::Option(Box::new(Ty::Bool)), &symbols());
        // tag=4, align=max(4,1)=4, payload_offset=4, total=align_up(4+1,4)=8
        assert_eq!(layout.alignment, 4);
        assert_eq!(layout.size, 8);
    }

    #[test]
    fn option_int_layout() {
        // Option[Int]: tag(4) + pad(4) + Int(8) → total=16, align=8
        let layout = layout_of(&Ty::Option(Box::new(Ty::Int)), &symbols());
        assert_eq!(layout.alignment, 8);
        assert_eq!(layout.size, 16);
    }

    #[test]
    fn result_layout() {
        // Result[Int, Bool]: tag(4) + pad(4) + max(Int=8, Bool=1)=8 → total=16, align=8
        let layout = layout_of(
            &Ty::Result(Box::new(Ty::Int), Box::new(Ty::Bool)),
            &symbols(),
        );
        assert_eq!(layout.alignment, 8);
        assert_eq!(layout.size, 16);
    }

    // ─── Function types ──────────────────────────────────────────────────────

    #[test]
    fn fn_type_layout() {
        let layout = layout_of(
            &Ty::Fn {
                params: vec![Ty::Int],
                ret: Box::new(Ty::Bool),
            },
            &symbols(),
        );
        assert_eq!(layout.size, 8);
        assert_eq!(layout.alignment, 8);
    }

    #[test]
    fn dyn_trait_layout() {
        let layout = layout_of(&Ty::AnyMixin(vec![]), &symbols());
        assert_eq!(layout.size, 16);
        assert_eq!(layout.alignment, 8);
    }

    // ─── layout_struct_fields helper ─────────────────────────────────────────

    #[test]
    fn struct_fields_mixed_alignment() {
        // [Bool(1,1), Int(8,8)] → offset 0, then padded to 8 → offset 8, total 16
        let fields = vec![
            TypeLayout {
                size: 1,
                alignment: 1,
                field_offsets: vec![],
            },
            TypeLayout {
                size: 8,
                alignment: 8,
                field_offsets: vec![],
            },
        ];
        let layout = layout_struct_fields(&fields);
        assert_eq!(layout.size, 16);
        assert_eq!(layout.alignment, 8);
        assert_eq!(layout.field_offsets, vec![0, 8]);
    }

    #[test]
    fn struct_fields_same_alignment() {
        // [Int(8,8), Int(8,8)] → offsets [0, 8], size 16, align 8
        let fields = vec![
            TypeLayout {
                size: 8,
                alignment: 8,
                field_offsets: vec![],
            },
            TypeLayout {
                size: 8,
                alignment: 8,
                field_offsets: vec![],
            },
        ];
        let layout = layout_struct_fields(&fields);
        assert_eq!(layout.size, 16);
        assert_eq!(layout.alignment, 8);
        assert_eq!(layout.field_offsets, vec![0, 8]);
    }

    #[test]
    fn struct_fields_empty() {
        let layout = layout_struct_fields(&[]);
        assert_eq!(layout.size, 0);
        assert_eq!(layout.alignment, 1);
        assert!(layout.field_offsets.is_empty());
    }

    // ─── Cranelift codegen tests ────────────────────────────────────────────────

    /// Helper: build a minimal MIR program with a single `main` function.
    fn make_main_program(locals: Vec<MirLocal>, blocks: Vec<BasicBlock>) -> MirProgram {
        MirProgram {
            functions: vec![MirFunction::with_parts(
                "main".to_string(),
                vec![],
                Ty::Unit,
                locals,
                blocks,
                0,
            )],
            entry: Some("main".to_string()),
            ffi_libs: vec![],
            vtables: Vec::new(),
            class_infos: Vec::new(),
        }
    }

    #[test]
    fn codegen_empty_main() {
        // A main function with a single block that returns nothing.
        let blocks = vec![BasicBlock {
            id: 0,
            instructions: vec![],
            terminator: Terminator::Return(None),
        }];

        let program = make_main_program(vec![], blocks);

        let mut codegen = CodeGen::new().expect("CodeGen::new failed");
        codegen
            .compile_program(&program)
            .expect("compile_program failed");
        let bytes = codegen.finish().expect("finish failed");
        assert!(
            !bytes.is_empty(),
            "Object file should contain at least some bytes"
        );
    }

    #[test]
    fn codegen_return_int() {
        // Function with two locals: a = 10, b = a + 32, then return.
        let locals = vec![
            MirLocal {
                id: 0,
                name: "a".to_string(),
                ty: Ty::Int,
                mutable: false,
            },
            MirLocal {
                id: 1,
                name: "b".to_string(),
                ty: Ty::Int,
                mutable: false,
            },
        ];

        let blocks = vec![BasicBlock {
            id: 0,
            instructions: vec![
                MirInst::Assign {
                    dest: 0,
                    value: MirValue::Literal(Literal::Int(10)),
                },
                MirInst::BinOp {
                    dest: 1,
                    op: BinOp::Add,
                    lhs: MirValue::Use(0),
                    rhs: MirValue::Literal(Literal::Int(32)),
                },
            ],
            terminator: Terminator::Return(None),
        }];

        let program = make_main_program(locals, blocks);

        let mut codegen = CodeGen::new().expect("CodeGen::new failed");
        codegen
            .compile_program(&program)
            .expect("compile_program failed");
        let bytes = codegen.finish().expect("finish failed");
        assert!(
            !bytes.is_empty(),
            "Object file for int arithmetic should be non-empty"
        );
    }

    #[test]
    fn codegen_branch() {
        // Function with a branch: if cond goto block1 else block2; both return.
        let locals = vec![MirLocal {
            id: 0,
            name: "cond".to_string(),
            ty: Ty::Bool,
            mutable: false,
        }];

        let blocks = vec![
            BasicBlock {
                id: 0,
                instructions: vec![MirInst::Assign {
                    dest: 0,
                    value: MirValue::Literal(Literal::Bool(true)),
                }],
                terminator: Terminator::Branch {
                    cond: MirValue::Use(0),
                    then_block: 1,
                    else_block: 2,
                },
            },
            BasicBlock {
                id: 1,
                instructions: vec![],
                terminator: Terminator::Return(None),
            },
            BasicBlock {
                id: 2,
                instructions: vec![],
                terminator: Terminator::Return(None),
            },
        ];

        let program = make_main_program(locals, blocks);

        let mut codegen = CodeGen::new().expect("CodeGen::new failed");
        codegen
            .compile_program(&program)
            .expect("compile_program failed");
        let bytes = codegen.finish().expect("finish failed");
        assert!(
            !bytes.is_empty(),
            "Object file with branches should be non-empty"
        );
    }
}
