use super::*;
use std::collections::BTreeSet;

/// Post-lowering fixup that elides use-after-free deallocs caused by the
/// `lower_assign` "pre-reassign drop" path. That path emits
/// `ruxen_dealloc(R)` before `Assign { dest: R, value: Use(X) }`; when the
/// call that produced `X` hands back the very same heap object it was given
/// for `R`, the dealloc frees the object the assign then re-reads.
///
/// Two shapes hit this:
///
/// ```text
///   # receiver-builder: returns `self` (parameter index 0)
///   def var with_status(c: Int) -> Reply
///     self.status = c
///     self
///   end
///   var r = Reply.new
///   r = r.with_status(201)     # dealloc(r) before the re-assign of r
///
///   # by-value rebind (Q55): takes a parameter BY VALUE and returns it,
///   # possibly via an intermediate `var q = p; ...; q`
///   def wrap(p: Payload) -> Payload
///     var q = p
///     q
///   end
///   var x = wrap(Payload.new(1))
///   x = wrap(x)                # dealloc(x) before the re-assign of x
/// ```
///
/// MIR pattern (consecutive instructions in one block):
///
/// ```text
///   Call { dest: Some(X), callee: F, args: [... Use(R) ...] }
///   Call { dest: None,    callee: "ruxen_dealloc", args: [Use(R)] }
///   Assign { dest: R, value: Use(X) }
/// ```
///
/// where `F` deterministically returns the parameter at the SAME index `k`
/// that `R` was passed at (`args[k] == Use(R)`). We detect this by building,
/// per function, a `name -> returned-param-index` map: for each block ending
/// in `Return(Some(Use(local)))` we resolve `local` back to a parameter index
/// by tracing intra-function `Move` / `Copy` / `Assign { value: Use(_) }`
/// aliasing, seeded with `params[i] -> i`. A function qualifies with index `k`
/// only when it has at least one returning block AND every returning block
/// resolves to that same `k`; a returning block that resolves to a different
/// index, or to a non-parameter local, disqualifies it. The receiver-builder
/// case is `k == 0` returned literally; the by-value-rebind case is any `k`
/// reached through the alias trace. Action: remove the middle `ruxen_dealloc`.
///
/// Indirect calls (closure / fn-pointer dispatch) cannot be traced, so they
/// keep the documented bounded-leak tradeoff: elide when `R` aliases ANY
/// user-visible argument (worst case a one-time leak, never a use-after-free).
///
/// We can't fix this in `lower_assign` itself because the callee's
/// returned-parameter property isn't known until that callee's body has been
/// lowered, and lowering order is per-function. Running this as a post-pass
/// over the whole `MirProgram` sidesteps the ordering.
pub fn elide_returns_self_realloc(mir: &mut crate::mir::nodes::MirProgram) {
    use crate::mir::nodes::{MirInst, MirValue, Terminator};

    // Step 1: collect, for each function, the SINGLE parameter index it
    // deterministically returns (if any). A function maps to index `k` when
    // every block that returns `Return(Some(Use(local)))` has `local` proven to
    // alias parameter `k` ON EVERY execution path that reaches that return.
    // Receiver-builders that return `self` literally are the `k == 0` special
    // case; a by-value passthrough (`var q = p; q`) is any `k` reached through
    // parameter aliasing.
    //
    // This is a forward dataflow over the CFG, NOT a linear last-writer walk
    // over the block vector. A per-local map `local -> k` (presence means
    // "definitely aliases parameter k on every path to here"; absence means
    // "not a known single parameter") is transferred through each block
    // (`transfer_param_prov`) and MET at control-flow merges by intersection
    // with value agreement (a local survives only when every predecessor agrees
    // on the same `k`). A merge where one path yields a fresh object and
    // another the parameter (`if c; Foo.new; else; a; end`) therefore drops the
    // returned local out of the map, so the function is correctly NOT treated
    // as returns-parameter (Q55b: the old linear walk misclassified it and
    // elided a needed dealloc, leaking on the fresh branch). Erring toward
    // "not a parameter" only ever LEAVES a dealloc in place (at worst a leak),
    // never removes a needed one, so it cannot create a use-after-free.
    let mut returns_param: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for func in &mir.functions {
        let n = func.blocks.len();
        if n == 0 {
            continue;
        }

        // Predecessor lists from each block's terminator successors.
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (bi, block) in func.blocks.iter().enumerate() {
            for succ in block_successors(&block.terminator) {
                if succ < n {
                    preds[succ].push(bi);
                }
            }
        }

        // Entry seed: each parameter aliases itself.
        let entry_in: std::collections::HashMap<crate::mir::nodes::LocalId, usize> = func
            .params
            .iter()
            .enumerate()
            .map(|(i, &p)| (p, i))
            .collect();

        // Forward dataflow to a fixpoint. `outs[b]` is the provenance map at the
        // END of block `b` (its state at the terminator / return point). The
        // maps only ever shrink as merges intersect them, so the fixpoint
        // converges; the iteration cap is a defensive bound.
        let mut outs: Vec<std::collections::HashMap<crate::mir::nodes::LocalId, usize>> =
            vec![std::collections::HashMap::new(); n];
        let cap = n.saturating_mul(2).saturating_add(4);
        let mut changed = true;
        let mut guard = 0usize;
        while changed && guard < cap {
            changed = false;
            guard += 1;
            for bi in 0..n {
                // IN[bi] = meet (intersection with agreement) over predecessor
                // OUTs, seeded with the parameter map for the entry block.
                let mut acc: Option<std::collections::HashMap<crate::mir::nodes::LocalId, usize>> =
                    if bi == 0 {
                        Some(entry_in.clone())
                    } else {
                        None
                    };
                for &p in &preds[bi] {
                    let po = &outs[p];
                    match acc.as_mut() {
                        None => acc = Some(po.clone()),
                        Some(a) => a.retain(|k, v| po.get(k) == Some(v)),
                    }
                }
                let new_in = acc.unwrap_or_default();
                let new_out = transfer_param_prov(&new_in, &func.blocks[bi]);
                if new_out != outs[bi] {
                    outs[bi] = new_out;
                    changed = true;
                }
            }
        }

        // A function qualifies with index `k` only when every returning block's
        // returned local is proven to alias parameter `k` at that return point,
        // and all agree on the same `k`.
        let mut saw_return = false;
        let mut agreed_index: Option<usize> = None;
        let mut disqualified = false;
        for (bi, block) in func.blocks.iter().enumerate() {
            if let Terminator::Return(Some(MirValue::Use(local))) = &block.terminator {
                saw_return = true;
                match outs[bi].get(local).copied() {
                    Some(k) => match agreed_index {
                        None => agreed_index = Some(k),
                        Some(prev) if prev != k => disqualified = true,
                        Some(_) => {}
                    },
                    None => disqualified = true,
                }
            }
        }

        if saw_return && !disqualified {
            if let Some(k) = agreed_index {
                returns_param.insert(func.name.clone(), k);
            }
        }
    }

    if returns_param.is_empty() {
        return;
    }

    // Step 2: walk every function's blocks, locate the bad triple,
    // and delete the middle (ruxen_dealloc) instruction.
    for func in &mut mir.functions {
        for block in &mut func.blocks {
            // Track, per result local, the call that produced it: the callee
            // name plus its POSITIONAL argument list. For a direct call we
            // consult `returns_param[callee]` to learn which argument position
            // the callee hands back, and elide only when `R` sits at exactly
            // that position (`args[k] == Use(R)`). For an indirect call we
            // can't know the returned slot, so we tag it `<indirect>` and fall
            // back to "R is any user-visible arg", the documented bounded-leak
            // (never a UAF) tradeoff.
            const INDIRECT_SENTINEL: &str = "<indirect>";
            // Map dest_local -> (callee_name, positional args).
            let mut last_call: std::collections::HashMap<
                crate::mir::nodes::LocalId,
                (String, Vec<MirValue>),
            > = std::collections::HashMap::new();

            let mut to_remove: Vec<usize> = Vec::new();
            for (i, inst) in block.instructions.iter().enumerate() {
                match inst {
                    MirInst::Call { dest, callee, args } => {
                        if let Some(d) = dest {
                            last_call.insert(*d, (callee.clone(), args.clone()));
                        }
                        // Detect: ruxen_dealloc(R) followed by Assign R = X
                        // where X came from a callee that hands R's object back
                        // at the argument position it returns.
                        if callee == "ruxen_dealloc" && args.len() == 1 {
                            if let MirValue::Use(r) = &args[0] {
                                if let Some(MirInst::Assign {
                                    dest: a_dest,
                                    value: MirValue::Use(x),
                                }) = block.instructions.get(i + 1)
                                {
                                    if a_dest == r {
                                        if let Some((callee_name, call_args)) = last_call.get(x) {
                                            let elide = if callee_name == INDIRECT_SENTINEL {
                                                // Indirect: args[0] is the
                                                // captures_ptr; `R` aliasing ANY
                                                // user arg means the closure may
                                                // hand it back.
                                                call_args.iter().skip(1).any(
                                                    |a| matches!(a, MirValue::Use(l) if l == r),
                                                )
                                            } else if let Some(&k) = returns_param.get(callee_name)
                                            {
                                                // Direct: elide only when `R` is
                                                // the arg at the position the
                                                // callee actually returns.
                                                matches!(
                                                    call_args.get(k),
                                                    Some(MirValue::Use(l)) if l == r
                                                )
                                            } else {
                                                false
                                            };
                                            if elide {
                                                to_remove.push(i);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    MirInst::CallIndirect {
                        dest: Some(d),
                        args,
                        ..
                    } => {
                        // Indirect call through a closure / fn pointer. ABI:
                        // args[0] is the captures_ptr the closure lowering
                        // prepends; args[1..] are the user's visible arguments.
                        // We can't know which (if any) the closure returns, so
                        // we store the full arg list and tag it
                        // INDIRECT_SENTINEL; the elide site skips arg 0 and
                        // treats any remaining user arg as a potential alias.
                        // The worst case is a bounded one-time leak, not a
                        // use-after-free.
                        last_call.insert(*d, (INDIRECT_SENTINEL.to_string(), args.clone()));
                    }
                    MirInst::Assign {
                        dest,
                        value: MirValue::Use(src),
                    } => {
                        if let Some(entry) = last_call.get(src).cloned() {
                            last_call.insert(*dest, entry);
                        }
                    }
                    _ => {}
                }
            }
            // Remove in reverse so earlier indices stay valid.
            for &i in to_remove.iter().rev() {
                block.instructions.remove(i);
            }
        }
    }
}

/// The successor block ids reachable from a terminator. Used to build the
/// predecessor map for `elide_returns_self_realloc`'s CFG dataflow.
fn block_successors(t: &crate::mir::nodes::Terminator) -> Vec<crate::mir::nodes::BlockId> {
    use crate::mir::nodes::Terminator;
    match t {
        Terminator::Goto(b) => vec![*b],
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch {
            targets, otherwise, ..
        } => {
            let mut v: Vec<crate::mir::nodes::BlockId> = targets.iter().map(|(_, b)| *b).collect();
            v.push(*otherwise);
            v
        }
        Terminator::Return(_) | Terminator::Unreachable => Vec::new(),
    }
}

/// Transfer a parameter-provenance map through one block's instructions,
/// returning the map at the block's end. A key `local -> k` means "definitely
/// aliases parameter `k` on every path to this point"; an absent local is not a
/// known single parameter. A `Move` / `Copy` / `Assign(Use)` carries the
/// source's provenance to the destination (or clears it when the source has
/// none); every other definition writes a value that does not alias a
/// parameter, so it clears its destination.
fn transfer_param_prov(
    in_state: &std::collections::HashMap<crate::mir::nodes::LocalId, usize>,
    block: &crate::mir::nodes::BasicBlock,
) -> std::collections::HashMap<crate::mir::nodes::LocalId, usize> {
    use crate::mir::nodes::{MirInst, MirValue};
    let mut s = in_state.clone();
    for inst in &block.instructions {
        match inst {
            MirInst::Move { dest, src } | MirInst::Copy { dest, src } => {
                match s.get(src).copied() {
                    Some(k) => {
                        s.insert(*dest, k);
                    }
                    None => {
                        s.remove(dest);
                    }
                }
            }
            MirInst::Assign {
                dest,
                value: MirValue::Use(src),
            } => match s.get(src).copied() {
                Some(k) => {
                    s.insert(*dest, k);
                }
                None => {
                    s.remove(dest);
                }
            },
            other => {
                if let Some(dest) = defined_local(other) {
                    s.remove(&dest);
                }
            }
        }
    }
    s
}

/// The local a MIR instruction writes, if any. Used by
/// `elide_returns_self_realloc`'s parameter-provenance trace to REVOKE a
/// local's param alias when it is redefined by anything other than a
/// param-propagating `Move` / `Copy` / `Assign(Use)`.
fn defined_local(inst: &crate::mir::nodes::MirInst) -> Option<crate::mir::nodes::LocalId> {
    use crate::mir::nodes::MirInst;
    match inst {
        MirInst::Assign { dest, .. }
        | MirInst::BinOp { dest, .. }
        | MirInst::Negate { dest, .. }
        | MirInst::Not { dest, .. }
        | MirInst::Compare { dest, .. }
        | MirInst::Alloc { dest, .. }
        | MirInst::StackAlloc { dest, .. }
        | MirInst::GetField { dest, .. }
        | MirInst::GetTag { dest, .. }
        | MirInst::GetPayload { dest, .. }
        | MirInst::Ref { dest, .. }
        | MirInst::RefMut { dest, .. }
        | MirInst::Copy { dest, .. }
        | MirInst::Move { dest, .. }
        | MirInst::StringLiteral { dest, .. }
        | MirInst::FuncAddr { dest, .. }
        | MirInst::FloatToBits { dest, .. }
        | MirInst::BitsToFloat { dest, .. }
        | MirInst::DataAddr { dest, .. } => Some(*dest),
        MirInst::Call { dest, .. } | MirInst::CallIndirect { dest, .. } => *dest,
        MirInst::SetField { .. } | MirInst::SetTag { .. } | MirInst::Drop { .. } | MirInst::Nop => {
            None
        }
    }
}

/// True only when a by-value capture is never moved out of its captures slot.
/// Reads through the captured value are borrows; returns, aggregate stores,
/// explicit moves, and consuming/unknown calls make ownership unprovable and
/// suppress that field's closure-drop glue.
pub(super) fn capture_slot_still_owned(
    func: &MirFunction,
    captures: LocalId,
    slot: usize,
    symbols: &SymbolTable,
) -> bool {
    use crate::mir::lower::runtime_abi::{callee_ownership, user_callee_param_is_ref};

    let mut aliases: HashSet<LocalId> = HashSet::new();
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
                    } if *base == captures && *field_index == slot => {
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
                    return false;
                }
                MirInst::Call { callee, args, .. } => {
                    let abi = callee_ownership(callee);
                    let param_is_ref = if abi.args_are_borrowed {
                        Vec::new()
                    } else {
                        user_callee_param_is_ref(symbols, callee, args.len())
                    };
                    for (idx, arg) in args.iter().enumerate() {
                        let MirValue::Use(local) = arg else {
                            continue;
                        };
                        if !aliases.contains(local) {
                            continue;
                        }
                        let borrowed = (abi.borrows_first_arg && idx == 0)
                            || (!abi.arg_transfer.contains(idx)
                                && (abi.args_are_borrowed
                                    || param_is_ref.get(idx).copied().unwrap_or(false)));
                        if !borrowed {
                            return false;
                        }
                    }
                }
                MirInst::CallIndirect { args, .. } => {
                    if args
                        .iter()
                        .any(|arg| matches!(arg, MirValue::Use(local) if aliases.contains(local)))
                    {
                        return false;
                    }
                }
                MirInst::SetField { value, .. } => {
                    if matches!(value, MirValue::Use(local) if aliases.contains(local)) {
                        return false;
                    }
                }
                MirInst::Drop { local } if aliases.contains(local) => return false,
                _ => {}
            }
        }
        if matches!(
            &block.terminator,
            Terminator::Return(Some(MirValue::Use(local))) if aliases.contains(local)
        ) {
            return false;
        }
    }
    true
}

#[derive(Clone)]
struct ClosureDropGlue {
    drop_fn: String,
    owned_capture_slots: Vec<(usize, LocalId)>,
}

fn closure_drop_callees(
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

fn value_uses_local(value: &MirValue, local: LocalId) -> bool {
    matches!(value, MirValue::Use(candidate) if *candidate == local)
}

fn instruction_initializes_local(inst: &MirInst, local: LocalId) -> bool {
    match inst {
        MirInst::Assign { dest, value } => {
            *dest == local && !matches!(value, MirValue::Literal(Literal::Int(0)))
        }
        MirInst::BinOp { dest, .. }
        | MirInst::Negate { dest, .. }
        | MirInst::Not { dest, .. }
        | MirInst::Compare { dest, .. }
        | MirInst::Alloc { dest, .. }
        | MirInst::StackAlloc { dest, .. }
        | MirInst::GetField { dest, .. }
        | MirInst::GetTag { dest, .. }
        | MirInst::GetPayload { dest, .. }
        | MirInst::Ref { dest, .. }
        | MirInst::RefMut { dest, .. }
        | MirInst::Copy { dest, .. }
        | MirInst::Move { dest, .. }
        | MirInst::StringLiteral { dest, .. }
        | MirInst::FuncAddr { dest, .. }
        | MirInst::FloatToBits { dest, .. }
        | MirInst::BitsToFloat { dest, .. }
        | MirInst::DataAddr { dest, .. } => *dest == local,
        MirInst::Call {
            dest: Some(dest), ..
        }
        | MirInst::CallIndirect {
            dest: Some(dest), ..
        } => *dest == local,
        MirInst::Call { dest: None, .. }
        | MirInst::CallIndirect { dest: None, .. }
        | MirInst::SetField { .. }
        | MirInst::SetTag { .. }
        | MirInst::Drop { .. }
        | MirInst::Nop => false,
    }
}

fn instruction_clears_local(inst: &MirInst, local: LocalId) -> bool {
    match inst {
        MirInst::Assign { dest, value } => {
            (*dest == local && matches!(value, MirValue::Literal(Literal::Int(0))))
                || (*dest != local && value_uses_local(value, local))
        }
        MirInst::Copy { dest, src } | MirInst::Move { dest, src } => {
            *dest != local && *src == local
        }
        MirInst::SetField { value, .. } => value_uses_local(value, local),
        MirInst::Drop { local: dropped } => *dropped == local,
        _ => false,
    }
}

fn initialized_after_block(block: &BasicBlock, local: LocalId, mut initialized: bool) -> bool {
    for inst in &block.instructions {
        if instruction_clears_local(inst, local) {
            initialized = false;
        }
        if instruction_initializes_local(inst, local) {
            initialized = true;
        }
    }
    initialized
}

fn instruction_mentions_local(inst: &MirInst, local: LocalId) -> bool {
    match inst {
        MirInst::Assign { dest, value } => *dest == local || value_uses_local(value, local),
        MirInst::BinOp { dest, lhs, rhs, .. } | MirInst::Compare { dest, lhs, rhs, .. } => {
            *dest == local || value_uses_local(lhs, local) || value_uses_local(rhs, local)
        }
        MirInst::Negate { dest, operand } | MirInst::Not { dest, operand } => {
            *dest == local || value_uses_local(operand, local)
        }
        MirInst::Call { dest, args, .. } => {
            *dest == Some(local) || args.iter().any(|arg| value_uses_local(arg, local))
        }
        MirInst::Alloc { dest, .. }
        | MirInst::StackAlloc { dest, .. }
        | MirInst::StringLiteral { dest, .. }
        | MirInst::FuncAddr { dest, .. }
        | MirInst::DataAddr { dest, .. } => *dest == local,
        MirInst::GetField { dest, base, .. } => *dest == local || *base == local,
        MirInst::SetField { base, value, .. } => *base == local || value_uses_local(value, local),
        MirInst::SetTag { dest, .. } => *dest == local,
        MirInst::GetTag { dest, src }
        | MirInst::Ref { dest, src }
        | MirInst::RefMut { dest, src }
        | MirInst::Copy { dest, src }
        | MirInst::Move { dest, src }
        | MirInst::FloatToBits { dest, src }
        | MirInst::BitsToFloat { dest, src } => *dest == local || *src == local,
        MirInst::GetPayload { dest, src, .. } => *dest == local || *src == local,
        MirInst::Drop { local: candidate } => *candidate == local,
        MirInst::CallIndirect {
            dest, callee, args, ..
        } => {
            *dest == Some(local)
                || *callee == local
                || args.iter().any(|arg| value_uses_local(arg, local))
        }
        MirInst::Nop => false,
    }
}

fn terminator_mentions_local(terminator: &Terminator, local: LocalId) -> bool {
    match terminator {
        Terminator::Return(value) => value
            .as_ref()
            .is_some_and(|value| value_uses_local(value, local)),
        Terminator::Branch { cond, .. } => value_uses_local(cond, local),
        Terminator::Switch { value, .. } => value_uses_local(value, local),
        Terminator::Goto(_) | Terminator::Unreachable => false,
    }
}

fn terminator_successors(terminator: &Terminator) -> Vec<BlockId> {
    match terminator {
        Terminator::Goto(target) => vec![*target],
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch {
            targets, otherwise, ..
        } => {
            let mut successors: Vec<BlockId> = targets.iter().map(|(_, target)| *target).collect();
            successors.push(*otherwise);
            successors
        }
        Terminator::Return(_) | Terminator::Unreachable => Vec::new(),
    }
}

fn reachable_blocks(func: &MirFunction) -> Vec<bool> {
    let mut reachable = vec![false; func.blocks.len()];
    let mut pending = vec![func.entry_block];
    while let Some(block_id) = pending.pop() {
        if block_id >= func.blocks.len() || reachable[block_id] {
            continue;
        }
        reachable[block_id] = true;
        pending.extend(terminator_successors(&func.blocks[block_id].terminator));
    }
    reachable
}

fn local_initialized_at_all_returns(
    func: &MirFunction,
    local: LocalId,
    reachable: &[bool],
) -> bool {
    let mut predecessors: Vec<Vec<BlockId>> = vec![Vec::new(); func.blocks.len()];
    for (block_id, block) in func.blocks.iter().enumerate() {
        if !reachable[block_id] {
            continue;
        }
        for successor in terminator_successors(&block.terminator) {
            if successor < func.blocks.len() && reachable[successor] {
                predecessors[successor].push(block_id);
            }
        }
    }

    let mut initialized_in = vec![true; func.blocks.len()];
    let mut initialized_out = vec![true; func.blocks.len()];
    initialized_in[func.entry_block] = false;
    initialized_out[func.entry_block] =
        initialized_after_block(&func.blocks[func.entry_block], local, false);

    loop {
        let mut changed = false;
        for block_id in 0..func.blocks.len() {
            if !reachable[block_id] {
                continue;
            }
            // The entry block and a block with no reachable predecessor
            // start with `local` uninitialized.
            let new_in = if block_id == func.entry_block || predecessors[block_id].is_empty() {
                false
            } else {
                predecessors[block_id]
                    .iter()
                    .all(|predecessor| initialized_out[*predecessor])
            };
            let new_out = initialized_after_block(&func.blocks[block_id], local, new_in);
            changed |= initialized_in[block_id] != new_in || initialized_out[block_id] != new_out;
            initialized_in[block_id] = new_in;
            initialized_out[block_id] = new_out;
        }
        if !changed {
            break;
        }
    }

    func.blocks.iter().enumerate().all(|(block_id, block)| {
        !reachable[block_id]
            || !matches!(block.terminator, Terminator::Return(_))
            || initialized_out[block_id]
    })
}

fn block_reaches(func: &MirFunction, start: BlockId, target: BlockId) -> bool {
    let mut visited = vec![false; func.blocks.len()];
    let mut pending = vec![start];
    while let Some(block_id) = pending.pop() {
        if block_id == target {
            return true;
        }
        if block_id >= func.blocks.len() || visited[block_id] {
            continue;
        }
        visited[block_id] = true;
        pending.extend(terminator_successors(&func.blocks[block_id].terminator));
    }
    false
}

fn branch_local_drop_blocks(
    func: &MirFunction,
    locals: &HashSet<LocalId>,
    reachable: &[bool],
) -> HashMap<BlockId, Vec<LocalId>> {
    let mut placements: HashMap<BlockId, Vec<LocalId>> = HashMap::new();
    for &local in locals {
        if local_initialized_at_all_returns(func, local, reachable) {
            continue;
        }
        let mention_blocks: Vec<BlockId> = func
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                block
                    .instructions
                    .iter()
                    .any(|inst| instruction_mentions_local(inst, local))
                    || terminator_mentions_local(&block.terminator, local)
            })
            .map(|(block_id, _)| block_id)
            .collect();
        let [block_id] = mention_blocks.as_slice() else {
            continue;
        };
        let block = &func.blocks[*block_id];
        let Terminator::Goto(target) = block.terminator else {
            continue;
        };
        if !reachable[*block_id]
            || block_reaches(func, target, *block_id)
            || !initialized_after_block(block, local, false)
        {
            continue;
        }
        placements.entry(*block_id).or_default().push(local);
    }
    for locals in placements.values_mut() {
        locals.sort_unstable_by(|left, right| right.cmp(left));
    }
    placements
}

/// True when `name` (a `Ty::Class` name such as `Shop.Cart`, or its MIR
/// symbol spelling `Shop_Cart`) resolves to a class in `drop_classes`, the
/// DefIds `collect_user_drop_classes` registered as having a `def drop`.
pub(crate) fn is_drop_class(
    symbols: &SymbolTable,
    drop_classes: &HashSet<DefId>,
    name: &str,
) -> bool {
    symbols
        .type_def(name)
        .is_some_and(|def| drop_classes.contains(&def.id))
}

pub(super) fn insert_drops(
    func: &mut MirFunction,
    return_locals: &HashSet<LocalId>,
    symbols: &SymbolTable,
    user_drop_classes: &HashSet<DefId>,
    resolve_ffi_alias_callee: &dyn Fn(String) -> String,
) {
    // Avoid recursing into a class's own `drop` method: if `Holder_drop`
    // takes `self: Holder`, drop-elaboration on `self` would emit a call
    // back to `Holder_drop`, recursing forever at runtime.
    let in_user_drop_method = func
        .name
        .strip_suffix("_drop")
        .is_some_and(|prefix| is_drop_class(symbols, user_drop_classes, prefix));

    // Build a set of parameter locals to skip.
    let param_set: HashSet<LocalId> = func.params.iter().copied().collect();

    // Determine which locals' value provenance is a fresh allocation that
    // this frame owns exclusively. We need this set BEFORE building
    // `drop_locals` because String / Vec / HashMap intermediates that are
    // technically compiler temporaries (`LocalKind::Temp`) can still own heap
    // (e.g. the implicit `ruxen_string_from` wrap inserted around a string
    // literal, or the owned `String` a `.clone` produces). Without
    // dropping such intermediates, every interpolated literal leaks the
    // owned-string copy of itself.
    let dealloc_safe = compute_dealloc_safe_locals(func, symbols);

    // Locals handed to FFI calls that take ownership must not be
    // dropped at scope exit even when they are constructor temps rather
    // than user `let` bindings. Rondo's accept loop exercises this:
    // `Task.spawn_raw(RondoConnectionTask.new(...))` lowers to a temp
    // allocation passed to `ruxen_executor_spawn`; the scheduler owns
    // that Future after the call.
    let mut moved_to_ffi: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for inst in &block.instructions {
            if let MirInst::Call { callee, args, .. } = inst {
                // Single source of truth: runtime_abi::MOVE_BY_FFI. Previously
                // this was a hand-rolled `matches!` that had drifted to include
                // a dead dotted `"Task.spawn_raw"` spelling the table lacked —
                // a forward UAF hazard if the two lists were ever read for
                // opposing decisions. MIR callees are always underscore-mangled,
                // so the dotted form was unreachable and has been dropped.
                let moves_args = crate::mir::lower::runtime_abi::is_move_by_ffi(callee.as_str());
                if moves_args {
                    for arg in args {
                        if let MirValue::Use(local) = arg {
                            moved_to_ffi.insert(*local);
                        }
                    }
                }
            }
        }
    }

    // Locals whose pointer transitively flows into a `Return` value via
    // `Assign`/`Copy`/`Move`. The dealloc-safety analysis processes blocks
    // in linear order, so a forward edge such as `block 4: Assign 2 = 5`
    // (where `5` is initialized later in `block 5`) leaves `2` tainted
    // and `5` alloc-rooted. Without this fixpoint, dropping `5` at the
    // common return block would free the value the caller is about to
    // read through `2`. We compute the set iteratively from the seed
    // `return_locals` and exclude every member from `drop_locals` below.
    let return_alias_chain = compute_return_alias_chain(func, return_locals);
    let closure_drop_callees =
        closure_drop_callees(func, &dealloc_safe, return_locals, &return_alias_chain);

    // Collect locals that need dropping: Move types, not params, not the
    // return value. Collect in declaration order.
    //
    // We always drop user-declared locals (`let` bindings) of an owning
    // type. Compiler temporaries (`LocalKind::Temp`) are dropped
    // only when:
    //   * the temp's type is a built-in heap-owning type
    //     (`String`, `Vec`, `HashMap`), AND
    //   * the dealloc-safety analysis classified the temp as alloc-rooted
    //     (its value chain traces to a fresh-alloc callee like
    //     `ruxen_string_from` / `ruxen_vec_new` / `ruxen_hash_new`, with
    //     no aliasing or transfer).
    //
    // Class / Struct / Enum temps are still skipped: they almost always
    // arise from `MirInst::Alloc` followed by an `Assign` to a named
    // local, and the dealloc-safety pass already moves ownership to the
    // named local — dropping the temp would double-free.
    let drop_locals: Vec<LocalId> = func
        .locals
        .iter()
        .filter(|local| {
            // Must be a Move type. Primitives, references, tuples-of-Copy
            // etc. never reach MirInst::Alloc, so excluding them here is
            // safe. User-defined structs/classes/enums are heap-allocated
            // by codegen even when their fields qualify them for the
            // implicit `Copy` include (ruby-naming.spec.md §3.6); the
            // Copy include affects copy-on-assign semantics, not the
            // heap-allocation strategy. They still need scope-exit drop.
            let is_user_aggregate = matches!(
                local.ty,
                Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
            );
            if !is_user_aggregate && ty_is_effectively_copy(&local.ty, symbols) {
                return false;
            }
            // Must not be a parameter.
            if param_set.contains(&local.id) {
                return false;
            }
            // Must not be the return value (any block) or alias to one.
            if return_locals.contains(&local.id) || return_alias_chain.contains(&local.id) {
                return false;
            }
            // Must not be an allocation whose ownership moved into an
            // FFI-owned runtime structure, such as the task scheduler.
            if moved_to_ffi.contains(&local.id) {
                return false;
            }
            // Drop types that own heap memory:
            //   * Class/Struct/Enum — always heap-allocated via `ruxen_alloc`.
            //   * String/Vec/HashMap — heap-allocated via the built-in
            //     constructors (`ruxen_string_from`, `ruxen_vec_new`,
            //     `ruxen_hash_new`). Each gets a dedicated free helper at
            //     drop time (see drop callee dispatch below).
            //
            // The dealloc-safety analysis (`compute_dealloc_safe_locals`)
            // is what guards against double-free for locals whose value
            // came from a non-fresh source (e.g. a function-call return
            // pointing into the caller's heap).
            if heap_free_callee(&local.ty).is_none() {
                return false;
            }
            // Compiler temporaries: only drop heap-owning built-ins
            // whose value is a fresh allocation (see comment above).
            // Class temps with a registered `def drop` (user_drop_classes
            // includes the built-in std::process::Command + Output
            // entries for inner-heap cleanup) also need scope-exit drop
            // when alloc-rooted — without this, the chained
            // `Command.new(p).arg(a).status()` pattern would leak the
            // intermediate Command's args/env spines.
            if local.kind == LocalKind::Temp {
                let is_builtin_heap = matches!(
                    local.ty,
                    Ty::String | Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_)
                );
                if is_builtin_heap && dealloc_safe.contains(&local.id) {
                    return true;
                }
                if let Ty::Class { name, .. } = &local.ty {
                    if is_drop_class(symbols, user_drop_classes, name)
                        && dealloc_safe.contains(&local.id)
                    {
                        return true;
                    }
                }
                return false;
            }
            // User-named aggregate locals only need scope-exit drop when
            // ownership analysis says this frame still owns the heap
            // allocation. This is not just a synthetic-HIR concern:
            // move-by-FFI calls such as `Task.spawn_raw(fut)` remove
            // `fut` from dealloc_safe because the executor now owns the
            // allocation. Dropping solely because the local has a class
            // type frees the queued Future before the scheduler polls it.
            if is_user_aggregate {
                return dealloc_safe.contains(&local.id);
            }
            true
        })
        .map(|local| local.id)
        .collect();

    if drop_locals.is_empty() && closure_drop_callees.is_empty() {
        // A returned `String` rebuilt in a loop (`out = out + x`) is never
        // scope-exit dropped, but each value it held before the last is.
        let rebind_locals = free_prior_value_on_rebind(
            func,
            symbols,
            &dealloc_safe,
            &param_set,
            &moved_to_ffi,
            return_locals,
            &return_alias_chain,
        );
        zero_init_at_entry(func, &rebind_locals);
        return;
    }

    // Pre-compute, for each drop-eligible local, the user-drop class name
    // so we can emit `{ClassName}_drop(self)` immediately before the
    // dealloc + no-op cleanup. Indexed for cheap lookup inside the loop.
    // Build the drop-callee map. For each drop-eligible local, resolve
    // its class to a DefId in user_drop_classes and
    // record the synthesized `{ClassName}_drop` callee. Resolve that
    // callee through `ffi_alias_map` so lib-decl `def drop as
    // "ruxen_..."` aliases reach the right C symbol — without this
    // last step, emitted MirInst::Call would point at an undefined
    // `JoinHandle_drop`-style symbol and the user destructor would
    // silently fail to run while `ruxen_dealloc` still freed memory.
    // That was the failure mode behind rondo's
    // docs/ruxen-issues.md §W15: Thread.spawn JoinHandle was getting
    // bare-dealloc'd, freeing the struct out from under the spawned
    // thread.
    let drop_callees: HashMap<LocalId, String> = drop_locals
        .iter()
        .filter_map(|&id| {
            let local = func.locals.iter().find(|l| l.id == id)?;
            if let Ty::Class { name, .. } = &local.ty {
                if is_drop_class(symbols, user_drop_classes, name) {
                    let mangled = format!("{}_drop", name);
                    let callee = resolve_ffi_alias_callee(mangled);
                    return Some((id, callee));
                }
            }
            None
        })
        .collect();

    let mut all_drop_locals = drop_locals.clone();
    all_drop_locals.extend(closure_drop_callees.keys().copied());
    all_drop_locals.sort_unstable();
    all_drop_locals.dedup();

    let mut drop_instructions: HashMap<LocalId, Vec<MirInst>> = HashMap::new();
    for &local_id in &all_drop_locals {
        let mut instructions = Vec::new();
        if let Some(glue) = closure_drop_callees.get(&local_id) {
            let escaped_slots: Vec<usize> = glue
                .owned_capture_slots
                .iter()
                .filter_map(|(slot, source)| return_alias_chain.contains(source).then_some(*slot))
                .collect();
            if !escaped_slots.is_empty() {
                let captures = func.new_temp(Ty::Int);
                instructions.push(MirInst::GetField {
                    dest: captures,
                    base: local_id,
                    field_index: 1,
                });
                for slot in escaped_slots {
                    instructions.push(MirInst::SetField {
                        base: captures,
                        field_index: slot,
                        value: MirValue::Literal(Literal::Int(0)),
                    });
                }
            }
            instructions.push(MirInst::Call {
                dest: None,
                callee: glue.drop_fn.clone(),
                args: vec![MirValue::Use(local_id)],
            });
            instructions.push(MirInst::Drop { local: local_id });
            drop_instructions.insert(local_id, instructions);
            continue;
        }

        // User-defined `def drop` runs first, while the allocation is live.
        // Ownership-tainted values and a class's own drop body remain
        // excluded exactly as they were under return-only placement.
        let mut user_drop_freed_storage = false;
        if !in_user_drop_method && dealloc_safe.contains(&local_id) {
            if let Some(callee) = drop_callees.get(&local_id) {
                user_drop_freed_storage =
                    crate::mir::lower::runtime_abi::drop_frees_storage(callee);
                instructions.push(MirInst::Call {
                    dest: None,
                    callee: callee.clone(),
                    args: vec![MirValue::Use(local_id)],
                });
            }
        }

        // `MirInst::Drop` is a backend no-op; this type-directed call is
        // the actual free and remains gated by exclusive alloc provenance.
        if dealloc_safe.contains(&local_id) && !user_drop_freed_storage {
            let ty = func
                .locals
                .iter()
                .find(|candidate| candidate.id == local_id)
                .expect("drop_locals references a missing local")
                .ty
                .clone();
            // `drop_locals` admits only types `heap_free_callee` covers.
            instructions.extend(
                heap_free_call(func, symbols, &ty, local_id)
                    .into_iter()
                    .flatten(),
            );
        }
        instructions.push(MirInst::Drop { local: local_id });
        drop_instructions.insert(local_id, instructions);
    }

    let rebind_locals = free_prior_value_on_rebind(
        func,
        symbols,
        &dealloc_safe,
        &param_set,
        &moved_to_ffi,
        return_locals,
        &return_alias_chain,
    );

    // A marker-only Drop never reads the local in either backend. Only locals
    // with real closure/user/free glue need path-sensitive placement.
    let active_drop_locals: HashSet<LocalId> = all_drop_locals
        .iter()
        .copied()
        .filter(|local| closure_drop_callees.contains_key(local) || dealloc_safe.contains(local))
        .collect();
    let reachable = reachable_blocks(func);
    let sink_placements = branch_local_drop_blocks(func, &active_drop_locals, &reachable);
    let sunk_locals: HashSet<LocalId> = sink_placements.values().flatten().copied().collect();

    // Confined, acyclic branch locals can be destroyed immediately before the
    // branch's Goto. This is the zero-overhead path used by ordinary if/match
    // arm temporaries.
    for (block_id, locals) in sink_placements {
        for local in locals {
            func.blocks[block_id]
                .instructions
                .extend(drop_instructions[&local].clone());
        }
    }

    let scope_drop_locals: Vec<LocalId> = all_drop_locals
        .iter()
        .copied()
        .filter(|local| !sunk_locals.contains(local))
        .collect();
    let guarded_locals: HashSet<LocalId> = scope_drop_locals
        .iter()
        .copied()
        .filter(|local| {
            active_drop_locals.contains(local)
                && !local_initialized_at_all_returns(func, *local, &reachable)
        })
        .collect();

    // General CFG fallback: a local spanning multiple blocks but initialized
    // on only some return paths gets one flag. False is installed at entry and
    // true immediately after every initializing definition.
    let mut drop_flags: HashMap<LocalId, LocalId> = HashMap::new();
    for &local in &guarded_locals {
        let flag = func.new_local(format!("__drop_flag_{}", local), Ty::Bool, true);
        drop_flags.insert(local, flag);
    }
    if !drop_flags.is_empty() {
        for block in &mut func.blocks {
            let original = std::mem::take(&mut block.instructions);
            for inst in original {
                let mut flag_updates: Vec<(LocalId, bool)> = drop_flags
                    .iter()
                    .filter_map(|(&local, &flag)| {
                        if instruction_initializes_local(&inst, local) {
                            Some((flag, true))
                        } else if instruction_clears_local(&inst, local) {
                            Some((flag, false))
                        } else {
                            None
                        }
                    })
                    .collect();
                flag_updates.sort_unstable_by_key(|(flag, _)| *flag);
                block.instructions.push(inst);
                for (flag, value) in flag_updates {
                    block.instructions.push(MirInst::Assign {
                        dest: flag,
                        value: MirValue::Literal(Literal::Bool(value)),
                    });
                }
            }
        }
        let mut initializers: Vec<MirInst> = drop_flags
            .values()
            .copied()
            .map(|flag| MirInst::Assign {
                dest: flag,
                value: MirValue::Literal(Literal::Bool(false)),
            })
            .collect();
        initializers.sort_unstable_by_key(|inst| match inst {
            MirInst::Assign { dest, .. } => *dest,
            _ => unreachable!(),
        });
        initializers.append(&mut func.blocks[func.entry_block].instructions);
        func.blocks[func.entry_block].instructions = initializers;
    }

    zero_init_at_entry(func, &rebind_locals);

    let return_blocks: Vec<BlockId> = func
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| matches!(block.terminator, Terminator::Return(_)))
        .map(|(block_id, _)| block_id)
        .collect();
    for return_block in return_blocks {
        let return_terminator = std::mem::replace(
            &mut func.blocks[return_block].terminator,
            Terminator::Unreachable,
        );
        let mut current = return_block;
        for &local in scope_drop_locals.iter().rev() {
            if let Some(&flag) = drop_flags.get(&local) {
                let drop_block = func.blocks.len();
                let continuation_block = drop_block + 1;
                let mut drop_path = BasicBlock::new(drop_block);
                drop_path
                    .instructions
                    .extend(drop_instructions[&local].clone());
                drop_path.instructions.push(MirInst::Assign {
                    dest: flag,
                    value: MirValue::Literal(Literal::Bool(false)),
                });
                drop_path.terminator = Terminator::Goto(continuation_block);
                func.blocks.push(drop_path);
                func.blocks.push(BasicBlock::new(continuation_block));
                func.blocks[current].terminator = Terminator::Branch {
                    cond: MirValue::Use(flag),
                    then_block: drop_block,
                    else_block: continuation_block,
                };
                current = continuation_block;
            } else {
                func.blocks[current]
                    .instructions
                    .extend(drop_instructions[&local].clone());
            }
        }
        func.blocks[current].terminator = return_terminator;
    }
}

/// Free the value a built-in heap local (`String`, `Array`, `Hash`, `Set`)
/// held before it is redefined, and return the locals this applies to.
///
/// Scope-exit drop frees a local's LAST value. A local redefined in a loop
/// body (`out = out + x`, or any temporary such as an interpolation result)
/// or rebound in straight-line code held other values before that, and each
/// of them leaked. Every definition of such a local now frees the previous
/// value first; the local is zeroed at function entry (see
/// [`zero_init_at_entry`]) so the first definition frees NULL.
///
/// Sound for exactly the locals the dealloc-safety analysis proved own
/// every value they are given: each definition stores a fresh allocation
/// (any other source taints the local), and no value is ever moved out,
/// aliased into another local, or stored. A returned local qualifies: its
/// earlier values are not the one returned. When the definition reads the
/// local itself (`s = f(s)`), the old value is kept in a temporary and freed
/// after the definition instead.
fn free_prior_value_on_rebind(
    func: &mut MirFunction,
    symbols: &SymbolTable,
    dealloc_safe: &HashSet<LocalId>,
    params: &HashSet<LocalId>,
    moved_to_ffi: &HashSet<LocalId>,
    return_locals: &HashSet<LocalId>,
    return_alias_chain: &HashSet<LocalId>,
) -> Vec<LocalId> {
    // Lowering already frees the prior value of some rebinds itself (the
    // splat assignment `a, *rest = xs` drops the old `rest`). Such a local
    // has its own free discipline; a second free here would double-free.
    let freed_by_lowering: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter())
        .filter_map(|inst| match inst {
            MirInst::Call { callee, args, .. }
                if FREE_FAMILY_CALLEES.contains(&callee.as_str()) =>
            {
                match args.first() {
                    Some(MirValue::Use(local)) => Some(*local),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect();
    let candidates: Vec<(LocalId, Ty)> = func
        .locals
        .iter()
        .filter(|local| {
            !freed_by_lowering.contains(&local.id)
                && matches!(
                    local.ty,
                    Ty::String | Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_)
                )
                && dealloc_safe.contains(&local.id)
                && !params.contains(&local.id)
                && !moved_to_ffi.contains(&local.id)
                && (return_locals.contains(&local.id) || !return_alias_chain.contains(&local.id))
        })
        .map(|local| (local.id, local.ty.clone()))
        .collect();

    let mut rebound = Vec::new();
    for (local, ty) in candidates {
        let defs: Vec<(BlockId, usize)> = func
            .blocks
            .iter()
            .enumerate()
            .flat_map(|(block_id, block)| {
                block
                    .instructions
                    .iter()
                    .enumerate()
                    .filter(|(_, inst)| instruction_initializes_local(inst, local))
                    .map(move |(idx, _)| (block_id, idx))
            })
            .collect();
        let redefined = defs.len() > 1
            || defs.iter().any(|&(block_id, _)| {
                terminator_successors(&func.blocks[block_id].terminator)
                    .into_iter()
                    .any(|succ| block_reaches(func, succ, block_id))
            });
        if !redefined {
            continue;
        }
        // Rewrite the last definition first so earlier indices stay valid.
        for &(block_id, idx) in defs.iter().rev() {
            let reads_local =
                instruction_reads_local(&func.blocks[block_id].instructions[idx], local);
            let freed = if reads_local {
                func.new_temp(ty.clone())
            } else {
                local
            };
            let free = heap_free_call(func, symbols, &ty, freed)
                .expect("a rebind-free candidate has a heap-owning type");
            let instructions = &mut func.blocks[block_id].instructions;
            if reads_local {
                instructions.splice(idx + 1..idx + 1, free);
                instructions.insert(
                    idx,
                    MirInst::Assign {
                        dest: freed,
                        value: MirValue::Use(local),
                    },
                );
            } else {
                instructions.splice(idx..idx, free);
            }
        }
        rebound.push(local);
    }
    rebound
}

/// True when `inst` reads `local` (as opposed to only defining it).
fn instruction_reads_local(inst: &MirInst, local: LocalId) -> bool {
    match inst {
        MirInst::Assign { value, .. } => value_uses_local(value, local),
        MirInst::Call { args, .. } => args.iter().any(|arg| value_uses_local(arg, local)),
        MirInst::CallIndirect { callee, args, .. } => {
            *callee == local || args.iter().any(|arg| value_uses_local(arg, local))
        }
        MirInst::Copy { src, .. } | MirInst::Move { src, .. } => *src == local,
        _ => false,
    }
}

/// Zero `locals` at function entry, so a free that runs before a local's
/// first definition frees NULL. Inserted after every other drop rewrite so
/// the zero store is not mistaken for a definition by the flag analysis.
fn zero_init_at_entry(func: &mut MirFunction, locals: &[LocalId]) {
    let entry = func.entry_block;
    for (i, &local) in locals.iter().enumerate() {
        func.blocks[entry].instructions.insert(
            i,
            MirInst::Assign {
                dest: local,
                value: MirValue::Literal(Literal::Int(0)),
            },
        );
    }
}

/// The type-correct heap-free runtime callee for a drop-eligible local.
///
/// SINGLE SOURCE for the drop-callee selection consumed by BOTH scope-exit
/// drop elaboration (`insert_drops`) AND loop back-edge / break / continue
/// dealloc (`emit::emit_dealloc_loop_locals`). Before extraction the scope-exit
/// table lived inline in `insert_drops` and the loop path used a bare
/// `ruxen_dealloc` for every body-local — which silently leaked a loop-body
/// `Vec`/`Hash`'s data buffer (only the spine pointer was passed to
/// `ruxen_dealloc`, never the element-aware drop helper). Keeping the two in
/// lockstep means a loop-body `String`/`Vec`/`Hash`/`Set` is freed by the same
/// helper a function-scope one is.
///
/// One callee per container. What the container's slots own is not part of
/// the callee: it travels as the element descriptor arguments that
/// [`heap_free_call`] appends.
///
/// Returns `None` for a type that is not heap-owning (the caller decides
/// whether that is a bug or a no-op). `Some` is also exactly the drop pass's
/// eligibility test.
pub(super) fn heap_free_callee(ty: &Ty) -> Option<&'static str> {
    Some(match ty {
        Ty::String => "ruxen_string_free",
        Ty::Array(_) => "ruxen_vec_drop",
        Ty::Map(_, _) => "ruxen_hash_drop",
        Ty::Set(_) => "ruxen_set_drop",
        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } => "ruxen_dealloc",
        _ => return None,
    })
}

/// The complete drop of `value` of type `ty`: the [`heap_free_callee`] call
/// plus the element descriptors a container's drop glue needs (see the
/// `RUXEN_ELEM_*` block in `library/std/core/runtime/runtime.h`), preceded by
/// the instructions that compute a descriptor naming element glue. Temps for
/// those instructions are allocated in `func`. `None` exactly when
/// `heap_free_callee` is `None`.
pub(super) fn heap_free_call(
    func: &mut MirFunction,
    symbols: &SymbolTable,
    ty: &Ty,
    value: LocalId,
) -> Option<Vec<MirInst>> {
    let callee = heap_free_callee(ty)?;
    let descs = match ty {
        Ty::Array(elem) => vec![elem_desc(elem, symbols)],
        Ty::Map(key, val) => vec![ElemDesc::Word(key_elem_desc(key)), elem_desc(val, symbols)],
        Ty::Set(elem) => vec![ElemDesc::Word(key_elem_desc(elem))],
        _ => Vec::new(),
    };
    let mut insts = Vec::new();
    let mut args = vec![MirValue::Use(value)];
    for desc in descs {
        let mut new_temp = || func.new_temp(Ty::Int);
        args.push(elem_desc_value(desc, &mut new_temp, &mut insts));
    }
    insts.push(MirInst::Call {
        dest: None,
        callee: callee.to_string(),
        args,
    });
    Some(insts)
}

/// Wire values of the element-descriptor tags. Must stay in step with the
/// `RUXEN_ELEM_*` constants in `library/std/core/runtime/runtime.h`.
const ELEM_POD: i64 = 0;
const ELEM_CSTR: i64 = 1;
const ELEM_VEC: i64 = 3;
const ELEM_HASH: i64 = 4;
const ELEM_SET: i64 = 5;
const ELEM_OBJ: i64 = 6;

/// A descriptor is at most 16 four-bit tags (one `i64`).
const ELEM_DESC_MAX_TAGS: usize = 16;

/// How many tags may precede an OBJ tag. The glue address fills the word
/// above the OBJ tag, so it is shifted left four bits per preceding tag plus
/// four for OBJ itself; a 48-bit code address still fits after two.
const ELEM_OBJ_MAX_PREFIX_TAGS: usize = 2;

/// Name prefix of the synthesised per-type element glue functions
/// (`__ruxen_elem_glue_Point`), see [`Lowerer::synthesize_elem_glue`].
pub(super) const ELEM_GLUE_PREFIX: &str = "__ruxen_elem_glue_";

/// What one collection slot owns, as built by [`elem_desc`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ElemDesc {
    /// A descriptor word known at compile time.
    Word(i64),
    /// A stream ending in OBJ: `tags` holds the `prefix_tags` preceding tags
    /// and the OBJ tag, and the address of the function `glue` is stored
    /// above them at run time.
    Glue {
        tags: i64,
        prefix_tags: usize,
        glue: String,
    },
}

/// What one collection slot of type `ty` owns, as a `RUXEN_ELEM_*`
/// descriptor: a pre-order stream of 4-bit tags, low nibble first.
///
/// The ONE builder of element descriptors. Drop glue frees a slot per this
/// word and `ruxen_elem_clone` (the derived-collection helpers, rewritten in
/// `key_desc.rs`) copies a slot per the same word, so copy depth and drop
/// depth cannot drift apart. A type that does not fit is described to the
/// deepest level that fits, with POD below it: such a value leaks its
/// deepest elements rather than risking a free of something the copy
/// aliased.
pub(super) fn elem_desc(ty: &Ty, symbols: &SymbolTable) -> ElemDesc {
    for depth in (0..=ELEM_DESC_MAX_TAGS).rev() {
        let mut tags = Vec::new();
        let glue = push_elem_tags(ty, depth, symbols, &mut tags);
        if tags.len() > ELEM_DESC_MAX_TAGS {
            continue;
        }
        let word = tags
            .iter()
            .enumerate()
            .fold(0, |desc, (i, tag)| desc | (tag << (4 * i)));
        match glue {
            Some(glue) if tags.len() <= ELEM_OBJ_MAX_PREFIX_TAGS + 1 => {
                return ElemDesc::Glue {
                    tags: word,
                    prefix_tags: tags.len() - 1,
                    glue,
                }
            }
            None => return ElemDesc::Word(word),
            _ => {}
        }
    }
    ElemDesc::Word(ELEM_POD)
}

/// Append the tags for `ty` to `out`, and return the glue function when the
/// stream ends in an OBJ tag.
fn push_elem_tags(
    ty: &Ty,
    depth: usize,
    symbols: &SymbolTable,
    out: &mut Vec<i64>,
) -> Option<String> {
    match ty {
        Ty::String => {
            out.push(ELEM_CSTR);
            None
        }
        Ty::Array(elem) if depth > 0 => {
            out.push(ELEM_VEC);
            push_elem_tags(elem, depth - 1, symbols, out)
        }
        Ty::Map(key, val) if depth > 0 => {
            out.push(ELEM_HASH);
            out.push(key_elem_desc(key));
            push_elem_tags(val, depth - 1, symbols, out)
        }
        Ty::Set(elem) if depth > 0 => {
            out.push(ELEM_SET);
            out.push(key_elem_desc(elem));
            None
        }
        _ => {
            let glue = obj_glue_name(ty, symbols);
            out.push(if glue.is_some() { ELEM_OBJ } else { ELEM_POD });
            glue
        }
    }
}

/// The element glue for a slot holding an instance of `ty`, or `None` when
/// such a slot stays POD (the container never frees it). A non-generic class
/// or struct qualifies when it has a `clone` (derived or declared): a
/// derived-collection helper must be able to copy what the drop will free.
/// Generic classes stay POD because their methods are per-instantiation
/// symbols; enums because their payloads have no drop rule yet
/// (`docs/specs/ownership/drop.spec.md`, out of scope).
fn obj_glue_name(ty: &Ty, symbols: &SymbolTable) -> Option<String> {
    let name = match ty {
        Ty::Class { name, generic_args } | Ty::Struct { name, generic_args }
            if generic_args.is_empty() && !name.contains('.') =>
        {
            name
        }
        _ => return None,
    };
    crate::implicit_includes::derive_applies(ty, symbols, "Clone")
        .then(|| format!("{ELEM_GLUE_PREFIX}{name}"))
}

/// Emit the instructions that compute `desc` into `out` and return the
/// value to pass. A `Glue` descriptor ORs the glue function's address,
/// shifted above the tag stream, into the tags; a `Word` is a literal.
pub(super) fn elem_desc_value(
    desc: ElemDesc,
    new_temp: &mut dyn FnMut() -> LocalId,
    out: &mut Vec<MirInst>,
) -> MirValue {
    match desc {
        ElemDesc::Word(word) => MirValue::Literal(Literal::Int(word)),
        ElemDesc::Glue {
            tags,
            prefix_tags,
            glue,
        } => {
            let addr = new_temp();
            out.push(MirInst::FuncAddr {
                dest: addr,
                func_name: glue,
            });
            let shifted = new_temp();
            out.push(MirInst::BinOp {
                dest: shifted,
                op: BinOp::Shl,
                lhs: MirValue::Use(addr),
                rhs: MirValue::Literal(Literal::Int(4 * (prefix_tags as i64 + 1))),
            });
            let word = new_temp();
            out.push(MirInst::BinOp {
                dest: word,
                op: BinOp::BitOr,
                lhs: MirValue::Use(shifted),
                rhs: MirValue::Literal(Literal::Int(tags)),
            });
            MirValue::Use(word)
        }
    }
}

impl<'a> Lowerer<'a> {
    /// Define every element glue function an OBJ descriptor names
    /// (`FuncAddr __ruxen_elem_glue_<Type>`, see [`elem_desc`]). Runs once
    /// over the whole program, after every body (and every drop) is lowered.
    ///
    /// `glue(op, slot)` mirrors a scope-exit drop of a `<Type>` local for
    /// `op == RUXEN_ELEM_GLUE_DROP` (the type's `def drop`, then
    /// `ruxen_dealloc` unless that drop releases the storage itself), and
    /// returns `<Type>_clone(slot)` for `op == RUXEN_ELEM_GLUE_CLONE`.
    pub(super) fn synthesize_elem_glue(&self, mir: &mut MirProgram) -> Result<(), String> {
        let defined: HashSet<String> = mir.functions.iter().map(|f| f.name.clone()).collect();
        let mut wanted: Vec<String> = mir
            .functions
            .iter()
            .flat_map(|f| f.blocks.iter().flat_map(|b| b.instructions.iter()))
            .filter_map(|inst| match inst {
                MirInst::FuncAddr { func_name, .. }
                    if func_name.starts_with(ELEM_GLUE_PREFIX) && !defined.contains(func_name) =>
                {
                    Some(func_name.clone())
                }
                _ => None,
            })
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        for glue_name in wanted {
            let type_name = &glue_name[ELEM_GLUE_PREFIX.len()..];
            let clone_mangled = format!("{type_name}_clone");
            let clone_callee = self.resolve_ffi_alias_callee(clone_mangled.clone());
            if clone_callee == clone_mangled && !defined.contains(&clone_callee) {
                return Err(format!(
                    "element glue for `{type_name}` needs `{clone_mangled}`, which was not lowered"
                ));
            }
            mir.functions
                .push(self.build_elem_glue(&glue_name, type_name, clone_callee));
        }
        Ok(())
    }

    fn build_elem_glue(
        &self,
        glue_name: &str,
        type_name: &str,
        clone_callee: String,
    ) -> MirFunction {
        let mut glue = MirFunction::new(glue_name, Ty::Int);
        let op = glue.new_local("op", Ty::Int, false);
        let slot = glue.new_local("slot", Ty::Int, false);
        glue.params = vec![op, slot];

        let is_drop = glue.new_temp(Ty::Bool);
        let drop_block = glue.new_block();
        let clone_block = glue.new_block();
        let entry = glue.entry_block;
        glue.blocks[entry].instructions.push(MirInst::Compare {
            dest: is_drop,
            op: CmpOp::Eq,
            lhs: MirValue::Use(op),
            rhs: MirValue::Literal(Literal::Int(0)),
        });
        glue.blocks[entry].terminator = Terminator::Branch {
            cond: MirValue::Use(is_drop),
            then_block: drop_block,
            else_block: clone_block,
        };

        let mut frees_storage = false;
        if is_drop_class(self.symbols, &self.user_drop_classes, type_name) {
            let callee = self.resolve_ffi_alias_callee(format!("{type_name}_drop"));
            frees_storage = crate::mir::lower::runtime_abi::drop_frees_storage(&callee);
            glue.blocks[drop_block].instructions.push(MirInst::Call {
                dest: None,
                callee,
                args: vec![MirValue::Use(slot)],
            });
        }
        if !frees_storage {
            glue.blocks[drop_block].instructions.push(MirInst::Call {
                dest: None,
                callee: "ruxen_dealloc".to_string(),
                args: vec![MirValue::Use(slot)],
            });
        }
        let none = glue.new_temp(Ty::Int);
        glue.blocks[drop_block].instructions.push(MirInst::Assign {
            dest: none,
            value: MirValue::Literal(Literal::Int(0)),
        });
        glue.blocks[drop_block].terminator = Terminator::Return(Some(MirValue::Use(none)));

        let copy = glue.new_temp(Ty::Int);
        glue.blocks[clone_block].instructions.push(MirInst::Call {
            dest: Some(copy),
            callee: clone_callee,
            args: vec![MirValue::Use(slot)],
        });
        glue.blocks[clone_block].terminator = Terminator::Return(Some(MirValue::Use(copy)));
        glue
    }
}

/// Make a container's drop shallow when a pointer to one of its elements
/// escaped. Whole-program post-pass over the drops [`insert_drops`] (and the
/// loop and splat frees) emitted with deep element descriptors.
///
/// Reading an element (`a[i]`, `for x in a`, `h[k]`, a `.rx` accessor such as
/// `first`) yields an alias of a slot the container still owns; the language
/// has no move-out-of-index rule. When such an alias is stored somewhere that
/// frees it too (pushed into another collection, passed to a consuming
/// parameter, kept in a field, returned out of the container's frame, freed),
/// a deep drop of the source would free the element twice. For those
/// containers the descriptor arguments become POD: the spine is freed and the
/// elements are left to the other owner (or leak), never freed twice.
///
/// The analysis is flow-insensitive and interprocedural over the functions
/// in `mir`: a fixpoint computes, per function, which parameters can reach an
/// escaping sink and which can reach the return value. Unknown callees fall
/// back to `runtime_abi::callee_ownership`, whose default treats every
/// argument as escaping.
pub(super) fn demote_escaping_element_drops(mir: &mut MirProgram, symbols: &SymbolTable) {
    let index: HashMap<String, usize> = mir
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();
    let mut summaries: Vec<ParamFlow> = mir
        .functions
        .iter()
        .map(|f| ParamFlow {
            escapes: vec![false; f.params.len()],
            returned: vec![false; f.params.len()],
        })
        .collect();
    loop {
        let mut changed = false;
        for i in 0..mir.functions.len() {
            let func = &mir.functions[i];
            let flow = ElementFlow::analyze(func, &index, &summaries, symbols);
            let summary = flow.param_flow(func);
            if summary != summaries[i] {
                summaries[i] = summary;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for i in 0..mir.functions.len() {
        let flow = ElementFlow::analyze(&mir.functions[i], &index, &summaries, symbols);
        // A closure's drop function frees its captures; the body that may
        // have leaked their elements is the closure function itself.
        let closure_body_escapes = mir.functions[i]
            .name
            .strip_suffix("_drop")
            .and_then(|body| index.get(body))
            .is_some_and(|&body| summaries[body].escapes.iter().any(|e| *e));
        for block in &mut mir.functions[i].blocks {
            for inst in &mut block.instructions {
                let MirInst::Call { callee, args, .. } = inst else {
                    continue;
                };
                if !matches!(
                    callee.as_str(),
                    "ruxen_vec_drop" | "ruxen_hash_drop" | "ruxen_set_drop"
                ) {
                    continue;
                }
                let Some(MirValue::Use(container)) = args.first() else {
                    continue;
                };
                if closure_body_escapes || flow.elements_escaped(*container) {
                    for desc in args.iter_mut().skip(1) {
                        *desc = MirValue::Literal(Literal::Int(ELEM_POD));
                    }
                }
            }
        }
    }
}

/// What a function does with what its parameters point at, for its callers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParamFlow {
    /// Something reachable from the parameter may be stored past the call.
    escapes: Vec<bool>,
    /// The return value may point into the parameter.
    returned: Vec<bool>,
}

/// Points-to facts for one function. A ROOT is a container-typed local or a
/// non-scalar parameter. `same[l]` is the roots `l` may BE (an alias or a
/// reference of the root); `elem[l]` the roots `l` may point INTO.
struct ElementFlow {
    same: Vec<BTreeSet<LocalId>>,
    elem: Vec<BTreeSet<LocalId>>,
    escaped: BTreeSet<LocalId>,
    returned: BTreeSet<LocalId>,
}

impl ElementFlow {
    fn analyze(
        func: &MirFunction,
        index: &HashMap<String, usize>,
        summaries: &[ParamFlow],
        symbols: &SymbolTable,
    ) -> Self {
        let n = func.locals.len();
        let mut flow = ElementFlow {
            same: vec![BTreeSet::new(); n],
            elem: vec![BTreeSet::new(); n],
            escaped: BTreeSet::new(),
            returned: BTreeSet::new(),
        };
        for local in &func.locals {
            let is_container = matches!(
                local.ty.peel_refs(),
                Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_)
            );
            let is_param = func.params.contains(&local.id);
            if is_container || (is_param && !is_scalar(&local.ty)) {
                flow.same[local.id as usize].insert(local.id);
            }
        }
        loop {
            let before = flow.size();
            for block in &func.blocks {
                for inst in &block.instructions {
                    flow.transfer(func, inst, index, summaries, symbols);
                }
                if let Terminator::Return(Some(MirValue::Use(value))) = &block.terminator {
                    let reach = flow.reach(*value);
                    // Returning an alias INTO a local container outlives
                    // the container's drop at this frame's exit.
                    let into_locals: Vec<LocalId> = flow.elem[*value as usize]
                        .iter()
                        .copied()
                        .filter(|root| !func.params.contains(root))
                        .collect();
                    flow.escaped.extend(into_locals);
                    flow.returned.extend(reach);
                }
            }
            if flow.size() == before {
                return flow;
            }
        }
    }

    fn size(&self) -> usize {
        self.same
            .iter()
            .chain(&self.elem)
            .map(BTreeSet::len)
            .sum::<usize>()
            + self.escaped.len()
            + self.returned.len()
    }

    /// Every root `value` may be or point into.
    fn reach(&self, value: LocalId) -> BTreeSet<LocalId> {
        let value = value as usize;
        self.same[value].union(&self.elem[value]).copied().collect()
    }

    fn flow_into(&mut self, dest: LocalId, roots: BTreeSet<LocalId>) {
        self.elem[dest as usize].extend(roots);
    }

    fn transfer(
        &mut self,
        func: &MirFunction,
        inst: &MirInst,
        index: &HashMap<String, usize>,
        summaries: &[ParamFlow],
        symbols: &SymbolTable,
    ) {
        use crate::mir::lower::runtime_abi::{callee_ownership, user_callee_param_is_ref};
        let non_scalar = |local: LocalId| !is_scalar(&func.locals[local as usize].ty);
        match inst {
            MirInst::Assign {
                dest,
                value: MirValue::Use(src),
            }
            | MirInst::Copy { dest, src }
            | MirInst::Move { dest, src }
            | MirInst::Ref { dest, src }
            | MirInst::RefMut { dest, src } => {
                let same = self.same[*src as usize].clone();
                let elem = self.elem[*src as usize].clone();
                self.same[*dest as usize].extend(same);
                self.elem[*dest as usize].extend(elem);
            }
            MirInst::GetField { dest, base, .. }
            | MirInst::GetPayload {
                dest, src: base, ..
            } => {
                if non_scalar(*dest) {
                    let reach = self.reach(*base);
                    self.flow_into(*dest, reach);
                }
            }
            MirInst::SetField {
                value: MirValue::Use(value),
                ..
            } => {
                let reach = self.reach(*value);
                self.escaped.extend(reach);
            }
            MirInst::CallIndirect { dest, args, .. } => {
                let mut reach = BTreeSet::new();
                for arg in args {
                    if let MirValue::Use(arg) = arg {
                        reach.extend(self.reach(*arg));
                    }
                }
                if let Some(dest) = dest.filter(|d| non_scalar(*d)) {
                    self.flow_into(dest, reach.clone());
                }
                self.escaped.extend(reach);
            }
            MirInst::Call { dest, callee, args } => {
                let summary = index.get(callee.as_str()).map(|&i| &summaries[i]);
                let frees =
                    FREE_FAMILY_CALLEES.contains(&callee.as_str()) || callee.ends_with("_drop");
                let abi = callee_ownership(callee);
                let param_is_ref = if summary.is_none() && !frees && !abi.args_are_borrowed {
                    user_callee_param_is_ref(symbols, callee, args.len())
                } else {
                    Vec::new()
                };
                let mut result_reach = BTreeSet::new();
                for (i, arg) in args.iter().enumerate() {
                    let MirValue::Use(arg) = arg else {
                        continue;
                    };
                    let reach = self.reach(*arg);
                    let escapes = if frees {
                        // Freeing an element alias frees the slot's object.
                        self.escaped.extend(self.elem[*arg as usize].clone());
                        false
                    } else if let Some(summary) = summary {
                        if summary.returned.get(i).copied().unwrap_or(true) {
                            result_reach.extend(reach.iter().copied());
                        }
                        summary.escapes.get(i).copied().unwrap_or(true)
                    } else {
                        result_reach.extend(reach.iter().copied());
                        !(abi.borrows_first_arg && i == 0)
                            && (abi.arg_transfer.contains(i)
                                || !(abi.args_are_borrowed
                                    || param_is_ref.get(i).copied().unwrap_or(false)))
                    };
                    if escapes {
                        // A collection that takes in an alias (or a value of
                        // unknown provenance: a parameter) may now hold the
                        // same object twice, or one another owner frees, so
                        // its own drop must not free it either. An owned
                        // local moved in is not an alias.
                        let aliased = !self.elem[*arg as usize].is_empty()
                            || self.same[*arg as usize]
                                .iter()
                                .any(|r| func.params.contains(r));
                        if summary.is_none() && abi.arg_transfer.contains(i) && aliased {
                            if let Some(MirValue::Use(target)) = args.first() {
                                self.escaped.extend(self.reach(*target));
                            }
                        }
                        self.escaped.extend(reach);
                    }
                }
                if let Some(dest) = dest.filter(|d| non_scalar(*d)) {
                    let fresh_owned = summary.is_none()
                        && abi.result == crate::mir::lower::runtime_abi::ResultOwnership::Fresh
                        && !func.locals[dest as usize].ty.any(&mut |t| {
                            matches!(
                                t,
                                Ty::Ref(_)
                                    | Ty::RefMut(_)
                                    | Ty::RefLifetime(_, _)
                                    | Ty::RefMutLifetime(_, _)
                                    | Ty::Tuple(_)
                            )
                        });
                    if !fresh_owned {
                        self.flow_into(dest, result_reach);
                    }
                }
            }
            _ => {}
        }
    }

    /// True when an element of the container `local` may be owned elsewhere.
    fn elements_escaped(&self, local: LocalId) -> bool {
        self.same[local as usize].iter().any(|root| {
            self.escaped.contains(root)
                || self
                    .escaped
                    .iter()
                    .any(|escaped| self.same[*escaped as usize].contains(&local))
        })
    }

    fn param_flow(&self, func: &MirFunction) -> ParamFlow {
        ParamFlow {
            escapes: func
                .params
                .iter()
                .map(|&p| self.elements_escaped(p))
                .collect(),
            returned: func
                .params
                .iter()
                .map(|p| self.returned.contains(p))
                .collect(),
        }
    }
}

/// The drop callees that free their first argument. Must list what
/// [`heap_free_callee`] can return.
const FREE_FAMILY_CALLEES: &[&str] = &[
    "ruxen_dealloc",
    "ruxen_string_free",
    "ruxen_vec_drop",
    "ruxen_hash_drop",
    "ruxen_set_drop",
];

/// A register value that can never carry a pointer.
fn is_scalar(ty: &Ty) -> bool {
    ty.is_scalar_primitive() || matches!(ty, Ty::Unit | Ty::Never)
}

/// The descriptor of a `Hash` key or a `Set` element: CSTR or POD only.
/// Those tables copy keys by their `key_kind` (`ruxen_set_insert_owned`,
/// `union`, `from_iter`), which can only say "a string" or "a word", so a
/// deeper key ownership would free what the copy aliased.
fn key_elem_desc(ty: &Ty) -> i64 {
    match ty {
        Ty::String => ELEM_CSTR,
        _ => ELEM_POD,
    }
}

/// Compute the set of locals whose value provenance is a fresh
/// `MirInst::Alloc` (or a chain of `Assign`/`Copy`/`Move` from one) AND
/// whose ownership of that allocation has not been aliased away to
/// another local or transferred to a callee/aggregate.
///
/// These are the locals safe to pass to `ruxen_dealloc` at scope exit:
/// freeing them releases an allocation that this frame owns exclusively.
///
/// A local is excluded (NOT dealloc-safe) when:
///
///   * Its provenance is not a fresh `Alloc` — for example `GetField` /
///     `GetPayload` (pointer into a parent struct), a function-call
///     return value (caller-owned semantics), `Ref`/`RefMut`, or numeric
///     /string literal output.
///   * Ownership escapes the current frame: the local is passed as a
///     `Use(local)` argument to any `Call`/`CallIndirect`, or stored
///     into another aggregate via `SetField`. After such a transfer the
///     callee/aggregate may have copied or freed the pointer, so a
///     scope-exit dealloc could double-free or use-after-free.
///   * Its value flows (via `Assign`/`Copy`/`Move`) into another local.
///     The MIR doesn't deep-copy structs/classes/enums on assignment —
///     the destination receives the same pointer — so dealloc'ing both
///     would double-free the shared allocation. We move the dealloc
///     responsibility to the last local in each propagation chain.
fn compute_dealloc_safe_locals(
    func: &MirFunction,
    symbols: &SymbolTable,
) -> std::collections::HashSet<LocalId> {
    use std::collections::HashSet;

    // `alloc_rooted`: locals whose value currently traces back to a fresh
    // `MirInst::Alloc` and that have not yet been aliased away or
    // transferred. We process instructions in a single forward pass over
    // a flattened block order; an instruction may both *grant* a local
    // alloc-rooted status (e.g. `Alloc dest=L`) and *revoke* it (e.g.
    // `Use(L)` as a Call argument later in the same block).
    let mut alloc_rooted: HashSet<LocalId> = HashSet::new();
    // `tainted_perm`: once a local was passed to a callee/aggregate or
    // its value was propagated to another local, it can never become
    // dealloc-safe. Re-allocations into the same id (rare) would still
    // honor this — we conservatively keep the local out of the safe set.
    let mut tainted_perm: HashSet<LocalId> = HashSet::new();

    // `match_payload_ptrs`: locals produced by `GetPayload` whose source
    // is a `Result`/`Option` value. A `GetField` reading out of such a
    // pointer extracts an OWNED payload (the sum type owned it; the match
    // moves ownership to the binding). We only seed this for Result /
    // Option — NOT user enums — because Result/Option are never themselves
    // drop-elaborated (not in the drop-eligible `Ty` set), so dropping the
    // extracted payload cannot double-free with a container drop; user
    // enums ARE drop-elaborated and their drop recurses into the payload.
    let mut match_payload_ptrs: HashSet<LocalId> = HashSet::new();
    let local_ty = |id: LocalId| func.locals.iter().find(|l| l.id == id).map(|l| &l.ty);

    // Single forward pass — block order is the lowering order, which
    // matches program execution order for the cases we care about.
    // Loops/back-edges are not modeled; back-edges only matter when an
    // alloc-rooted local is mutated mid-loop, which the lowerer
    // currently never produces for user `let` bindings of Class/Struct
    // /Enum types.
    for block in &func.blocks {
        for inst in &block.instructions {
            match inst {
                MirInst::Alloc { dest, .. } | MirInst::StackAlloc { dest, .. } => {
                    if !tainted_perm.contains(dest) {
                        alloc_rooted.insert(*dest);
                    }
                }
                MirInst::Assign { dest, value } => {
                    if let MirValue::Use(src) = value {
                        if alloc_rooted.contains(src) {
                            // Pointer-copy: `dest` now ALIASES `src`'s
                            // allocation. `src` must ALWAYS lose its
                            // alloc-rooted status — the pointer now lives in
                            // `dest`, so freeing `src` independently at scope
                            // exit would double-free. This holds even when
                            // `dest` is already `tainted_perm` (e.g. a loop
                            // body-local pre-zeroed by `prepend_zero_init`
                            // *then* assigned the `ruxen_string_from` temp):
                            // the back-edge frees `dest`, so the producing temp
                            // `src` must NOT also be scope-exit freed. We hand
                            // dealloc responsibility to `dest` ONLY if `dest`
                            // is still eligible (not permanently tainted);
                            // otherwise the allocation is owned by whatever
                            // frees `dest` (the loop back-edge), and neither
                            // `src` nor `dest` is scope-exit dropped.
                            tainted_perm.insert(*src);
                            alloc_rooted.remove(src);
                            if !tainted_perm.contains(dest) {
                                alloc_rooted.insert(*dest);
                            }
                            continue;
                        }
                    }
                    // Literal or non-alloc-rooted source → permanently
                    // exclude dest. (We also drop any prior alloc-root
                    // status, since the local is being overwritten.)
                    tainted_perm.insert(*dest);
                    alloc_rooted.remove(dest);
                }
                MirInst::Copy { dest, src } | MirInst::Move { dest, src } => {
                    if alloc_rooted.contains(src) {
                        // Same aliasing rule as the `Assign` arm above: a
                        // copy/move out of an alloc-rooted `src` always strips
                        // `src`'s ownership (the pointer now lives in `dest`);
                        // `dest` inherits it only if still eligible.
                        tainted_perm.insert(*src);
                        alloc_rooted.remove(src);
                        if !tainted_perm.contains(dest) {
                            alloc_rooted.insert(*dest);
                        }
                    } else {
                        tainted_perm.insert(*dest);
                        alloc_rooted.remove(dest);
                    }
                }
                // Every other instruction that defines a local produces
                // a value that is NOT a fresh allocation owned by this
                // frame — taint the destination.
                // Payload pointer of a `match` / `?` extraction. The
                // pointer itself is an intermediate (taint it), but if it
                // reads out of a Result/Option we remember it so the
                // GetField that pulls the payload field out knows that
                // field is OWNED (the sum type owned it; the match transfers
                // ownership to the binding). Scoped to Result/Option — see
                // `match_payload_ptrs` doc above.
                MirInst::GetPayload { dest, src, .. } => {
                    if matches!(local_ty(*src), Some(Ty::Result(_, _)) | Some(Ty::Option(_))) {
                        match_payload_ptrs.insert(*dest);
                    }
                    tainted_perm.insert(*dest);
                    alloc_rooted.remove(dest);
                }
                // Field read. Out of a match payload pointer it is an OWNED
                // extraction of a heap-owning value → alloc-root it so the
                // drop-eligibility filter (plus the return-alias / move-out
                // guards) can give it a scope-exit drop; without this a
                // `File` matched out of `File.open(p)` leaks its fd. A field
                // read of anything else is a borrowed view into a parent the
                // frame already owns — tainting keeps us from double-freeing.
                MirInst::GetField { dest, base, .. } => {
                    let owns_heap = matches!(
                        local_ty(*dest),
                        Some(Ty::Class { .. })
                            | Some(Ty::Struct { .. })
                            | Some(Ty::String)
                            | Some(Ty::Array(_))
                            | Some(Ty::Map(_, _))
                            | Some(Ty::Set(_))
                    );
                    if match_payload_ptrs.contains(base)
                        && owns_heap
                        && !tainted_perm.contains(dest)
                    {
                        alloc_rooted.insert(*dest);
                    } else {
                        tainted_perm.insert(*dest);
                        alloc_rooted.remove(dest);
                    }
                }
                MirInst::BinOp { dest, .. }
                | MirInst::Negate { dest, .. }
                | MirInst::Not { dest, .. }
                | MirInst::Compare { dest, .. }
                | MirInst::GetTag { dest, .. }
                | MirInst::Ref { dest, .. }
                | MirInst::RefMut { dest, .. }
                | MirInst::StringLiteral { dest, .. }
                | MirInst::FuncAddr { dest, .. }
                | MirInst::FloatToBits { dest, .. }
                | MirInst::BitsToFloat { dest, .. }
                | MirInst::DataAddr { dest, .. } => {
                    // Phase B-5: a static data pointer (vtable /
                    // class_info) is not a heap allocation — taint
                    // the dest so dealloc-tracking treats it as a
                    // non-owning pointer. `FloatToBits` likewise produces
                    // a scalar integer, never a heap allocation.
                    tainted_perm.insert(*dest);
                    alloc_rooted.remove(dest);
                }
                MirInst::Call { dest, callee, args } => {
                    // Runtime callee ownership: see runtime_abi.rs. ALL six
                    // formerly-inline predicates (FRESH_ALLOC_CALLEES,
                    // is_runtime_consume_helper, is_runtime_borrow_helper ×3
                    // rebinds, is_move_by_ffi_callee, is_pointer_store_helper,
                    // borrows_first_arg) + transfer_indices now live there,
                    // classified once by `callee_ownership`.
                    use crate::mir::lower::runtime_abi::{
                        callee_ownership, user_callee_param_is_ref, ResultOwnership,
                    };
                    let abi = callee_ownership(callee.as_str());

                    if let Some(d) = dest {
                        if abi.result == ResultOwnership::Fresh && !tainted_perm.contains(d) {
                            alloc_rooted.insert(*d);
                        } else {
                            tainted_perm.insert(*d);
                            alloc_rooted.remove(d);
                        }
                    }
                    // A USER callee (not classified by the runtime-ABI tables)
                    // hits the conservative default: `args_are_borrowed == false`
                    // taints EVERY arg. That is wrong for a `&T` / `&var T`
                    // parameter — the value is auto-borrowed, the callee never
                    // owns it (drop elaboration skips a function's own params),
                    // so the source must stay drop-eligible and be freed at the
                    // CALLER's scope exit. Resolve per-param ref-ness by callee
                    // NAME and skip tainting the borrow slots. By-value params
                    // keep their taint (no double-free regression). Only computed
                    // when the ABI would otherwise default-taint, so runtime
                    // callees are byte-for-byte unaffected (parity oracle).
                    let param_is_ref: Vec<bool> = if !abi.args_are_borrowed {
                        user_callee_param_is_ref(symbols, callee.as_str(), args.len())
                    } else {
                        Vec::new()
                    };
                    // Reproduces the old arg loop precedence exactly:
                    // arg0-borrow continue, then explicit transfer (folds in
                    // the old pointer-store idx==1 case via the ArgMask), then
                    // borrow continue, then the user-`&T`-param borrow, then
                    // default taint.
                    for (idx, arg) in args.iter().enumerate() {
                        if let MirValue::Use(l) = arg {
                            if abi.borrows_first_arg && idx == 0 {
                                continue;
                            }
                            if abi.arg_transfer.contains(idx) {
                                tainted_perm.insert(*l);
                                alloc_rooted.remove(l);
                                continue;
                            }
                            if abi.args_are_borrowed {
                                continue;
                            }
                            // User callee with a declared `&T` param at this
                            // slot: auto-borrow, do not taint.
                            if param_is_ref.get(idx).copied().unwrap_or(false) {
                                continue;
                            }
                            tainted_perm.insert(*l);
                            alloc_rooted.remove(l);
                        }
                    }
                }
                MirInst::CallIndirect { dest, args, .. } => {
                    if let Some(d) = dest {
                        tainted_perm.insert(*d);
                        alloc_rooted.remove(d);
                    }
                    for arg in args {
                        if let MirValue::Use(l) = arg {
                            tainted_perm.insert(*l);
                            alloc_rooted.remove(l);
                        }
                    }
                }
                MirInst::SetField { value, .. } => {
                    // Storing a local into another aggregate transfers
                    // ownership (the aggregate now references it).
                    if let MirValue::Use(l) = value {
                        tainted_perm.insert(*l);
                        alloc_rooted.remove(l);
                    }
                }
                // No-op for instructions that don't define or move a
                // local.
                MirInst::SetTag { .. } | MirInst::Drop { .. } | MirInst::Nop => {}
            }
        }
    }

    // §4.1 Increment 3 — drop the block_on ROOT future at block_on exit.
    //
    // The block_on rewriter emits `var __block_on_fut_N = <fut_expr>`, where
    // `<fut_expr>` is an async-fn wrapper call (`worker()`) returning a fresh
    // `__*Future`. The conservative user-callee taint rule marks that
    // call-result local NOT-alloc-rooted, so `__block_on_fut_N` was never
    // scope-exit-dropped — the root future (and, via the Increment-2 glue, its
    // live `__sub_i` sub-futures) LEAKED on every `block_on(async_fn())`, and a
    // cooperatively-cancelled future's resources were never cleaned. Force such
    // a local back into `alloc_rooted` so `insert_drops` emits its
    // `<Class>_drop` (Increment-2 glue) + `ruxen_dealloc`. This is the spec
    // §4.1 "block_on-exit path calls Drop_dynamic_drop on the future".
    //
    // SOUNDNESS is STRUCTURAL, not by name: we force-root only when the local
    // is NEVER CONSUMED — never passed by-value to a call, stored into an
    // aggregate (`SetField`), moved/copied/assigned INTO another local, or
    // returned. A genuine block_on root is only ever BORROWED
    // (`(&var fut).poll(cx)`), so it satisfies this; a (hypothetical) user
    // local that merely shares the `__block_on_fut_` name but IS consumed (e.g.
    // moved into another call) fails the check and is left tainted — no
    // double-free. The name prefix only SCOPES the rule to compiler-generated
    // block_on roots (cheap filter); the consume check is what makes it safe.
    let mut consumed: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for inst in &block.instructions {
            match inst {
                MirInst::Call { args, .. } | MirInst::CallIndirect { args, .. } => {
                    for a in args {
                        if let MirValue::Use(l) = a {
                            consumed.insert(*l);
                        }
                    }
                }
                MirInst::SetField { value, .. } | MirInst::Assign { value, .. } => {
                    if let MirValue::Use(l) = value {
                        consumed.insert(*l);
                    }
                }
                MirInst::Copy { src, .. } | MirInst::Move { src, .. } => {
                    consumed.insert(*src);
                }
                _ => {}
            }
        }
        if let Terminator::Return(Some(MirValue::Use(l))) = &block.terminator {
            consumed.insert(*l);
        }
    }
    for local in &func.locals {
        if local.name.starts_with("__block_on_fut_")
            && matches!(local.ty, Ty::Class { .. })
            && !consumed.contains(&local.id)
        {
            alloc_rooted.insert(local.id);
        }
    }

    alloc_rooted
}

/// Compute the transitive set of locals whose value flows into a `Return`
/// terminator via `Assign` / `Copy` / `Move` or into a returned aggregate.
///
/// Seed the set with the locals that appear directly in any
/// `Return(Some(Use(L)))` terminator, then iterate to a fixpoint: every
/// `Assign { dest: D, value: Use(S) }` (and `Copy`/`Move`) where `D` is
/// already in the set adds `S` as well — `S` aliases the same heap as
/// `D`, so freeing it would corrupt the returned value.
///
/// We need this because `compute_dealloc_safe_locals` walks blocks in
/// linear order. A function whose return block precedes the producer
/// block (typical for `match`-arm-as-tail-expression) leaves the
/// intermediate alloc-rooted, which would otherwise be picked up as a
/// drop candidate. Excluding the alias chain keeps the dealloc safe.
fn compute_return_alias_chain(
    func: &MirFunction,
    seed_locals: &HashSet<LocalId>,
) -> HashSet<LocalId> {
    use std::collections::HashSet;
    let mut chain: HashSet<LocalId> = seed_locals.clone();
    loop {
        let before = chain.len();
        for block in &func.blocks {
            for inst in &block.instructions {
                match inst {
                    MirInst::Assign { dest, value } => {
                        if chain.contains(dest) {
                            if let MirValue::Use(src) = value {
                                chain.insert(*src);
                            }
                        }
                    }
                    MirInst::Copy { dest, src } | MirInst::Move { dest, src } => {
                        if chain.contains(dest) {
                            chain.insert(*src);
                        }
                    }
                    MirInst::SetField {
                        base,
                        value: MirValue::Use(src),
                        ..
                    } => {
                        if chain.contains(base) {
                            chain.insert(*src);
                        }
                    }
                    _ => {}
                }
            }
        }
        if chain.len() == before {
            break;
        }
    }
    chain
}

#[cfg(test)]
mod elem_desc_tests {
    use super::*;

    fn array(elem: Ty) -> Ty {
        Ty::Array(Box::new(elem))
    }

    fn word(ty: &Ty) -> i64 {
        match elem_desc(ty, &SymbolTable::new()) {
            ElemDesc::Word(word) => word,
            glue => panic!("expected a compile-time word, got {glue:?}"),
        }
    }

    #[test]
    fn nested_types_encode_pre_order_low_nibble_first() {
        // Array[String]: VEC then CSTR.
        assert_eq!(word(&array(Ty::String)), 0x13);
        // Hash[String, Array[Int]]: HASH, key CSTR, value VEC then POD.
        let map = Ty::Map(Box::new(Ty::String), Box::new(array(Ty::Int)));
        assert_eq!(word(&map), 0x314);
        // A Set element is a key: never deeper than CSTR.
        assert_eq!(word(&Ty::Set(Box::new(array(Ty::String)))), 0x05);
    }

    #[test]
    fn a_type_deeper_than_sixteen_tags_is_cut_to_pod() {
        let mut deep = Ty::String;
        for _ in 0..20 {
            deep = array(deep);
        }
        // Fifteen VEC tags and a POD leaf: the deepest levels leak rather
        // than overflow the word.
        assert_eq!(word(&deep), 0x0333_3333_3333_3333);
    }

    #[test]
    fn a_glue_descriptor_ors_the_shifted_glue_address_into_its_tags() {
        let mut next = 10;
        let mut new_temp = || {
            next += 1;
            next
        };
        let mut out = Vec::new();
        let desc = ElemDesc::Glue {
            tags: 0x63,
            prefix_tags: 1,
            glue: format!("{ELEM_GLUE_PREFIX}Point"),
        };
        let value = elem_desc_value(desc, &mut new_temp, &mut out);
        assert!(matches!(value, MirValue::Use(13)));
        assert!(matches!(
            &out[..],
            [
                MirInst::FuncAddr { dest: 11, func_name },
                MirInst::BinOp { dest: 12, op: BinOp::Shl, rhs: MirValue::Literal(Literal::Int(8)), .. },
                MirInst::BinOp { dest: 13, op: BinOp::BitOr, rhs: MirValue::Literal(Literal::Int(0x63)), .. },
            ] if func_name == "__ruxen_elem_glue_Point"
        ));
    }
}
