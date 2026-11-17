//! What a call result points into and carries, read off a finished body:
//! [`record_default_views`] records each value's [`View`], and
//! [`borrows_of`] is every local a drop must wait for.

use super::*;

/// `local` and every local holding a borrow of it or of storage it owns:
/// all of them must be dead before `local` may be dropped. A borrow is a
/// `Ref`/`RefMut` of it, a copy of a borrow word, a field read
/// (`GetField`: `U.new(..).name` points into the object) or a call result
/// that may point into it or carry what it holds, per the [`CallFact`]
/// recorded for the result ([`record_default_views`]), or an argument a
/// call stores it in (`param_modes::call_stores`, the loan check's rule);
/// a scalar or an owned result borrows nothing.
pub(super) fn borrows_of(
    func: &MirFunction,
    local: LocalId,
    symbols: &SymbolTable,
    summaries: &CallSummaries,
) -> HashSet<LocalId> {
    let mut set = HashSet::from([local]);
    let view = |dest: LocalId| !matches!(func.view(dest), Some(View::Scalar | View::Own));
    let into = |dest: LocalId, slot: usize| match func.view(dest) {
        Some(View::Args(fact) | View::Holds(fact)) => {
            fact.borrows.contains(&slot) || fact.carries.contains(&slot)
        }
        Some(View::Scalar | View::Own) => false,
        _ => true,
    };
    loop {
        let before = set.len();
        for inst in func.blocks.iter().flat_map(|block| &block.instructions) {
            match inst {
                MirInst::Ref { dest, src }
                | MirInst::RefMut { dest, src }
                | MirInst::FieldAddr {
                    dest, base: src, ..
                } if set.contains(src) => {
                    set.insert(*dest);
                }
                MirInst::GetField { dest, base, .. } if set.contains(base) && view(*dest) => {
                    set.insert(*dest);
                }
                MirInst::Call { dest, args, .. } | MirInst::CallIndirect { dest, args, .. } => {
                    let held: Vec<usize> = (0..args.len())
                        .filter(|&slot| matches!(args[slot], MirValue::Use(a) if set.contains(&a)))
                        .collect();
                    if held.is_empty() {
                        continue;
                    }
                    if let Some(dest) = dest.filter(|&d| held.iter().any(|&s| into(d, s))) {
                        set.insert(dest);
                    }
                    let (modes, keeps) = match inst {
                        MirInst::CallIndirect { modes, .. } => (
                            crate::mir::param_modes::resolve_modes(modes, func, args, symbols),
                            None,
                        ),
                        MirInst::Call { callee, .. } => (
                            summaries.modes.of_call(func, callee, args, symbols),
                            summaries.modes.keeps(callee),
                        ),
                        _ => continue,
                    };
                    let scalar = |l: LocalId| is_scalar(&func.locals[l as usize].ty);
                    for (from, into) in
                        crate::mir::param_modes::call_stores(func, args, &modes, keeps, scalar)
                    {
                        if held.contains(&from) {
                            set.extend(args[into].local());
                        }
                    }
                }
                MirInst::Copy { dest, src } | MirInst::Move { dest, src }
                    if *src != local && set.contains(src) =>
                {
                    set.insert(*dest);
                }
                MirInst::Assign {
                    dest,
                    value: MirValue::Use(src),
                } if *src != local && set.contains(src) => {
                    set.insert(*dest);
                }
                _ => {}
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

/// Record in `func.views` the [`View`]s read off its finished body, for
/// each parameter and each value lowering recorded no view of where it made
/// it: a scalar is `View::Scalar`, a fresh runtime result holding
/// references is `View::Holds` of what it borrows ([`owns_storage`]),
/// and any other call result points into the non-scalar arguments
/// [`call_result_may_borrow_arg`] names (a closure call result into all of
/// them). Runs once
/// every callee's body exists, before the loan check and drop elaboration
/// read the views.
pub(in crate::mir::lower) fn record_default_views(
    func: &mut MirFunction,
    summaries: &CallSummaries,
    facts: &crate::mir::loan_check::summary::BodyFacts,
) {
    let scalar = |local: LocalId| is_scalar(&func.locals[local as usize].ty);
    let mut found: Vec<(LocalId, View)> = func
        .params
        .iter()
        .filter(|&&param| scalar(param) && !func.views.contains_key(&param))
        .map(|&param| (param, View::Scalar))
        .collect();
    for inst in func.blocks.iter().flat_map(|block| &block.instructions) {
        let Some(dest) = inst.dest() else {
            continue;
        };
        if func.views.contains_key(&dest) {
            continue;
        }
        let pointed_into = |args: &[MirValue], may_borrow: &dyn Fn(usize) -> bool| {
            (0..args.len())
                .filter(|&slot| {
                    args[slot].local().is_some_and(|arg| !scalar(arg)) && may_borrow(slot)
                })
                .collect::<Vec<usize>>()
        };
        // What the result carries: every non-scalar argument of a call no
        // declaration describes, when the result may hold a reference.
        let carries_all = |args: &[MirValue]| {
            if func.locals[dest as usize]
                .ty
                .may_hold_reference(&mut |_| true)
            {
                pointed_into(args, &|_| true)
            } else {
                Vec::new()
            }
        };
        let view = match inst {
            _ if scalar(dest) => View::Scalar,
            MirInst::Call { callee, args, .. } => {
                let fresh = owns_storage(func, dest, callee, summaries);
                let Some((declared_facts, declared)) =
                    summaries.result_sources.get(callee.as_str())
                else {
                    // A runtime helper with no declaration (the compiler
                    // calls it itself): its result may point into every
                    // argument it may borrow, and carry every one.
                    let fact = CallFact {
                        borrows: pointed_into(args, &|slot| {
                            fresh || call_result_may_borrow_arg(func, dest, callee, slot, summaries)
                        }),
                        carries: carries_all(args),
                    };
                    found.push((
                        dest,
                        if fresh {
                            View::Holds(fact)
                        } else {
                            View::Args(fact)
                        },
                    ));
                    continue;
                };
                let source = &declared_facts.source;
                let summary = facts.get(callee.as_str());
                // (`Ambiguous` is E0748 in resolve and never lowered.)
                let elided = |slot: usize| match source {
                    ResultSource::Slots(from) => from.contains(&slot),
                    ResultSource::Nothing => false,
                    ResultSource::Unknown | ResultSource::Ambiguous(_) => true,
                };
                let borrows = match *declared {
                    // A C function whose declared result holds a reference
                    // borrows what its declaration says; one returning a
                    // value borrows nothing only when the value is provably
                    // new (`-> owned`, a Fresh row), else every argument
                    // (`reverse` returns its receiver, `take` copies its
                    // element pointers).
                    Declared::Ffi => pointed_into(args, &|slot| match source {
                        ResultSource::Nothing => {
                            call_result_may_borrow_arg(func, dest, callee, slot, summaries)
                        }
                        _ => elided(slot) || summaries.may_return_param(callee, slot),
                    }),
                    // A body borrows what its signature says, and whatever
                    // else the body may return (the compiler does not yet
                    // check a body against its signature, and
                    // `Comparable#clamp` returns any of its three borrows).
                    // ... and what its summary finds (`loan_check::summary`).
                    Declared::Body => pointed_into(args, &|slot| {
                        (matches!(source, ResultSource::Slots(_)) && elided(slot))
                            || call_result_may_borrow_arg(func, dest, callee, slot, summaries)
                            || summary.is_some_and(|fact| fact.borrows.contains(&slot))
                    }),
                };
                // An unknown source carries every argument; otherwise what
                // the declaration says, and a body's summary.
                let carries = match source {
                    ResultSource::Unknown => carries_all(args),
                    _ => pointed_into(args, &|slot| {
                        declared_facts.carries.contains(&slot)
                            || summary.is_some_and(|fact| fact.carries.contains(&slot))
                    }),
                };
                let fact = CallFact { borrows, carries };
                if fresh {
                    View::Holds(fact)
                } else {
                    View::Args(fact)
                }
            }
            // A closure call whose declared result could not be shown to
            // own itself (`closure_call_facts`) may point into and carry
            // every argument.
            MirInst::CallIndirect { args, .. } => View::Args(CallFact {
                borrows: pointed_into(args, &|_| true),
                carries: carries_all(args),
            }),
            _ => continue,
        };
        found.push((dest, view));
    }
    for (dest, view) in found {
        func.record_view(dest, view);
    }
}

/// Whether the result `dest` of a call to `callee` is a fresh value that
/// owns its storage and holds references (`h.keys`, an `Array[&K]` whose
/// spine the caller frees): [`View::Holds`] of what it borrows, so the
/// view keeps denoting its own storage.
fn owns_storage(
    func: &MirFunction,
    dest: LocalId,
    callee: &str,
    summaries: &CallSummaries,
) -> bool {
    use crate::mir::lower::runtime_abi::{result_ownership, ResultOwnership};
    let ty = &func.locals[dest as usize].ty;
    !ty.is_ref()
        && ty.holds_reference()
        && (summaries.fresh.contains(callee)
            || (!summaries.block_params.contains_key(callee)
                && result_ownership(callee, &summaries.declared_results) == ResultOwnership::Fresh))
}

/// Whether the result `dest` of a call to `callee` may point into the
/// argument at `slot` (see [`record_default_views`]).
fn call_result_may_borrow_arg(
    func: &MirFunction,
    dest: LocalId,
    callee: &str,
    slot: usize,
    summaries: &CallSummaries,
) -> bool {
    use crate::mir::lower::runtime_abi::{result_ownership, ResultOwnership};
    if func.locals[dest as usize].ty.is_ref() || summaries.may_return_param(callee, slot) {
        return true;
    }
    let user_callee = summaries.block_params.contains_key(callee);
    !user_callee
        && !summaries.fresh.contains(callee)
        && result_ownership(callee, &summaries.declared_results) != ResultOwnership::Fresh
        && !FREE_FAMILY_CALLEES.contains(&callee)
}
