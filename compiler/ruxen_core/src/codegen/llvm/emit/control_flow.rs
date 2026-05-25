//! Terminator emission: returns, branches, switches, and unreachable.
//!
//! Translates each MIR `Terminator` into the corresponding LLVM terminator
//! instruction, closing out a basic block.

use super::*;

pub(super) fn translate_terminator<'ctx>(
    term: &Terminator,
    func: &MirFunction,
    local_allocas: &HashMap<LocalId, PointerValue<'ctx>>,
    block_map: &[BasicBlock<'ctx>],
    builder: &Builder<'ctx>,
    _module: &Module<'ctx>,
    context: &'ctx Context,
) -> Result<(), String> {
    match term {
        Terminator::Return(val) => {
            if func.name == "main" {
                let zero = context.i32_type().const_int(0, false);
                builder
                    .build_return(Some(&zero))
                    .map_err(|e| format!("Failed to build return: {:?}", e))?;
            } else {
                match val {
                    Some(v) => {
                        let ret_val = gen_value(v, func, local_allocas, builder, context)?;
                        if let Some(ret_ty) = ty_to_llvm(&func.return_ty, context) {
                            let ret_val = coerce_value(ret_val, ret_ty, builder);
                            builder
                                .build_return(Some(&ret_val))
                                .map_err(|e| format!("Failed to build return: {:?}", e))?;
                        } else {
                            builder
                                .build_return(Some(&ret_val))
                                .map_err(|e| format!("Failed to build return: {:?}", e))?;
                        }
                    }
                    None => {
                        if ty_to_llvm(&func.return_ty, context).is_some() {
                            // Non-void return type but no value — return zero
                            let ret_ty = ty_to_llvm(&func.return_ty, context).unwrap();
                            let zero = ret_ty.const_zero();
                            builder
                                .build_return(Some(&zero))
                                .map_err(|e| format!("Failed to build return: {:?}", e))?;
                        } else {
                            builder
                                .build_return(None)
                                .map_err(|e| format!("Failed to build void return: {:?}", e))?;
                        }
                    }
                }
            }
        }

        Terminator::Goto(target) => {
            builder
                .build_unconditional_branch(block_map[*target])
                .map_err(|e| format!("Failed to build goto: {:?}", e))?;
        }

        Terminator::Branch {
            cond,
            then_block,
            else_block,
        } => {
            let cond_val = gen_value(cond, func, local_allocas, builder, context)?;
            // Convert to i1 for LLVM's br instruction
            let cond_i1 = if cond_val.is_pointer_value() {
                // Pointer: compare != null
                let ptr_val = cond_val.into_pointer_value();
                let null = context.ptr_type(AddressSpace::default()).const_null();
                builder
                    .build_int_compare(
                        IntPredicate::NE,
                        builder
                            .build_ptr_to_int(ptr_val, context.i64_type(), "ptrtoint")
                            .unwrap(),
                        builder
                            .build_ptr_to_int(null, context.i64_type(), "nullint")
                            .unwrap(),
                        "tobool",
                    )
                    .map_err(|e| format!("Failed to build ptr compare: {:?}", e))?
            } else {
                // Integer (i8 bool): compare != 0
                let int_val = cond_val.into_int_value();
                builder
                    .build_int_compare(
                        IntPredicate::NE,
                        int_val,
                        int_val.get_type().const_zero(),
                        "tobool",
                    )
                    .map_err(|e| format!("Failed to build bool compare: {:?}", e))?
            };
            builder
                .build_conditional_branch(cond_i1, block_map[*then_block], block_map[*else_block])
                .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        }

        Terminator::Switch {
            value,
            targets,
            otherwise,
        } => {
            let val = gen_value(value, func, local_allocas, builder, context)?;
            let int_val = val.into_int_value();

            let cases: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = targets
                .iter()
                .map(|(disc, bid)| {
                    (
                        int_val.get_type().const_int(*disc as u64, true),
                        block_map[*bid],
                    )
                })
                .collect();

            let case_refs: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = cases;
            builder
                .build_switch(int_val, block_map[*otherwise], &case_refs)
                .map_err(|e| format!("Failed to build switch: {:?}", e))?;
        }

        Terminator::Unreachable => {
            builder
                .build_unreachable()
                .map_err(|e| format!("Failed to build unreachable: {:?}", e))?;
        }
    }

    Ok(())
}
