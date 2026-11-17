//! The one MIR walk every backend lowers through (F-29 P4).
//!
//! [`lower_function`] translates each `MirInst` and `Terminator` of a MIR
//! function into [`IrBuilder`] operations. It decides what every
//! instruction means at the machine level (the operation a `BinOp` is, the
//! runtime string comparison, field and payload offsets, the program
//! entry's exit status, a zero-sized parameter's binding); the builder only
//! spells those operations for its backend. Coercions arrive decided, as
//! `MirInst::Convert` (`mir::coercion`).

use crate::codegen::ir_builder::{FloatOp, IntOp, IrBuilder};
use crate::codegen::layout::field_slot_offset;
use crate::mir::abi::{self, AbiClass, Coercion};
use crate::mir::lang_intrinsics::runtime_name;
use crate::mir::nodes::*;
use crate::parser::ast::BinOp;

/// Byte offset of an enum's payload: past the 4-byte tag and its padding.
const ENUM_PAYLOAD_OFFSET: i64 = 8;

/// Lower `func` through `b`, a builder for that function.
pub fn lower_function<B: IrBuilder>(func: &MirFunction, b: &mut B) -> Result<(), String> {
    let locate = b.tracks_locations();
    // The definition line covers what has no span of its own (the frame,
    // drops, zero-init), so every emitted instruction has a location.
    let fn_line = func.def_span.as_ref().map_or(0, |s| s.line);
    if locate {
        b.set_location(fn_line, 1);
    }

    // Every local whose address a `RefMut` takes keeps addressable storage
    // for the whole frame; MIR emits `RefMut` only for a slot-addressed
    // `&var T` (`mir::refmut_abi`), so there is no type filter here.
    let mut address_taken = vec![false; func.locals.len()];
    for inst in func.blocks.iter().flat_map(|block| &block.instructions) {
        if let MirInst::RefMut { src, .. } = inst {
            address_taken[*src as usize] = true;
        }
    }
    for local in &func.locals {
        let class = local_class(func, local.id);
        b.declare_local(local.id, class, address_taken[local.id as usize]);
    }

    let blocks: Vec<B::Block> = func.blocks.iter().map(|_| b.create_block()).collect();
    let entry = blocks[func.entry_block];

    // A zero-sized (`Unit`/`Never`) parameter has no slot in the signature
    // (`FnAbi`), so the incoming values advance only for a sized one; the
    // body may still name a zero-sized one (`f.(self)` in a universal
    // `Kernel` body on a unit receiver), so it binds to a `0` word.
    let mut incoming = b.entry_params(entry)?.into_iter();
    for &param in &func.params {
        let value = if AbiClass::of(&func.locals[param as usize].ty).is_some() {
            incoming
                .next()
                .ok_or_else(|| format!("missing parameter {param} of function '{}'", func.name))?
        } else {
            b.iconst(AbiClass::I64, 0)
        };
        b.def_local(param, value)?;
    }
    b.enter_body(entry)?;

    for (idx, block) in func.blocks.iter().enumerate() {
        b.switch_to_block(blocks[idx]);
        if locate {
            b.set_location(fn_line, 1);
        }
        for (inst_idx, inst) in block.instructions.iter().enumerate() {
            if locate {
                let (line, column) = block
                    .span_at(inst_idx)
                    .map_or((fn_line, 1), |s| (s.line, s.column));
                b.set_location(line, column);
            }
            lower_instruction(inst, func, b).map_err(|e| {
                format!(
                    "Error in function '{}', block {}, instruction {:?}: {}",
                    func.name, idx, inst, e
                )
            })?;
        }
        lower_terminator(&block.terminator, func, &blocks, b)?;
    }
    Ok(())
}

/// The class of local `id` (the `I64` word for a type with none).
fn local_class(func: &MirFunction, id: LocalId) -> AbiClass {
    abi::value_class(&MirValue::Use(id), func)
}

/// The builder value of a MIR operand.
fn value<B: IrBuilder>(v: &MirValue, b: &mut B) -> Result<B::Value, String> {
    Ok(match v {
        MirValue::Literal(Literal::Int(n)) => b.iconst(AbiClass::I64, *n),
        MirValue::Literal(Literal::Float(f)) => b.f64const(*f),
        MirValue::Literal(Literal::Bool(v)) => b.iconst(AbiClass::I8, *v as i64),
        MirValue::Literal(Literal::Char(c)) => b.iconst(AbiClass::I32, *c as i64),
        // A string literal operand is never read; its text lowers through
        // `MirInst::StringLiteral`.
        MirValue::Literal(Literal::String(_)) => b.zero(AbiClass::Ptr),
        MirValue::Use(id) => b.use_local(*id)?,
        MirValue::Unit => b.iconst(AbiClass::I64, 0),
    })
}

fn values<B: IrBuilder>(vs: &[MirValue], b: &mut B) -> Result<Vec<B::Value>, String> {
    vs.iter().map(|v| value(v, b)).collect()
}

fn lower_instruction<B: IrBuilder>(
    inst: &MirInst,
    func: &MirFunction,
    b: &mut B,
) -> Result<(), String> {
    match inst {
        // A value of another class reaches `dest` through a `Convert`.
        MirInst::Assign { dest, value: v } => {
            let val = value(v, b)?;
            b.def_local(*dest, val)
        }

        MirInst::Convert {
            dest,
            src,
            coercion,
        } => {
            let val = value(src, b)?;
            let val = b.coerce(val, *coercion, local_class(func, *dest));
            b.def_local(*dest, val)
        }

        // MIR gave `dest` the class the operation produces.
        MirInst::BinOp { dest, op, lhs, rhs } => {
            let float = abi::value_class(lhs, func).is_float();
            let (l, r) = (value(lhs, b)?, value(rhs, b)?);
            let result = binop(*op, float, l, r, b)?;
            b.def_local(*dest, result)
        }

        MirInst::Negate { dest, operand } => {
            let val = value(operand, b)?;
            let result = if abi::value_class(operand, func).is_float() {
                b.float_negate(val)
            } else {
                b.int_negate(val)
            };
            b.def_local(*dest, result)
        }

        MirInst::Not { dest, operand } => {
            let val = value(operand, b)?;
            let one = b.iconst(abi::value_class(operand, func), 1);
            let result = b.int_op(IntOp::Xor, val, one);
            b.def_local(*dest, result)
        }

        // Every comparison produces the `I8` MIR gave `dest`.
        MirInst::Compare { dest, op, lhs, rhs } => {
            let (l, r) = (value(lhs, b)?, value(rhs, b)?);
            let result = if abi::is_string_operand(lhs, func) || abi::is_string_operand(rhs, func) {
                string_compare(*op, l, r, b)?
            } else if abi::value_class(lhs, func).is_float() {
                b.float_compare(*op, l, r)
            } else {
                b.int_compare(*op, l, r)
            };
            b.def_local(*dest, result)
        }

        MirInst::Call { dest, callee, args } => {
            // Each argument already has its parameter's class
            // (`mir::coercion`), so a signature inferred for an undeclared
            // callee sees those classes too.
            let args = values(args, b)?;
            let callee = b.callee(runtime_name(callee)?, &args, dest.is_some())?;
            let result = b.call(callee, &args)?;
            match dest {
                // MIR gave `dest` the callee's return class; a callee that
                // returns nothing leaves it zero.
                Some(dest) => {
                    let val = match result {
                        Some(val) => val,
                        None => b.zero(local_class(func, *dest)),
                    };
                    b.def_local(*dest, val)
                }
                None => Ok(()),
            }
        }

        MirInst::CallIndirect {
            dest, callee, args, ..
        } => {
            let callee = b.use_local(*callee)?;
            let args = values(args, b)?;
            // The signature returns `dest`'s own class: the closure returns
            // into it, and a mismatch reads garbage or traps (wasm).
            let ret = dest.and_then(|d| AbiClass::of(&func.locals[d as usize].ty));
            match (b.call_indirect(callee, &args, ret)?, dest) {
                (Some(val), Some(dest)) => b.def_local(*dest, val),
                _ => Ok(()),
            }
        }

        MirInst::Alloc { dest, ty, size } => {
            let size = if *size > 0 {
                *size
            } else {
                abi::simple_type_size(ty)
            };
            let alloc = b.runtime_callee("ruxen_alloc")?;
            let size = b.iconst(AbiClass::I64, size as i64);
            let ptr = b
                .call(alloc, &[size])?
                .ok_or("`ruxen_alloc` returns no pointer")?;
            b.def_local(*dest, ptr)
        }

        MirInst::StackAlloc { dest, .. } => {
            let zero = b.zero(local_class(func, *dest));
            b.def_local(*dest, zero)
        }

        // Field slot `field_index` is at `layout::field_slot_offset`.
        MirInst::GetField {
            dest,
            base,
            field_index,
        } => {
            let base = b.use_local(*base)?;
            let addr = b.ptr_offset(base, field_slot_offset(*field_index) as i64);
            let val = b.load(local_class(func, *dest), addr)?;
            b.def_local(*dest, val)
        }

        MirInst::SetField {
            base,
            field_index,
            value: v,
        } => {
            let base = b.use_local(*base)?;
            let val = value(v, b)?;
            let addr = b.ptr_offset(base, field_slot_offset(*field_index) as i64);
            b.store(addr, val)
        }

        MirInst::FieldAddr {
            dest,
            base,
            field_index,
        } => {
            let base = b.use_local(*base)?;
            let addr = b.ptr_offset(base, field_slot_offset(*field_index) as i64);
            b.def_local(*dest, addr)
        }

        // The tag is the `I32` at an enum's start.
        MirInst::SetTag { dest, tag } => {
            let ptr = b.use_local(*dest)?;
            let tag = b.iconst(AbiClass::I32, *tag as i64);
            b.store(ptr, tag)
        }

        MirInst::GetTag { dest, src } => {
            let ptr = b.use_local(*src)?;
            let tag = b.load(AbiClass::I32, ptr)?;
            b.def_local(*dest, tag)
        }

        MirInst::GetPayload { dest, src, .. } => {
            let ptr = b.use_local(*src)?;
            let payload = b.ptr_offset(ptr, ENUM_PAYLOAD_OFFSET);
            b.def_local(*dest, payload)
        }

        // A `&var T` of a slot-addressed `T` is the address of the source's
        // storage (`mir::refmut_abi`); MIR emits `RefMut` for no other type.
        MirInst::RefMut { dest, src } => {
            let addr = b.local_addr(*src)?;
            b.def_local(*dest, addr)
        }

        // An immutable borrow is the value itself (the borrow checker
        // enforces its semantics). A `Ref`, `Copy` or `Move` across classes
        // reaches codegen as a `Convert`.
        MirInst::Ref { dest, src } | MirInst::Copy { dest, src } => {
            let val = b.use_local(*src)?;
            b.def_local(*dest, val)
        }

        MirInst::Move { dest, src } => {
            let val = b.use_local(*src)?;
            b.def_local(*dest, val)?;
            let zero = b.zero(local_class(func, *src));
            b.def_local(*src, zero)
        }

        // A marker: the `ruxen_dealloc` call is emitted by drop elaboration
        // (`mir/lower`), gated by its flow analysis. Freeing here too would
        // double-free.
        MirInst::Drop { .. } => Ok(()),

        MirInst::StringLiteral { dest, value } => {
            let ptr = b.string_literal(value)?;
            b.def_local(*dest, ptr)
        }

        MirInst::FuncAddr { dest, func_name } => {
            let callee = b.callee(func_name, &[], true)?;
            let addr = b.func_addr(callee);
            b.def_local(*dest, addr)
        }

        // Phase B-5: a vtable / class_info symbol, declared before any body.
        MirInst::DataAddr { dest, data_sym } => {
            let addr = b.data_addr(data_sym)?;
            b.def_local(*dest, addr)
        }

        // `mir::coercion` (the last lowering step) replaces every one with
        // the `Convert` of its word reinterpretation.
        MirInst::FloatToBits { .. } | MirInst::BitsToFloat { .. } => {
            unreachable!("FloatToBits/BitsToFloat reach codegen as a `Convert`")
        }

        MirInst::Nop => Ok(()),
    }
}

/// A `BinOp` on two operands of one class (`float`: a float class).
fn binop<B: IrBuilder>(
    op: BinOp,
    float: bool,
    l: B::Value,
    r: B::Value,
    b: &mut B,
) -> Result<B::Value, String> {
    let compare = |cmp: CmpOp, b: &mut B| {
        if float {
            b.float_compare(cmp, l, r)
        } else {
            b.int_compare(cmp, l, r)
        }
    };
    let (int, float_op) = match op {
        BinOp::Add => (IntOp::Add, Some(FloatOp::Add)),
        BinOp::Sub => (IntOp::Sub, Some(FloatOp::Sub)),
        BinOp::Mul => (IntOp::Mul, Some(FloatOp::Mul)),
        BinOp::Div => (IntOp::Div, Some(FloatOp::Div)),
        BinOp::Mod => (IntOp::Rem, Some(FloatOp::Rem)),
        BinOp::BitAnd | BinOp::And | BinOp::KeywordAnd => (IntOp::And, None),
        BinOp::BitOr | BinOp::Or | BinOp::KeywordOr => (IntOp::Or, None),
        BinOp::BitXor => (IntOp::Xor, None),
        BinOp::Shl => (IntOp::Shl, None),
        BinOp::Shr => (IntOp::Shr, None),
        BinOp::Eq => return Ok(compare(CmpOp::Eq, b)),
        BinOp::NotEq => return Ok(compare(CmpOp::NotEq, b)),
        BinOp::Lt => return Ok(compare(CmpOp::Lt, b)),
        BinOp::LtEq => return Ok(compare(CmpOp::LtEq, b)),
        BinOp::Gt => return Ok(compare(CmpOp::Gt, b)),
        BinOp::GtEq => return Ok(compare(CmpOp::GtEq, b)),
        // MIR lowering (`mir/lower/expr/binops.rs`) desugars `=~` to a
        // method call, `**` and `<=>` to runtime calls and `??` to branches,
        // so none reaches codegen as a `BinOp`.
        BinOp::MatchOp => {
            unreachable!("BinOp::MatchOp should have been desugared to a method call at MIR-lower")
        }
        BinOp::Pow => {
            unreachable!("BinOp::Pow should have been lowered to a runtime call at MIR-lower")
        }
        BinOp::Cmp => {
            unreachable!("BinOp::Cmp should have been lowered to a runtime call at MIR-lower")
        }
        BinOp::NilCoalesce => {
            unreachable!("BinOp::NilCoalesce should have been lowered to branch CFG")
        }
    };
    match (float, float_op) {
        (false, _) => Ok(b.int_op(int, l, r)),
        (true, Some(op)) => b.float_op(op, l, r),
        (true, None) => Err(format!("`{op:?}` has no float form")),
    }
}

/// A comparison of two strings through the runtime: `ruxen_string_eq`
/// answers `1`/`0` and `ruxen_string_cmp` the C `strcmp` sign, both as an
/// `I64` the `Bool` result narrows from.
fn string_compare<B: IrBuilder>(
    op: CmpOp,
    l: B::Value,
    r: B::Value,
    b: &mut B,
) -> Result<B::Value, String> {
    let symbol = match op {
        CmpOp::Eq | CmpOp::NotEq => "ruxen_string_eq",
        CmpOp::Lt | CmpOp::LtEq | CmpOp::Gt | CmpOp::GtEq => "ruxen_string_cmp",
    };
    let callee = b.runtime_callee(symbol)?;
    let answer = b
        .call(callee, &[l, r])?
        .ok_or_else(|| format!("`{symbol}` returns no value"))?;
    Ok(match op {
        CmpOp::Eq => b.coerce(answer, Coercion::Truncate, AbiClass::I8),
        CmpOp::NotEq => {
            let zero = b.iconst(AbiClass::I64, 0);
            b.int_compare(CmpOp::Eq, answer, zero)
        }
        CmpOp::Lt | CmpOp::LtEq | CmpOp::Gt | CmpOp::GtEq => {
            let zero = b.iconst(AbiClass::I64, 0);
            b.int_compare(op, answer, zero)
        }
    })
}

fn lower_terminator<B: IrBuilder>(
    term: &Terminator,
    func: &MirFunction,
    blocks: &[B::Block],
    b: &mut B,
) -> Result<(), String> {
    match term {
        // The program entry returns the C `main` exit status (`FnAbi`).
        Terminator::Return(_) if func.is_entry => {
            let status = b.iconst(AbiClass::I32, 0);
            b.ret(Some(status))
        }
        // MIR converted the value to the return class.
        Terminator::Return(Some(v)) => {
            let val = value(v, b)?;
            b.ret(Some(val))
        }
        // A valueless return in a function that returns a value comes only
        // from an unreachable fallthrough block (the terminal block after
        // an `if let` whose arms all return); the signature still needs a
        // value of its class.
        Terminator::Return(None) => match AbiClass::of(&func.return_ty) {
            Some(class) => {
                let zero = b.zero(class);
                b.ret(Some(zero))
            }
            None => b.ret(None),
        },
        Terminator::Goto(target) => b.jump(blocks[*target]),
        Terminator::Branch {
            cond,
            then_block,
            else_block,
        } => {
            let cond = value(cond, b)?;
            b.branch(cond, blocks[*then_block], blocks[*else_block])
        }
        Terminator::Switch {
            value: v,
            targets,
            otherwise,
        } => {
            let val = value(v, b)?;
            let cases: Vec<(i64, B::Block)> = targets
                .iter()
                .map(|&(discriminant, target)| (discriminant, blocks[target]))
                .collect();
            b.switch(val, &cases, blocks[*otherwise])
        }
        Terminator::Unreachable => b.unreachable(),
    }
}
