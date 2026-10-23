use super::*;

mod closures;
pub(in crate::mir::lower) mod copies;
mod flow;
mod glue;

pub(super) use closures::*;
pub(crate) use closures::{CLOSURE_PAIR_CAPTURES, CLOSURE_PAIR_FN};
pub(super) use flow::*;
pub(super) use glue::*;

/// The ONE parameter `func` returns on every path, by MIR argument slot, if
/// any: every `Return(Some(Use(local)))` must have `local` proven to alias
/// that same parameter on every execution path reaching it (a forward
/// dataflow over the CFG, see `elide_returns_self_realloc`). A
/// receiver-builder returning `self` is slot 0. Shared by
/// `elide_returns_self_realloc` and `CallSummaries::passthrough_param`.
pub(super) fn single_returned_param(func: &crate::mir::nodes::MirFunction) -> Option<usize> {
    use crate::mir::nodes::{MirValue, Terminator};
    let n = func.blocks.len();
    if n == 0 {
        return None;
    }

    let preds = crate::mir::dataflow::predecessors(func);

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
        agreed_index
    } else {
        None
    }
}

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
    let returns_param = returned_params(mir);
    if returns_param.is_empty() {
        return;
    }
    for func in &mut mir.functions {
        let sites = elided_rebind_frees(func, &returns_param);
        // Remove in reverse so earlier indices stay valid.
        for &(block, index) in sites.iter().rev() {
            func.blocks[block].instructions.remove(index);
        }
    }
}

/// For each function that hands back one parameter on every path, that
/// parameter's position, by function name.
pub(super) fn returned_params(
    mir: &crate::mir::nodes::MirProgram,
) -> std::collections::HashMap<String, usize> {
    // For each function, the SINGLE parameter index it
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
    mir.functions
        .iter()
        .filter_map(|func| single_returned_param(func).map(|k| (func.name.clone(), k)))
        .collect()
}

/// The instructions of `func` that [`elide_returns_self_realloc`] deletes,
/// as (block, index) in ascending order: each rebind free `ruxen_dealloc(R)`
/// (with the field glue call right before it) whose next instruction is
/// `R = X` for an `X` a call hands back as `R`'s own object. The loan check
/// skips the same instructions, which never run.
pub(super) fn elided_rebind_frees(
    func: &MirFunction,
    returns_param: &std::collections::HashMap<String, usize>,
) -> Vec<(BlockId, usize)> {
    use crate::mir::nodes::{MirInst, MirValue};
    let mut sites = Vec::new();
    for (block_id, block) in func.blocks.iter().enumerate() {
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
                                            call_args
                                                .iter()
                                                .skip(1)
                                                .any(|a| matches!(a, MirValue::Use(l) if l == r))
                                        } else if let Some(&k) = returns_param.get(callee_name) {
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
                                            // The rebind frees the old
                                            // object's fields right before
                                            // its storage; they belong to
                                            // the object handed back too.
                                            let frees_fields = i > 0
                                                && matches!(
                                                    &block.instructions[i - 1],
                                                    MirInst::Call { callee, args, .. }
                                                        if matches!(args.as_slice(), [MirValue::Use(l)] if l == r)
                                                            && func.field_glue_calls.contains_key(callee)
                                                );
                                            if frees_fields {
                                                sites.push((block_id, i - 1));
                                            }
                                            sites.push((block_id, i));
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
    }
    sites
}

/// The successor block ids reachable from a terminator. Used to build the
/// predecessor map for `elide_returns_self_realloc`'s CFG dataflow.
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
                if let Some(dest) = other.dest() {
                    s.remove(&dest);
                }
            }
        }
    }
    s
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
        | MirInst::FieldAddr { dest, .. }
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
        | MirInst::Convert { dest, .. }
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
    inst.dest() == Some(local) || inst.read_locals().contains(&local)
}

fn terminator_mentions_local(terminator: &Terminator, local: LocalId) -> bool {
    terminator.read_local() == Some(local)
}

fn reachable_blocks(func: &MirFunction) -> Vec<bool> {
    let mut reachable = vec![false; func.blocks.len()];
    let mut pending = vec![func.entry_block];
    while let Some(block_id) = pending.pop() {
        if block_id >= func.blocks.len() || reachable[block_id] {
            continue;
        }
        reachable[block_id] = true;
        pending.extend(func.blocks[block_id].terminator.successors());
    }
    reachable
}

fn local_initialized_at_all_returns(
    func: &MirFunction,
    local: LocalId,
    reachable: &[bool],
) -> bool {
    let predecessors: Vec<Vec<BlockId>> = crate::mir::dataflow::predecessors(func)
        .into_iter()
        .map(|preds| preds.into_iter().filter(|&pred| reachable[pred]).collect())
        .collect();

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
        pending.extend(func.blocks[block_id].terminator.successors());
    }
    false
}

/// Confined, acyclic branch locals: a local initialized on some paths only
/// whose every use, and every use of a borrow of it (`lifetimes`, the
/// local alone when absent), sits in one block ending in a `Goto`, is
/// dropped at the end of that block.
fn branch_local_drop_blocks(
    func: &MirFunction,
    locals: &HashSet<LocalId>,
    reachable: &[bool],
    lifetimes: &HashMap<LocalId, HashSet<LocalId>>,
) -> HashMap<BlockId, Vec<LocalId>> {
    let mut placements: HashMap<BlockId, Vec<LocalId>> = HashMap::new();
    for &local in locals {
        if local_initialized_at_all_returns(func, local, reachable) {
            continue;
        }
        let alone = HashSet::from([local]);
        let lifetime = lifetimes.get(&local).unwrap_or(&alone);
        let mention_blocks: Vec<BlockId> = func
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                lifetime.iter().any(|&l| {
                    block
                        .instructions
                        .iter()
                        .any(|inst| instruction_mentions_local(inst, l))
                        || terminator_mentions_local(&block.terminator, l)
                })
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

/// One statement of a lowered body that created locals: it ends at the
/// `nop_ordinal`-th `MirInst::Nop` marker of `block`, and `locals` are the
/// locals allocated while it was lowered. A temporary among them that is
/// used only before the marker is dropped there (`statement_drop_sites`).
/// The marker is found by its ordinal, not an index, because later passes
/// (`key_desc`) insert instructions; none reorders them.
#[derive(Debug, Clone)]
pub(super) struct StmtScope {
    pub(super) block: BlockId,
    pub(super) nop_ordinal: usize,
    pub(super) locals: std::ops::Range<LocalId>,
}

/// The statement scopes of the function being lowered, and how many
/// markers each block holds so far.
#[derive(Debug, Default)]
pub(super) struct StmtScopeLog {
    pub(super) scopes: Vec<StmtScope>,
    markers_in_block: HashMap<BlockId, usize>,
}

impl StmtScopeLog {
    /// The ordinal the next marker emitted into `block` will have.
    pub(super) fn next_marker(&mut self, block: BlockId) -> usize {
        let count = self.markers_in_block.entry(block).or_insert(0);
        let ordinal = *count;
        *count += 1;
        ordinal
    }
}

/// What drop elaboration knows about each lowered user function as a
/// callee, computed over the whole program before any drop is placed.
#[derive(Debug, Default)]
pub(super) struct CallSummaries {
    /// Functions whose every return hands back a fresh value their own frame
    /// owned outright (`returns_fresh`): the caller owns the result.
    pub(super) fresh: HashSet<String>,
    /// Per function, which MIR parameters (receiver first) a returned value
    /// may alias. An argument bound to such a parameter is not a mere
    /// borrow: the call's result can be that very value.
    pub(super) returned_params: HashMap<String, Vec<bool>>,
    /// Functions that return one parameter on every path (`Kernel#tap`,
    /// a builder returning `self`), by MIR argument slot: the call's result
    /// IS that argument, so ownership passes from the argument to the result.
    pub(super) passthrough_param: HashMap<String, usize>,
    /// Per function, which MIR parameters take a block (`Fn[...]`,
    /// `Ty::is_borrowed_block`), read off the lowered body itself. A block
    /// parameter only calls its closure (E1018), so the caller keeps and
    /// frees the pair; this covers callees name resolution cannot find, such
    /// as a mixin default or `Kernel#tap` copied into an implementor. Every
    /// body drop elaboration sees has an entry; it is passed to
    /// `runtime_abi::user_callee_param_is_ref`, the one reader.
    pub(super) block_params: HashMap<String, Vec<bool>>,
    /// Which argument slots of each `.rx` FFI declaration take an owned
    /// value that moves into C ([`FfiByValue`]).
    pub(super) ffi_by_value: FfiByValue,
    /// The `.rx` FFI declarations whose result is owned (`-> owned T`),
    /// read before the runtime ABI table (`runtime_abi::result_ownership`).
    pub(super) declared_results: crate::mir::lower::runtime_abi::DeclaredResults,
    /// Functions that only read their one argument and return a new owner
    /// (a derived class `clone`), from the role lowering gave them
    /// ([`Self::record_roles`]). Each is also in `fresh`.
    pub(super) reads_argument: HashSet<String>,
}

/// Per C symbol (and Ruxen spelling) of a `.rx` `lib` declaration, which
/// argument slots take their value BY VALUE
/// (`runtime_abi::ffi_param_takes_value`). Passing an owned heap value there
/// moves it into the C side (`Array#push(x)`, `Hash#insert(k, v)`,
/// `Mutex.new(v)`, `Sender#send(v)`), so the caller must not free it; a
/// parameter the C side only reads is declared `&T`. The one source every
/// drop-side reader asks: drop elaboration, the escape walks, element flow
/// and the loop-local frees.
#[derive(Debug, Clone, Default)]
pub(super) struct FfiByValue(HashMap<String, Vec<bool>>);

impl FfiByValue {
    /// The by-value slots of every declaration in `libs`. Declarations
    /// that share a C symbol must all take the slot by value for it to
    /// count.
    pub(super) fn from_libs(libs: &[crate::mir::nodes::FfiLib]) -> Self {
        let mut table: HashMap<String, Vec<bool>> = HashMap::new();
        for decl in libs.iter().flat_map(|lib| &lib.functions) {
            let mask: Vec<bool> = decl
                .param_types
                .iter()
                .enumerate()
                .map(|(i, ty)| {
                    crate::mir::lower::runtime_abi::ffi_param_takes_value(
                        ty,
                        decl.has_receiver && i == 0,
                    )
                })
                .collect();
            for key in [&decl.name, &decl.ruxen_name] {
                let entry = table.entry(key.clone()).or_insert_with(|| mask.clone());
                for (slot, by_value) in entry.iter_mut().enumerate() {
                    *by_value &= mask.get(slot).copied().unwrap_or(false);
                }
            }
        }
        FfiByValue(table)
    }

    /// Whether the FFI callee `callee` takes argument slot `slot` by value.
    pub(super) fn takes_by_value(&self, callee: &str, slot: usize) -> bool {
        self.0
            .get(callee)
            .and_then(|params| params.get(slot))
            .copied()
            .unwrap_or(false)
    }
}

impl CallSummaries {
    /// Whether `callee` may return its parameter at MIR argument slot `slot`.
    pub(super) fn may_return_param(&self, callee: &str, slot: usize) -> bool {
        self.returned_params
            .get(callee)
            .and_then(|params| params.get(slot))
            .copied()
            .unwrap_or(false)
    }

    /// Record what each synthesized function's [`MirFnRole`] says about its
    /// calls: a derived class `clone` reads its argument and returns a new
    /// owner the caller frees.
    pub(super) fn record_roles(&mut self, functions: &[MirFunction]) {
        for func in functions {
            if func.role == MirFnRole::DerivedClone {
                self.fresh.insert(func.name.clone());
                self.reads_argument.insert(func.name.clone());
            }
        }
    }

    /// Record which parameters `func` may return (those its returned locals
    /// reach through copies, moves and borrows), whether it returns one
    /// parameter on every path, and which parameters take a block.
    pub(super) fn record_returned_params(&mut self, func: &MirFunction) {
        let blocks: Vec<bool> = func
            .params
            .iter()
            .map(|&p| func.locals[p as usize].ty.is_borrowed_block())
            .collect();
        self.block_params.insert(func.name.clone(), blocks);
        let returned: HashSet<LocalId> = func
            .blocks
            .iter()
            .filter_map(|block| match &block.terminator {
                Terminator::Return(Some(MirValue::Use(local))) => Some(*local),
                _ => None,
            })
            .collect();
        let reached = compute_return_alias_chain(func, &returned);
        let params: Vec<bool> = func.params.iter().map(|p| reached.contains(p)).collect();
        if params.contains(&true) {
            self.returned_params.insert(func.name.clone(), params);
        }
        if let Some(slot) = single_returned_param(func) {
            self.passthrough_param.insert(func.name.clone(), slot);
        }
    }
}

/// Whether `func` returns a FRESH value on every path: its return type owns
/// heap, and every returned local is one the dealloc-safety analysis proves
/// this frame owned outright (a fresh allocation, never stored, moved,
/// aliased or returned through a parameter). Such a function hands its
/// caller ownership of the result. A getter returning a field, a function
/// returning a parameter, and a function returning a literal do not.
///
/// A container qualifies only when its elements own no heap: the caller's
/// drop of an `Array[String]` frees every element, and a fresh spine can
/// hold elements borrowed from elsewhere (`Array#select` pushes the
/// receiver's own elements).
pub(super) fn returns_fresh(
    func: &MirFunction,
    symbols: &SymbolTable,
    summaries: &CallSummaries,
) -> bool {
    if !fresh_result_type(&func.return_ty) {
        return false;
    }
    let returns: Vec<&Option<MirValue>> = func
        .blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Return(value) => Some(value),
            _ => None,
        })
        .collect();
    if returns.is_empty() {
        return false;
    }
    let owned = compute_dealloc_safe_locals(func, symbols, summaries);
    returns
        .iter()
        .all(|value| matches!(value, Some(MirValue::Use(local)) if owned.contains(local)))
}

/// Whether a fresh value of type `ty` owns everything its drop frees. A
/// container qualifies only when its elements own no heap: a fresh spine
/// can hold elements borrowed from elsewhere (`Array#select` and
/// `Enumerable#partition` push the receiver's own elements). A tuple owns
/// its elements as an object owns its fields, so it qualifies when each
/// element is a scalar, a borrow (which owns nothing) or itself qualifies.
fn fresh_result_type(ty: &Ty) -> bool {
    match ty {
        Ty::String | Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } => true,
        Ty::Array(elem) | Ty::Set(elem) => is_scalar(elem),
        Ty::Map(key, value) => is_scalar(key) && is_scalar(value),
        Ty::Tuple(elems) => elems.iter().all(|e| {
            is_scalar(e) || matches!(e, Ty::Ref(_) | Ty::RefMut(_)) || fresh_result_type(e)
        }),
        _ => false,
    }
}

/// Remove the statement markers `lower_scoped_statement` emitted; they mean
/// nothing to codegen.
pub(super) fn strip_statement_markers(func: &mut MirFunction) {
    for block in &mut func.blocks {
        if !block
            .instructions
            .iter()
            .any(|inst| matches!(inst, MirInst::Nop))
        {
            continue;
        }
        let spans = std::mem::take(&mut block.inst_spans);
        let instructions = std::mem::take(&mut block.instructions);
        for (index, inst) in instructions.into_iter().enumerate() {
            if matches!(inst, MirInst::Nop) {
                continue;
            }
            block.instructions.push(inst);
            if let Some(span) = spans.get(index) {
                block.inst_spans.push(span.clone());
            }
        }
    }
}

/// The statement end at which each temporary in `candidates` is dropped:
/// `(block, marker ordinal)`. A temporary defined in its innermost
/// statement's final block and used, with every borrow of it (`lifetimes`,
/// [`borrows_of`]), only before that statement's marker is dropped there.
/// One a binding still borrows past its statement (`n = U.new(..).name`)
/// lives on: when every use of it and of its borrows is in that same block,
/// it is dropped at the end of the statement that last uses them, or at the
/// end of the block when no statement marker follows that use (the second
/// map; in a loop body, every iteration). Temporaries not listed keep the
/// function-exit drop.
fn statement_drop_sites(
    func: &MirFunction,
    candidates: &HashSet<LocalId>,
    scopes: &[StmtScope],
    lifetimes: &HashMap<LocalId, HashSet<LocalId>>,
) -> (
    HashMap<LocalId, (BlockId, usize)>,
    HashMap<LocalId, BlockId>,
) {
    let mut sites = HashMap::new();
    let mut block_end = HashMap::new();
    for &local in candidates {
        let Some(scope) = scopes
            .iter()
            .filter(|scope| scope.locals.contains(&local))
            .min_by_key(|scope| scope.locals.end - scope.locals.start)
        else {
            continue;
        };
        let Some(block) = func.blocks.get(scope.block) else {
            continue;
        };
        let Some(marker) = nth_marker(block, scope.nop_ordinal) else {
            continue;
        };
        let defined_here = block.instructions[..marker]
            .iter()
            .any(|inst| instruction_initializes_local(inst, local));
        if !defined_here {
            continue;
        }
        let Some(lifetime) = lifetimes.get(&local) else {
            continue;
        };
        // Every use of the temporary and its borrows must sit in the
        // statement's own block; the last one decides the drop point.
        let elsewhere = func.blocks.iter().enumerate().any(|(block_id, b)| {
            lifetime.iter().any(|&l| {
                terminator_mentions_local(&b.terminator, l)
                    || (block_id != scope.block
                        && b.instructions
                            .iter()
                            .any(|inst| instruction_mentions_local(inst, l)))
            })
        });
        if elsewhere {
            continue;
        }
        let last_use = block
            .instructions
            .iter()
            .rposition(|inst| {
                lifetime
                    .iter()
                    .any(|&l| instruction_mentions_local(inst, l))
            })
            .unwrap_or(0);
        if last_use < marker {
            sites.insert(local, (scope.block, scope.nop_ordinal));
            continue;
        }
        // The first statement marker after the last use.
        let ordinal_after = block.instructions[..=last_use]
            .iter()
            .filter(|inst| matches!(inst, MirInst::Nop))
            .count();
        if nth_marker(block, ordinal_after).is_some() {
            sites.insert(local, (scope.block, ordinal_after));
        } else {
            block_end.insert(local, scope.block);
        }
    }
    (sites, block_end)
}

/// The index of the `ordinal`-th `MirInst::Nop` marker in `block`.
fn nth_marker(block: &BasicBlock, ordinal: usize) -> Option<usize> {
    block
        .instructions
        .iter()
        .enumerate()
        .filter(|(_, inst)| matches!(inst, MirInst::Nop))
        .nth(ordinal)
        .map(|(index, _)| index)
}

/// `local` and every local holding a borrow of it or of storage it owns:
/// all of them must be dead before `local` may be dropped. A borrow is a
/// `Ref`/`RefMut` of it, a copy of a borrow word, a field read
/// (`GetField`: `U.new(..).name` points into the object) or a call result
/// that may point into it, per the [`View`] recorded for the result
/// ([`record_default_views`]); a scalar or an owned result borrows nothing.
fn borrows_of(func: &MirFunction, local: LocalId) -> HashSet<LocalId> {
    let mut set = HashSet::from([local]);
    let view = |dest: LocalId| !matches!(func.view(dest), Some(View::Scalar | View::Own));
    let into = |dest: LocalId, slot: usize| match func.view(dest) {
        Some(View::Args(slots)) => slots.contains(&slot),
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
                MirInst::Call {
                    dest: Some(dest),
                    callee,
                    args,
                } if args.iter().enumerate().any(|(slot, arg)| {
                    matches!(arg, MirValue::Use(a) if set.contains(a)) && into(*dest, slot)
                }) =>
                {
                    set.insert(*dest);
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
/// it: a scalar is `View::Scalar`, and a call result points into the
/// non-scalar arguments [`call_result_may_borrow_arg`] names (a closure
/// call result into all of them). Runs once
/// every callee's body exists, before the loan check and drop elaboration
/// read the views.
pub(super) fn record_default_views(func: &mut MirFunction, summaries: &CallSummaries) {
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
            let slots = (0..args.len()).filter(|&slot| {
                args[slot].local().is_some_and(|arg| !scalar(arg)) && may_borrow(slot)
            });
            View::Args(slots.collect())
        };
        let view = match inst {
            _ if scalar(dest) => View::Scalar,
            MirInst::Call { callee, args, .. } => pointed_into(args, &|slot| {
                call_result_may_borrow_arg(func, dest, callee, slot, summaries)
            }),
            // A closure call whose declared result could not be shown to
            // own itself (`closure_call_facts`) may point into every
            // argument.
            MirInst::CallIndirect { args, .. } => pointed_into(args, &|_| true),
            _ => continue,
        };
        found.push((dest, view));
    }
    for (dest, view) in found {
        func.record_view(dest, view);
    }
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

/// Insert each statement temporary's drop instructions immediately before
/// its statement's marker (later temporaries first).
fn place_statement_drops(
    func: &mut MirFunction,
    sites: &HashMap<LocalId, (BlockId, usize)>,
    drop_instructions: &HashMap<LocalId, Vec<MirInst>>,
) {
    let mut by_block: HashMap<BlockId, HashMap<usize, Vec<LocalId>>> = HashMap::new();
    for (&local, &(block, ordinal)) in sites {
        by_block
            .entry(block)
            .or_default()
            .entry(ordinal)
            .or_default()
            .push(local);
    }
    for (block_id, mut at_marker) in by_block {
        for locals in at_marker.values_mut() {
            locals.sort_unstable_by(|left, right| right.cmp(left));
        }
        let block = &mut func.blocks[block_id];
        let mut ordinal = 0;
        let mut index = 0;
        while index < block.instructions.len() {
            if matches!(block.instructions[index], MirInst::Nop) {
                if let Some(locals) = at_marker.remove(&ordinal) {
                    let span = block.span_at(index);
                    for local in locals {
                        for inst in drop_instructions[&local].iter().cloned() {
                            block.insert_inst(index, inst, span.clone());
                            index += 1;
                        }
                    }
                }
                ordinal += 1;
            }
            index += 1;
        }
    }
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
    summaries: &CallSummaries,
    stmt_scopes: &[StmtScope],
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

    free_moved_payload_shells(func, &param_set);

    // Determine which locals' value provenance is a fresh allocation that
    // this frame owns exclusively. We need this set BEFORE building
    // `drop_locals` because String / Vec / HashMap intermediates that are
    // technically compiler temporaries (`LocalKind::Temp`) can still own heap
    // (e.g. the implicit `ruxen_string_from` wrap inserted around a string
    // literal, or the owned `String` a `.clone` produces). Without
    // dropping such intermediates, every interpolated literal leaks the
    // owned-string copy of itself.
    let dealloc_safe = compute_dealloc_safe_locals(func, symbols, summaries);

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
    //   * the temp's type is a built-in heap-owning type (`String`,
    //     `Array`, `Hash`, `Set`), a class with a `def drop`, or a plain
    //     class or struct, AND
    //   * the dealloc-safety analysis classified the temp as alloc-rooted
    //     (its value chain traces to `MirInst::Alloc`, a fresh-alloc
    //     callee like `ruxen_string_from`, or a fresh-returning user
    //     function, with no aliasing or transfer). An `Assign` of a
    //     temporary into a named local moves that ownership to the local.
    //
    // Enum temps are still skipped (see the filter below). A temporary used
    // only inside its statement is dropped at the statement's end
    // (`statement_drop_sites`); any other is dropped at function exit.
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
            // A Copy tuple is a heap box too: every owner holds its own
            // (`duplicate_boxed_copies`), so each is freed once.
            let is_user_aggregate = matches!(
                local.ty,
                Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } | Ty::Tuple(_)
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
                // A class or struct temporary that still owns its fresh
                // allocation (`Parcel.new(1).value`, `make().value`): an
                // `Assign` into a named local strips the temporary's
                // ownership, so only a value used and abandoned within its
                // statement reaches here. Its free runs the object's
                // field glue first (`heap_free_call`), so the fields it
                // owns die with it. Enum
                // temporaries stay out: a match moves a payload out of one
                // without stripping it.
                if matches!(
                    local.ty,
                    Ty::Class { .. } | Ty::Struct { .. } | Ty::Tuple(_)
                ) {
                    return dealloc_safe.contains(&local.id);
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
            &HashSet::new(),
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
                user_drop_freed_storage = matches!(
                    &func.locals[local_id as usize].ty,
                    Ty::Class { name, .. } if symbols.drop_consumes_self(name)
                );
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

    // A marker-only Drop never reads the local in either backend. Only locals
    // with real closure/user/free glue need path-sensitive placement.
    let active_drop_locals: HashSet<LocalId> = all_drop_locals
        .iter()
        .copied()
        .filter(|local| closure_drop_callees.contains_key(local) || dealloc_safe.contains(local))
        .collect();

    // A statement temporary is dropped at the end of its statement: a lock
    // guard from `m.lock!.get` is released before the next statement, and
    // a temporary in a loop body is freed every iteration.
    let statement_candidates: HashSet<LocalId> = active_drop_locals
        .iter()
        .copied()
        .filter(|local| {
            !closure_drop_callees.contains_key(local)
                && func.locals[*local as usize].kind == LocalKind::Temp
        })
        .collect();
    // What each droppable local must outlive: itself and every borrow of it.
    let lifetimes: HashMap<LocalId, HashSet<LocalId>> = active_drop_locals
        .iter()
        .map(|&local| (local, borrows_of(func, local)))
        .collect();
    let (statement_sites, block_end_sites) =
        statement_drop_sites(func, &statement_candidates, stmt_scopes, &lifetimes);
    let mut statement_sunk: HashSet<LocalId> = statement_sites.keys().copied().collect();
    statement_sunk.extend(block_end_sites.keys().copied());

    // A statement-dropped temporary is freed at its statement's end, so its
    // previous value never survives to a redefinition.
    let rebind_locals = free_prior_value_on_rebind(
        func,
        symbols,
        &dealloc_safe,
        &param_set,
        &moved_to_ffi,
        return_locals,
        &return_alias_chain,
        &statement_sunk,
    );

    let reachable = reachable_blocks(func);
    let branch_candidates: HashSet<LocalId> = active_drop_locals
        .difference(&statement_sunk)
        .copied()
        .collect();
    let mut sink_placements =
        branch_local_drop_blocks(func, &branch_candidates, &reachable, &lifetimes);
    for (&local, &block) in &block_end_sites {
        sink_placements.entry(block).or_default().push(local);
    }
    for locals in sink_placements.values_mut() {
        locals.sort_unstable_by(|left, right| right.cmp(left));
    }
    let mut sunk_locals: HashSet<LocalId> = sink_placements.values().flatten().copied().collect();
    sunk_locals.extend(statement_sunk.iter().copied());
    place_statement_drops(func, &statement_sites, &drop_instructions);

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
    dropped_at_statement_end: &HashSet<LocalId>,
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
                && !dropped_at_statement_end.contains(&local.id)
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
                func.blocks[block_id]
                    .terminator
                    .successors()
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
        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } | Ty::Tuple(_) => "ruxen_dealloc",
        _ => return None,
    })
}

/// The complete drop of `value` of type `ty`: the [`heap_free_callee`] call
/// plus the element descriptors a container's drop glue needs (see the
/// `RUXEN_ELEM_*` block in `library/std/core/runtime/runtime.h`), preceded by
/// the instructions that compute a descriptor naming element glue. An object
/// that owns fields first calls its field drop glue ([`field_drop_glue_name`]);
/// a `def drop` is the caller's to run before all of this. Temps for those
/// instructions are allocated in `func`. `None` exactly when
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
    // An object's owned fields die with it, before its storage is freed.
    // The glue is defined later by `Lowerer::synthesize_field_drop_glue`
    // from the type recorded here (`MirFunction::field_glue_calls`); the
    // argument is a local of exactly `ty`.
    if let Some(glue) = field_drop_glue_name(ty, symbols) {
        func.field_glue_calls.insert(glue.clone(), ty.clone());
        let object = if func.locals[value as usize].ty == *ty {
            value
        } else {
            let typed = func.new_temp(ty.clone());
            insts.push(MirInst::Assign {
                dest: typed,
                value: MirValue::Use(value),
            });
            typed
        };
        insts.push(MirInst::Call {
            dest: None,
            callee: glue,
            args: vec![MirValue::Use(object)],
        });
    }
    let mut args = vec![MirValue::Use(value)];
    for desc in descs {
        record_elem_glue(func, &desc);
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

/// The drop callees that free their first argument. Must list what
/// [`heap_free_callee`] can return.
const FREE_FAMILY_CALLEES: &[&str] = &[
    "ruxen_dealloc",
    "ruxen_string_free",
    "ruxen_vec_drop",
    "ruxen_hash_drop",
    "ruxen_set_drop",
];

/// A machine scalar, a register value that can never carry a pointer: an
/// integer, float, Bool, Char, unit or never.
pub(super) fn is_scalar(ty: &Ty) -> bool {
    ty.is_scalar_primitive() || matches!(ty, Ty::Unit | Ty::Never)
}

/// Heap-owning payload types: a by-value binding of one out of an owned
/// `Option` / `Result` takes ownership and frees it at scope exit.
pub(super) fn is_owned_heap_payload(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Class { .. }
            | Ty::Struct { .. }
            | Ty::String
            | Ty::Array(_)
            | Ty::Map(_, _)
            | Ty::Set(_)
    )
}

/// Locals that hold a borrowed view rather than an owned value: a field
/// read (`GetField` from anything but a match payload pointer), a `Ref` /
/// `RefMut`, and every local a view is assigned, copied or moved into. A
/// match payload pointer is a `GetPayload` out of an `Option` / `Result`
/// that is not itself a view.
fn borrowed_views(func: &MirFunction) -> HashSet<LocalId> {
    let local_ty = |id: LocalId| func.locals.iter().find(|l| l.id == id).map(|l| &l.ty);
    let mut views: HashSet<LocalId> = HashSet::new();
    let mut payload_ptrs: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for inst in &block.instructions {
            match inst {
                MirInst::GetPayload { dest, src, .. } => {
                    if matches!(local_ty(*src), Some(Ty::Result(_, _)) | Some(Ty::Option(_)))
                        && !views.contains(src)
                    {
                        payload_ptrs.insert(*dest);
                    }
                }
                MirInst::GetField { dest, base, .. } => {
                    if !payload_ptrs.contains(base) {
                        views.insert(*dest);
                    }
                }
                MirInst::Ref { dest, .. }
                | MirInst::RefMut { dest, .. }
                | MirInst::FieldAddr { dest, .. } => {
                    views.insert(*dest);
                }
                MirInst::Assign {
                    dest,
                    value: MirValue::Use(src),
                }
                | MirInst::Copy { dest, src }
                | MirInst::Move { dest, src } => {
                    if views.contains(src) {
                        views.insert(*dest);
                    }
                }
                _ => {}
            }
        }
    }
    views
}

/// Free the box of an `Option` / `Result` local whose payload a `match` arm
/// moved into a by-value binding (`MirPayloadMove`): the binding owns and
/// frees the payload, and the borrow checker forbids reading the local
/// again, so nothing else frees the emptied box. Only a box the frame owns
/// is freed: never a parameter's, and never a view of a field or reference.
fn free_moved_payload_shells(func: &mut MirFunction, param_set: &HashSet<LocalId>) {
    if func.payload_moves.is_empty() {
        return;
    }
    let views = borrowed_views(func);
    for payload_move in std::mem::take(&mut func.payload_moves) {
        let local = payload_move.scrutinee;
        if param_set.contains(&local) || views.contains(&local) {
            continue;
        }
        func.blocks[payload_move.block].instructions.insert(
            0,
            MirInst::Call {
                dest: None,
                callee: "ruxen_dealloc".to_string(),
                args: vec![MirValue::Use(local)],
            },
        );
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
///     `GetPayload` (pointer into a parent struct), the result of a call
///     that does not return a fresh value (a runtime callee outside
///     `FRESH_ALLOC_CALLEES`, a user function outside
///     `CallSummaries::fresh`),
///     `Ref`/`RefMut`, or numeric/string literal output.
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
    summaries: &CallSummaries,
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
    // A sum type read out of a field or through a reference is borrowed:
    // the object still owns its payload, so a binding only views it.
    let views = borrowed_views(func);

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
                    if matches!(local_ty(*src), Some(Ty::Result(_, _)) | Some(Ty::Option(_)))
                        && !views.contains(src)
                    {
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
                    let owns_heap = local_ty(*dest).is_some_and(is_owned_heap_payload);
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
                | MirInst::FieldAddr { dest, .. }
                | MirInst::StringLiteral { dest, .. }
                | MirInst::FuncAddr { dest, .. }
                | MirInst::FloatToBits { dest, .. }
                | MirInst::BitsToFloat { dest, .. }
                | MirInst::Convert { dest, .. }
                | MirInst::DataAddr { dest, .. } => {
                    // Phase B-5: a static data pointer (vtable /
                    // class_info) is not a heap allocation — taint
                    // the dest so dealloc-tracking treats it as a
                    // non-owning pointer. `FloatToBits` likewise produces
                    // a scalar integer, never a heap allocation, and a
                    // `Convert` (inserted after this pass) owns nothing.
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
                        callee_ownership, result_ownership, user_callee_param_is_ref,
                        ResultOwnership,
                    };
                    let abi = callee_ownership(callee.as_str());
                    // A derived `clone` and a closure share only read their
                    // receiver (their result is in `summaries.fresh`). Field
                    // drop glue only reads its object, ahead of the object's
                    // own free (a rebind's free, see `heap_free_call`).
                    let borrows_receiver = abi.borrows_first_arg
                        || summaries.reads_argument.contains(callee.as_str())
                        || is_field_glue_call(func, callee);

                    // A user function that returns a fresh value hands the
                    // caller ownership of it, as a fresh-alloc runtime
                    // callee does (`returns_fresh`).
                    let result_is_fresh =
                        result_ownership(callee.as_str(), &summaries.declared_results)
                            == ResultOwnership::Fresh
                            || summaries.fresh.contains(callee.as_str());
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
                        user_callee_param_is_ref(
                            symbols,
                            callee.as_str(),
                            args.len(),
                            summaries
                                .block_params
                                .get(callee.as_str())
                                .map(Vec::as_slice),
                        )
                    } else {
                        Vec::new()
                    };
                    // A callee that returns one of its parameters on every
                    // path hands the argument back as the result. A named
                    // owner lent to a borrowing slot keeps it, and the result
                    // is a view of it (`r.with_body(b)` discarding the
                    // returned `self`). Otherwise (a temporary, or a value
                    // passed by value) the argument's ownership moves to the
                    // result, after the argument loop: `[1, 2].tap { }`.
                    let passthrough_slot =
                        dest.and(summaries.passthrough_param.get(callee.as_str()).and_then(
                            |&slot| match args.get(slot) {
                                Some(MirValue::Use(arg)) => Some((slot, *arg)),
                                _ => None,
                            },
                        ));
                    let passthrough_named = passthrough_slot.and_then(|(slot, arg)| {
                        let lent = abi.args_are_borrowed
                            || (slot == 0 && borrows_receiver)
                            || param_is_ref.get(slot).copied().unwrap_or(false);
                        (lent && func.locals[arg as usize].kind != LocalKind::Temp).then_some(arg)
                    });
                    let passthrough = passthrough_slot
                        .map(|(_, arg)| arg)
                        .filter(|_| passthrough_named.is_none());
                    let passthrough_owned = passthrough.is_some_and(|arg| {
                        alloc_rooted.contains(&arg) && !tainted_perm.contains(&arg)
                    });
                    if let Some(d) = dest {
                        if result_is_fresh && !tainted_perm.contains(d) {
                            alloc_rooted.insert(*d);
                        } else if passthrough_owned {
                            // Rooted after the argument loop, below.
                        } else {
                            tainted_perm.insert(*d);
                            alloc_rooted.remove(d);
                        }
                    }
                    // Reproduces the old arg loop precedence exactly:
                    // arg0-borrow continue, then explicit transfer (folds in
                    // the old pointer-store idx==1 case via the ArgMask), then
                    // borrow continue, then the user-`&T`-param borrow, then
                    // default taint.
                    for (idx, arg) in args.iter().enumerate() {
                        if let MirValue::Use(l) = arg {
                            if borrows_receiver && idx == 0 {
                                continue;
                            }
                            if passthrough_named == Some(*l) {
                                continue;
                            }
                            // A value passed BY VALUE to a `.rx` FFI
                            // declaration moves into the C side.
                            if summaries.ffi_by_value.takes_by_value(callee, idx) {
                                tainted_perm.insert(*l);
                                alloc_rooted.remove(l);
                                continue;
                            }
                            // A borrowed argument the callee may return is
                            // aliased by the result: not a mere borrow.
                            if abi.args_are_borrowed && !summaries.may_return_param(callee, idx) {
                                continue;
                            }
                            // User callee with a declared `&T` param at this
                            // slot: auto-borrow, do not taint — unless the
                            // callee may return that parameter, whose result
                            // then aliases the argument.
                            if param_is_ref.get(idx).copied().unwrap_or(false)
                                && !summaries.may_return_param(callee, idx)
                            {
                                continue;
                            }
                            // A boxed Copy value passed by value stays the
                            // caller's: the callee keeps or returns only its
                            // own copy (`duplicate_boxed_copies`).
                            if copies::is_boxed_copy(&func.locals[*l as usize].ty, symbols)
                                && !summaries.may_return_param(callee, idx)
                            {
                                continue;
                            }
                            tainted_perm.insert(*l);
                            alloc_rooted.remove(l);
                        }
                    }
                    // The passthrough result is the argument itself: the
                    // argument stops owning it and the result owns it.
                    if let (Some(arg), Some(d)) = (passthrough, dest) {
                        tainted_perm.insert(arg);
                        alloc_rooted.remove(&arg);
                        if passthrough_owned {
                            alloc_rooted.insert(*d);
                        }
                    }
                }
                MirInst::CallIndirect { dest, args, .. } => {
                    // A closure body returns a boxed Copy value as a box of
                    // its own and keeps only copies of its arguments
                    // (`duplicate_boxed_copies`), so such a result is the
                    // caller's and such an argument stays the caller's.
                    let boxed =
                        |l: LocalId| copies::is_boxed_copy(&func.locals[l as usize].ty, symbols);
                    if let Some(d) = dest {
                        if boxed(*d) && !tainted_perm.contains(d) {
                            alloc_rooted.insert(*d);
                        } else {
                            tainted_perm.insert(*d);
                            alloc_rooted.remove(d);
                        }
                    }
                    for arg in args {
                        if let MirValue::Use(l) = arg {
                            if boxed(*l) {
                                continue;
                            }
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
                    // A returned borrow (`&x`, `&var x`) keeps `x` alive
                    // for the caller just as a returned copy of it does.
                    MirInst::Copy { dest, src }
                    | MirInst::Move { dest, src }
                    | MirInst::Ref { dest, src }
                    | MirInst::RefMut { dest, src }
                    | MirInst::FieldAddr {
                        dest, base: src, ..
                    } => {
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
