//! Make every value conversion explicit MIR (F-29 P1b).
//!
//! The last step of lowering, after drop elaboration: wherever a value
//! crosses into a slot of another machine class, insert one
//! `MirInst::Convert` carrying the [`Coercion`] `mir::abi` decides. A
//! backend then emits each `Convert` as given and otherwise trusts operand
//! classes; it decides no coercion of its own. The crossings:
//!
//! - `Assign { dest, value }`: the value into `dest`'s class
//!   ([`abi::assign_mode`]); the `Assign` becomes the `Convert`.
//! - a call argument into the callee's parameter class
//!   ([`abi::call_arg_mode`], the slot from [`CalleeIndex`]); a callee no
//!   declaration describes takes its argument as passed, a narrow integer
//!   widened to the `I64` word.
//! - a returned value into the function's return class ([`abi::return_mode`]).
//! - the operands of a `BinOp` or a non-string `Compare` to one class
//!   ([`abi::unify_operands`]); a string `Compare`'s operands to the `Ptr`
//!   the runtime comparison takes.
//! - an instruction's result into its `dest` ([`abi::produced`]): an
//!   operation, a call or an address lands in a fresh temporary of the
//!   class it produces, converted into `dest` right after; a `Copy` or
//!   `Ref` across classes, and every `FloatToBits` / `BitsToFloat`, becomes
//!   the `Convert` itself.
//!
//! A passthrough pseudo-call ([`NOOP_PASSTHROUGH`]) is its first argument,
//! so it becomes an `Assign` of it, crossing as any `Assign` does.
//!
//! One linear walk over each body's instructions; a block is rebuilt only
//! when it gains a conversion. Each inserted `Convert` carries the span of
//! the instruction it serves.

use crate::lexer::token::Span;

use super::abi::{self, AbiClass, CalleeIndex, Coercion, Mode, Unify};
use super::lang_intrinsics::{runtime_name, NOOP_PASSTHROUGH};
use super::nodes::{Literal, LocalId, MirFunction, MirInst, MirProgram, MirValue, Terminator};

/// Build `program.callees` and insert the `Convert` of every value
/// crossing in `program`.
pub fn make_coercions_explicit(program: &mut MirProgram) {
    program.callees = CalleeIndex::for_program(program);
    for func in &mut program.functions {
        make_explicit_in(func, &program.callees);
    }
}

fn make_explicit_in(func: &mut MirFunction, callees: &CalleeIndex) {
    let mut blocks = std::mem::take(&mut func.blocks);
    for block in &mut blocks {
        // Conversions to insert: (before the instruction at this index,
        // with the span of the instruction at this index, conversion).
        let mut before: Vec<(usize, usize, MirInst)> = Vec::new();
        for (index, inst) in block.instructions.iter_mut().enumerate() {
            if let MirInst::Call { dest, callee, args } = inst {
                if runtime_name(callee) == Ok(NOOP_PASSTHROUGH) {
                    *inst = match *dest {
                        Some(dest) => MirInst::Assign {
                            dest,
                            value: args
                                .first()
                                .cloned()
                                .unwrap_or(MirValue::Literal(Literal::Int(0))),
                        },
                        None => MirInst::Nop,
                    };
                }
            }
            let mut convert = |func: &mut MirFunction, value: &mut MirValue, to, mode| {
                if let Some(inst) = convert_operand(func, value, to, mode) {
                    before.push((index, index, inst));
                }
            };
            match inst {
                MirInst::Assign { dest, value } => {
                    let (from, to) = (abi::value_class(value, func), local_class(func, *dest));
                    if from != to {
                        let mode = abi::assign_mode(value, *dest, func, from, to);
                        if let Some(coercion) = needed(abi::coercion(from, to, mode)) {
                            *inst = MirInst::Convert {
                                dest: *dest,
                                src: value.clone(),
                                coercion,
                            };
                        }
                    }
                }
                MirInst::BinOp { lhs, rhs, .. } => unify(func, lhs, rhs, &mut convert),
                MirInst::Compare { lhs, rhs, .. } => {
                    if abi::is_string_operand(lhs, func) || abi::is_string_operand(rhs, func) {
                        convert(func, lhs, AbiClass::Ptr, Mode::UNSIGNED);
                        convert(func, rhs, AbiClass::Ptr, Mode::UNSIGNED);
                    } else {
                        unify(func, lhs, rhs, &mut convert);
                    }
                }
                MirInst::Call { callee, args, .. } if !args.is_empty() => {
                    // An unresolvable callee is the backend's error to report.
                    let Ok(name) = runtime_name(callee) else {
                        continue;
                    };
                    let params = callees.get(name).map_or(&[][..], |c| &c.abi.params);
                    for (i, arg) in args.iter_mut().enumerate() {
                        let from = abi::value_class(arg, func);
                        let param = params.get(i).copied();
                        let to = match param {
                            Some(p) => p.class,
                            None if from.is_int() && from.bits() < Some(64) => AbiClass::I64,
                            None => from,
                        };
                        if from != to {
                            let mode = abi::call_arg_mode(arg, func, from, to, param);
                            convert(func, arg, to, mode);
                        }
                    }
                }
                _ => {}
            }
            if let Some(convert) = land_result(func, inst, callees) {
                before.push((index + 1, index, convert));
            }
        }
        if let Terminator::Return(Some(value)) = &mut block.terminator {
            if let Some(to) = AbiClass::of(&func.return_ty).filter(|_| !func.is_entry) {
                let mode = abi::return_mode(func);
                if let Some(inst) = convert_operand(func, value, to, mode) {
                    // A terminator has no span of its own.
                    let end = block.instructions.len();
                    before.push((end, end, inst));
                }
            }
        }
        if !before.is_empty() {
            insert_before(&mut block.instructions, &mut block.inst_spans, before);
        }
    }
    func.blocks = blocks;
}

/// Bring a binary operation's operands to one class.
fn unify(
    func: &mut MirFunction,
    lhs: &mut MirValue,
    rhs: &mut MirValue,
    convert: &mut impl FnMut(&mut MirFunction, &mut MirValue, AbiClass, Mode),
) {
    let (l, r) = (abi::value_class(lhs, func), abi::value_class(rhs, func));
    if l == r {
        return;
    }
    match abi::unify_operands(l, r, lhs, rhs, func) {
        Unify::RhsToLhs(mode) => convert(func, rhs, l, mode),
        Unify::LhsToRhs(mode) => convert(func, lhs, r, mode),
    }
}

/// Bring `inst`'s result into its `dest`'s class ([`abi::produced`]).
/// A `Copy` or `Ref` that crosses classes, and every `FloatToBits` /
/// `BitsToFloat`, is replaced by its `Convert`. Any other crossing result
/// is retargeted into a fresh temporary of the class it produces, and the
/// `Convert` from it into `dest` is returned, to follow `inst`.
fn land_result(
    func: &mut MirFunction,
    inst: &mut MirInst,
    callees: &CalleeIndex,
) -> Option<MirInst> {
    let (from, mode) = abi::produced(inst, func, callees)?;
    let dest = inst.dest()?;
    let coercion = abi::coercion(from, local_class(func, dest), mode);
    let reinterpreted = match inst {
        MirInst::FloatToBits { src, .. } | MirInst::BitsToFloat { src, .. } => Some(*src),
        MirInst::Copy { src, .. } | MirInst::Ref { src, .. } => needed(coercion).map(|_| *src),
        _ => None,
    };
    if let Some(src) = reinterpreted {
        *inst = MirInst::Convert {
            dest,
            src: MirValue::Use(src),
            coercion,
        };
        return None;
    }
    let coercion = needed(coercion)?;
    let produced = func.new_temp(from.ty());
    *inst.dest_mut()? = produced;
    Some(MirInst::Convert {
        dest,
        src: MirValue::Use(produced),
        coercion,
    })
}

/// Convert `value` to class `to` under `mode` into a fresh temporary, point
/// `value` at it, and return the `Convert`; `None` when nothing is emitted.
fn convert_operand(
    func: &mut MirFunction,
    value: &mut MirValue,
    to: AbiClass,
    mode: Mode,
) -> Option<MirInst> {
    let coercion = needed(abi::coercion(abi::value_class(value, func), to, mode))?;
    let dest = func.new_temp(to.ty());
    let src = std::mem::replace(value, MirValue::Use(dest));
    Some(MirInst::Convert {
        dest,
        src,
        coercion,
    })
}

/// A coercion that emits an operation: neither `Same` nor `Keep`.
fn needed(coercion: Coercion) -> Option<Coercion> {
    (!matches!(coercion, Coercion::Same | Coercion::Keep)).then_some(coercion)
}

fn local_class(func: &MirFunction, id: LocalId) -> AbiClass {
    abi::value_class(&MirValue::Use(id), func)
}

/// Splice `before` (ascending by index) into `instructions`, each
/// `(at, span_of, conversion)` ahead of the instruction at `at` with the
/// span of the instruction at `span_of`. A block with spans keeps them
/// index-aligned; one without (a synthesized body) stays without.
fn insert_before(
    instructions: &mut Vec<MirInst>,
    spans: &mut Vec<Option<Span>>,
    before: Vec<(usize, usize, MirInst)>,
) {
    let with_spans = !spans.is_empty();
    let old_insts = std::mem::take(instructions);
    let old_spans = std::mem::take(spans);
    let span_at = |i: usize| old_spans.get(i).cloned().flatten();
    let total = old_insts.len() + before.len();
    instructions.reserve(total);
    if with_spans {
        spans.reserve(total);
    }
    let mut pending = before.into_iter().peekable();
    let mut push = |inst: MirInst, span_of: usize| {
        instructions.push(inst);
        if with_spans {
            spans.push(span_at(span_of));
        }
    };
    for (i, inst) in old_insts.into_iter().enumerate() {
        while let Some((_, span_of, convert)) = pending.next_if(|(at, ..)| *at == i) {
            push(convert, span_of);
        }
        push(inst, i);
    }
    // Conversions after the last instruction and for the terminator go
    // last; the terminator's have no span (`span_of` is past the end).
    for (_, span_of, convert) in pending {
        push(convert, span_of);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::types::Ty;

    /// `def widen(x: Int) -> Int` and a caller holding an `Int32` `small`
    /// and a `Float` `f`; the caller's single block is `body`.
    fn program(body: impl FnOnce(LocalId, LocalId) -> Vec<MirInst>) -> MirProgram {
        let mut widen = MirFunction::new("widen", Ty::Int);
        let x = widen.new_local("x", Ty::Int, false);
        widen.params.push(x);
        widen.blocks[0].terminator = Terminator::Return(Some(MirValue::Use(x)));

        let mut caller = MirFunction::new("caller", Ty::Unit);
        let small = caller.new_local("small", Ty::Int32, false);
        let f = caller.new_local("f", Ty::Float, false);
        for inst in body(small, f) {
            caller.blocks[0].push_inst(inst, Some(Span::new(0, 0, 7, 1)));
        }
        caller.blocks[0].terminator = Terminator::Return(None);

        let mut program = MirProgram::new();
        program.functions = vec![widen, caller];
        make_coercions_explicit(&mut program);
        program
    }

    #[test]
    fn assign_across_classes_becomes_a_signed_conversion() {
        let program = program(|small, f| {
            vec![MirInst::Assign {
                dest: f,
                value: MirValue::Use(small),
            }]
        });
        let block = &program.functions[1].blocks[0];
        assert!(matches!(
            block.instructions[..],
            [MirInst::Convert {
                coercion: Coercion::IntToFloat { signed: true },
                ..
            }]
        ));
    }

    #[test]
    fn narrow_argument_widens_before_the_call_with_its_span() {
        let program = program(|small, _| {
            vec![MirInst::Call {
                dest: None,
                callee: "widen".to_string(),
                args: vec![MirValue::Use(small)],
            }]
        });
        let block = &program.functions[1].blocks[0];
        let [MirInst::Convert {
            dest,
            coercion: Coercion::Extend { signed: true },
            ..
        }, MirInst::Call { args, .. }] = &block.instructions[..]
        else {
            panic!("expected a Convert then the call: {:?}", block.instructions);
        };
        assert!(matches!(args[..], [MirValue::Use(arg)] if arg == *dest));
        assert_eq!(block.span_at(0).map(|s| s.line), Some(7));
        assert!(program.callees.get("widen").is_some());
    }

    #[test]
    fn call_result_lands_through_a_word_conversion_after_the_call() {
        let program = program(|small, f| {
            vec![MirInst::Call {
                dest: Some(f),
                callee: "widen".to_string(),
                args: vec![MirValue::Use(small)],
            }]
        });
        let block = &program.functions[1].blocks[0];
        let [MirInst::Convert { .. }, MirInst::Call {
            dest: Some(result), ..
        }, MirInst::Convert {
            src: MirValue::Use(src),
            coercion: Coercion::BitsToFloat { int: AbiClass::I64 },
            ..
        }] = &block.instructions[..]
        else {
            panic!(
                "expected the call result converted after the call: {:?}",
                block.instructions
            );
        };
        assert_eq!(result, src);
        assert_eq!(block.span_at(2).map(|s| s.line), Some(7));
    }

    #[test]
    fn comparison_result_widens_into_its_dest() {
        let program = program(|small, _| {
            vec![MirInst::Compare {
                dest: small,
                op: crate::mir::nodes::CmpOp::Eq,
                lhs: MirValue::Use(small),
                rhs: MirValue::Use(small),
            }]
        });
        let block = &program.functions[1].blocks[0];
        assert!(matches!(
            block.instructions[..],
            [
                MirInst::Compare { .. },
                MirInst::Convert {
                    coercion: Coercion::Extend { signed: false },
                    ..
                }
            ]
        ));
    }

    #[test]
    fn passthrough_call_becomes_an_assign_of_its_argument() {
        let program = program(|small, f| {
            vec![MirInst::Call {
                dest: Some(f),
                callee: "yield".to_string(),
                args: vec![MirValue::Use(small)],
            }]
        });
        let block = &program.functions[1].blocks[0];
        assert!(matches!(
            block.instructions[..],
            [MirInst::Convert {
                src: MirValue::Use(_),
                coercion: Coercion::IntToFloat { signed: true },
                ..
            }]
        ));
    }

    #[test]
    fn same_class_operands_are_untouched() {
        let program = program(|small, _| {
            vec![MirInst::Assign {
                dest: small,
                value: MirValue::Use(small),
            }]
        });
        let block = &program.functions[1].blocks[0];
        assert!(matches!(block.instructions[..], [MirInst::Assign { .. }]));
    }
}
