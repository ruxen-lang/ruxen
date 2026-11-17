//! Closure pairs: the pair layout, the `__ruxen_closure_drop` helper, the
//! share callee, and which closure locals own (and drop) a pair.

use super::*;

/// True only when a by-value capture is never moved out of its captures slot.
/// Reads through the captured value are borrows; returns, aggregate stores,
/// explicit moves, and consuming/unknown calls make ownership unprovable and
/// suppress that field's closure-drop glue.
pub(in crate::mir::lower) fn capture_slot_still_owned(
    func: &MirFunction,
    captures: LocalId,
    slot: usize,
    symbols: &SymbolTable,
    ffi: &FfiByValue,
) -> bool {
    !value_escapes(func, HashSet::new(), Some((captures, slot)), symbols, ffi)
}

/// True when the closure pair in `pair` may outlive `func`'s body: it is
/// returned, stored into an aggregate, moved, or passed to a consuming or
/// unknown callee. Calling it (`GetField` + `CallIndirect`) and lending it to
/// a borrowing parameter (a `Fn[...]` block param) keep it local.
pub(in crate::mir::lower) fn closure_pair_escapes(
    func: &MirFunction,
    pair: LocalId,
    symbols: &SymbolTable,
    ffi: &FfiByValue,
) -> bool {
    value_escapes(func, HashSet::from([pair]), None, symbols, ffi)
}

/// The escape walk shared by [`capture_slot_still_owned`] and
/// [`closure_pair_escapes`]: start from `aliases` (plus every read of
/// `slot_of` = (captures pointer, slot), when given), follow `Assign`/`Copy`
/// aliases, and report whether any alias leaves `func`'s ownership.
pub(super) fn value_escapes(
    func: &MirFunction,
    mut aliases: HashSet<LocalId>,
    slot_of: Option<(LocalId, usize)>,
    symbols: &SymbolTable,
    ffi: &FfiByValue,
) -> bool {
    use crate::mir::lower::runtime_abi::{callee_ownership, user_callee_param_is_ref};

    let mut changed = true;
    while changed {
        changed = false;
        for block in &func.blocks {
            for inst in &block.instructions {
                match inst {
                    MirInst::GetField {
                        dest,
                        base,
                        field_index,
                    } if slot_of == Some((*base, *field_index)) => {
                        changed |= aliases.insert(*dest);
                    }
                    MirInst::Assign { dest, value } => {
                        if matches!(value, MirValue::Use(src) if aliases.contains(src)) {
                            changed |= aliases.insert(*dest);
                        }
                    }
                    MirInst::Copy { dest, src } => {
                        if aliases.contains(src) {
                            changed |= aliases.insert(*dest);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    for block in &func.blocks {
        for inst in &block.instructions {
            match inst {
                MirInst::Move { src, .. } if aliases.contains(src) => {
                    return true;
                }
                MirInst::Call { callee, args, .. } => {
                    let abi = callee_ownership(callee);
                    let param_is_ref = if abi.args_are_borrowed {
                        Vec::new()
                    } else {
                        user_callee_param_is_ref(symbols, callee, args.len(), None)
                    };
                    for (idx, arg) in args.iter().enumerate() {
                        let MirValue::Use(local) = arg else {
                            continue;
                        };
                        if !aliases.contains(local) {
                            continue;
                        }
                        let borrowed = (abi.borrows_first_arg && idx == 0)
                            || (!ffi.takes_by_value(callee, idx)
                                && (abi.args_are_borrowed
                                    || param_is_ref.get(idx).copied().unwrap_or(false)));
                        if !borrowed {
                            return true;
                        }
                    }
                }
                MirInst::CallIndirect { args, .. } => {
                    if args
                        .iter()
                        .any(|arg| matches!(arg, MirValue::Use(local) if aliases.contains(local)))
                    {
                        return true;
                    }
                }
                MirInst::SetField { value, .. } => {
                    if matches!(value, MirValue::Use(local) if aliases.contains(local)) {
                        return true;
                    }
                }
                MirInst::Drop { local } if aliases.contains(local) => return true,
                _ => {}
            }
        }
        if matches!(
            &block.terminator,
            Terminator::Return(Some(MirValue::Use(local))) if aliases.contains(local)
        ) {
            return true;
        }
    }
    false
}

#[derive(Clone)]
pub(super) struct ClosureDropGlue {
    pub(super) drop_fn: String,
    pub(super) owned_capture_slots: Vec<(usize, LocalId)>,
}

pub(super) fn closure_drop_callees(
    func: &MirFunction,
    dealloc_safe: &HashSet<LocalId>,
    return_locals: &HashSet<LocalId>,
    return_alias_chain: &HashSet<LocalId>,
) -> HashMap<LocalId, ClosureDropGlue> {
    let mut owners: HashMap<LocalId, ClosureDropGlue> = func
        .closure_drops
        .iter()
        .map(|drop| {
            (
                drop.local,
                ClosureDropGlue {
                    drop_fn: drop.drop_fn.clone(),
                    owned_capture_slots: drop.owned_capture_slots.clone(),
                },
            )
        })
        .collect();
    // A closure value a call hands over as a fresh owner (`p.clone`,
    // `ruxen_closure_share`) is released through its pair's drop slot, by
    // whichever local holds it last (kept only when that local is
    // dealloc-safe, below).
    for inst in func.blocks.iter().flat_map(|block| &block.instructions) {
        if let MirInst::Call {
            dest: Some(dest), ..
        } = inst
        {
            if is_owned_closure(&func.locals[*dest as usize].ty) {
                owners.entry(*dest).or_insert_with(|| ClosureDropGlue {
                    drop_fn: CLOSURE_DROP_HELPER.to_string(),
                    owned_capture_slots: Vec::new(),
                });
            }
        }
    }

    for block in &func.blocks {
        for inst in &block.instructions {
            let (dest, src) = match inst {
                MirInst::Assign {
                    dest,
                    value: MirValue::Use(src),
                }
                | MirInst::Copy { dest, src }
                | MirInst::Move { dest, src } => (*dest, *src),
                _ => continue,
            };
            if let Some(drop_fn) = owners.remove(&src) {
                owners.insert(dest, drop_fn);
            }
        }
    }

    owners.retain(|local, _| {
        dealloc_safe.contains(local)
            && !return_locals.contains(local)
            && !return_alias_chain.contains(local)
    });
    owners
}

// ─── Closure pairs ──────────────────────────────────────────────────────────
//
// A closure value is a pointer to a heap block of four words, built once per
// literal by `Lowerer::lower_closure` (expr/closure.rs):
//
//   [0] fn_ptr       the synthesized body, called with captures_ptr first
//   [1] captures_ptr one word per capture, or NULL
//   [2] drop_fn      that literal's `<closure>_drop`, so whoever owns the
//                    pair (a field, a collection slot) can free a closure
//                    whose literal it never saw
//   [3] shares       owners beyond the first; `.clone` of a Proc adds one,
//                    and `<closure>_drop` takes one away until none is left,
//                    then frees the owned captures, the captures box and
//                    the pair
//
// The C runtime reads the same slots through `RuxenClosurePair` and the
// `RUXEN_CLOSURE_*` defines (library/std/core/runtime/runtime.h); the test
// `closure_pair_slots_match_the_c_runtime` below checks the two agree.

/// Slot of the body's address in a closure pair.
pub(crate) const CLOSURE_PAIR_FN: usize = 0;
/// Slot of the captures box in a closure pair.
pub(crate) const CLOSURE_PAIR_CAPTURES: usize = 1;
/// Slot of the literal's `<closure>_drop` address in a closure pair.
pub(in crate::mir::lower) const CLOSURE_PAIR_DROP: usize = 2;
/// Slot of the extra-owner count in a closure pair.
pub(in crate::mir::lower) const CLOSURE_PAIR_SHARES: usize = 3;
/// Size in bytes of a closure pair.
pub(in crate::mir::lower) const CLOSURE_PAIR_SIZE: usize = 32;

/// `__ruxen_closure_drop(pair)`: release one owner of `pair` (NULL is a
/// no-op) by calling its drop slot. Synthesised on demand by
/// [`Lowerer::synthesize_closure_helpers`]; field glue and a dropped
/// `.clone` result call it.
pub(in crate::mir::lower) const CLOSURE_DROP_HELPER: &str = "__ruxen_closure_drop";
/// `ruxen_closure_share(pair) -> pair` (core runtime, `alloc.c`): add one
/// owner to `pair` (NULL is a no-op) and return it. `Proc#clone` is the
/// `.rx` declaration of it (`class Closure`, std/core); the derived `clone`
/// of a closure field calls it. Its declaration says `-> owned Self`, so its
/// result is caller-owned (`runtime_abi::result_ownership`); as a runtime
/// callee it only reads its argument.
pub(in crate::mir::lower) const CLOSURE_SHARE_FN: &str = "ruxen_closure_share";

/// True when a value of type `ty` owns a closure pair: a `Fn[...]`,
/// `Proc[...]` (`any Fn[...]`) or `some Fn[...]` held by value, not behind
/// a reference. A field or collection slot of such a type frees the
/// closure it holds.
pub(in crate::mir::lower) fn is_owned_closure(ty: &Ty) -> bool {
    !matches!(
        ty,
        Ty::Ref(_) | Ty::RefMut(_) | Ty::RefLifetime(..) | Ty::RefMutLifetime(..)
    ) && ty.is_callable()
}

/// The call that releases one owner of the closure pair in `pair`.
pub(in crate::mir::lower) fn closure_drop_call(pair: LocalId) -> MirInst {
    MirInst::Call {
        dest: None,
        callee: CLOSURE_DROP_HELPER.to_string(),
        args: vec![MirValue::Use(pair)],
    }
}

/// The call that adds one owner to the closure pair in `pair`, its result
/// in `dest`.
pub(in crate::mir::lower) fn closure_share_call(dest: LocalId, pair: LocalId) -> MirInst {
    MirInst::Call {
        dest: Some(dest),
        callee: CLOSURE_SHARE_FN.to_string(),
        args: vec![MirValue::Use(pair)],
    }
}

/// Define [`CLOSURE_DROP_HELPER`] when a body calls it. Runs once, after
/// every body and glue function is lowered.
pub(in crate::mir::lower) fn synthesize_closure_helpers(mir: &mut MirProgram) {
    let called = |mir: &MirProgram, name: &str| {
        mir.functions
            .iter()
            .flat_map(|f| f.blocks.iter().flat_map(|b| b.instructions.iter()))
            .any(|inst| matches!(inst, MirInst::Call { callee, .. } if callee == name))
    };
    if !mir
        .functions
        .iter()
        .any(|f| f.role == MirFnRole::ClosureDrop)
        && called(mir, CLOSURE_DROP_HELPER)
    {
        mir.functions.push(build_closure_drop_helper());
    }
}

/// `__ruxen_closure_drop(pair)`: `if pair != 0 { pair[2](pair) }`.
pub(super) fn build_closure_drop_helper() -> MirFunction {
    let mut f = MirFunction::new(CLOSURE_DROP_HELPER, Ty::Unit);
    f.role = MirFnRole::ClosureDrop;
    let pair = f.new_local("closure", Ty::Int, false);
    f.params = vec![pair];
    let entry = f.entry_block;
    let live_block = f.new_block();
    let done = f.new_block();
    let live = f.new_temp(Ty::Bool);
    f.blocks[entry].instructions.push(MirInst::Compare {
        dest: live,
        op: CmpOp::NotEq,
        lhs: MirValue::Use(pair),
        rhs: MirValue::Literal(Literal::Int(0)),
    });
    f.blocks[entry].terminator = Terminator::Branch {
        cond: MirValue::Use(live),
        then_block: live_block,
        else_block: done,
    };
    let drop_fn = f.new_temp(Ty::Int);
    f.blocks[live_block].instructions.push(MirInst::GetField {
        dest: drop_fn,
        base: pair,
        field_index: CLOSURE_PAIR_DROP,
    });
    f.blocks[live_block]
        .instructions
        .push(MirInst::CallIndirect {
            dest: None,
            callee: drop_fn,
            args: vec![MirValue::Use(pair)],
            // The literal's `<closure>_drop` releases this owner of the pair.
            modes: vec![ParamMode::Move],
            pair: None,
        });
    f.blocks[live_block].terminator = Terminator::Goto(done);
    f.blocks[done].terminator = Terminator::Return(None);
    f
}

#[cfg(test)]
mod pair_layout_tests {
    use super::*;

    /// The value of `#define NAME <int>` in `header`.
    fn define(header: &str, name: &str) -> usize {
        header
            .lines()
            .find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some("#define") && words.next() == Some(name))
                    .then(|| words.next())
                    .flatten()
            })
            .unwrap_or_else(|| panic!("runtime.h defines no `{name}`"))
            .parse()
            .unwrap_or_else(|e| panic!("`{name}` is not an integer: {e}"))
    }

    /// The C runtime reads closure pairs through `RuxenClosurePair` and the
    /// `RUXEN_CLOSURE_*` slots (runtime.h, which `_Static_assert`s the struct
    /// against them); the compiler writes them through `CLOSURE_PAIR_*`.
    /// Drift between the two is a silent miscompile.
    #[test]
    fn closure_pair_slots_match_the_c_runtime() {
        let header = include_str!("../../../../../../library/std/core/runtime/runtime.h");
        assert_eq!(define(header, "RUXEN_CLOSURE_FN_SLOT"), CLOSURE_PAIR_FN);
        assert_eq!(
            define(header, "RUXEN_CLOSURE_CAPTURES_SLOT"),
            CLOSURE_PAIR_CAPTURES
        );
        assert_eq!(define(header, "RUXEN_CLOSURE_DROP_SLOT"), CLOSURE_PAIR_DROP);
        assert_eq!(
            define(header, "RUXEN_CLOSURE_SHARES_SLOT"),
            CLOSURE_PAIR_SHARES
        );
        assert_eq!(define(header, "RUXEN_CLOSURE_PAIR_SIZE"), CLOSURE_PAIR_SIZE);
    }
}
