//! Closure and element flow: the whole-program pass that makes a container's
//! drop shallow when one of its element pointers escaped, and the closure
//! target analysis it reads.

use super::*;
use std::collections::BTreeSet;

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
/// in `mir`: a fixpoint computes, per function and per closure context (what
/// its parameters may hold, [`closure_targets`]), which parameters can reach
/// an escaping sink and which can reach the return value. A call through a
/// closure pair reads the summaries of the closure bodies the pair may hold,
/// so lending an element to a block that only reads it is no escape. Unknown
/// callees fall back to `runtime_abi::callee_ownership`, whose default treats
/// every argument as escaping.
pub(in crate::mir::lower) fn demote_escaping_element_drops(
    mir: &mut MirProgram,
    symbols: &SymbolTable,
) {
    let ffi = FfiByValue::from_libs(&mir.ffi_libs);
    let declared = crate::mir::lower::runtime_abi::DeclaredResults::from_libs(&mir.ffi_libs);
    let index: HashMap<String, usize> = mir
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();
    // Summaries are per (function, closure context): what each parameter
    // may hold (`closure_targets`). A shared stdlib body such as
    // `Hash#each` lets an element escape only when the block it is handed
    // stores its argument, so each call site reads the summary for the
    // closures it actually passes. Every function also gets the summary for
    // the union of its callers' contexts, which decides its own drops.
    let (defaults, captures) = closure_targets(mir, &index);
    let mut summaries: HashMap<FlowKey, ParamFlow> = defaults
        .iter()
        .enumerate()
        .map(|(i, held)| ((i, held.clone()), ParamFlow::none(&mir.functions[i])))
        .collect();
    loop {
        let requests = std::cell::RefCell::new(Vec::new());
        let ctx = FlowCtx {
            index: &index,
            functions: &mir.functions,
            summaries: &summaries,
            requests: &requests,
            captures: &captures,
            symbols,
            ffi: &ffi,
            declared: &declared,
        };
        let mut next: HashMap<FlowKey, ParamFlow> = summaries
            .keys()
            .map(|key| {
                let func = &mir.functions[key.0];
                (
                    key.clone(),
                    ElementFlow::analyze(func, &key.1, &ctx).param_flow(func),
                )
            })
            .collect();
        for key in requests.into_inner() {
            let none = ParamFlow::none(&mir.functions[key.0]);
            next.entry(key).or_insert(none);
        }
        if next == summaries {
            break;
        }
        summaries = next;
    }

    let requests = std::cell::RefCell::new(Vec::new());
    let ctx = FlowCtx {
        index: &index,
        functions: &mir.functions,
        summaries: &summaries,
        requests: &requests,
        captures: &captures,
        symbols,
        ffi: &ffi,
        declared: &declared,
    };
    let flows: Vec<ElementFlow> = mir
        .functions
        .iter()
        .zip(&defaults)
        .map(|(func, held)| ElementFlow::analyze(func, held, &ctx))
        .collect();

    // A container held in an object's field is read through `GetField`; when
    // an element of that read escapes (`@items = @items.select { … }` keeps
    // aliases of the old elements), every drop of that field (the field drop
    // glue, the free on overwrite) must be shallow too. Slots are keyed by
    // class family so a read through a parent-typed receiver covers the
    // subclass's glue as well.
    let mut aliased_slots: HashSet<(DefId, usize)> = HashSet::new();
    for (func, flow) in mir.functions.iter().zip(&flows) {
        for inst in func.blocks.iter().flat_map(|b| b.instructions.iter()) {
            if let MirInst::GetField {
                dest,
                base,
                field_index,
            } = inst
            {
                if flow.elements_escaped(*dest) {
                    if let Some(family) = class_family(&func.locals[*base as usize].ty, symbols) {
                        aliased_slots.insert((family, *field_index));
                    }
                }
            }
        }
    }

    for (i, flow) in flows.iter().enumerate() {
        // A closure's drop function frees its captures; the body that may
        // have leaked their elements is the closure function itself.
        let closure_body_escapes = mir.functions[i]
            .name
            .strip_suffix("_drop")
            .and_then(|body| index.get(body))
            .and_then(|&body| summaries.get(&(body, defaults[body].clone())))
            .is_some_and(|summary| summary.escapes.iter().any(|e| *e));
        let field_reads: HashMap<LocalId, (DefId, usize)> = mir.functions[i]
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter_map(|inst| match inst {
                MirInst::GetField {
                    dest,
                    base,
                    field_index,
                } => class_family(&mir.functions[i].locals[*base as usize].ty, symbols)
                    .map(|family| (*dest, (family, *field_index))),
                _ => None,
            })
            .collect();
        let copied_element_containers: HashSet<LocalId> = mir.functions[i]
            .locals
            .iter()
            .filter(|local| match &local.ty {
                Ty::Array(elem) | Ty::Set(elem) | Ty::Map(_, elem) => {
                    super::copies::is_boxed_copy(elem, symbols)
                }
                _ => false,
            })
            .map(|local| local.id)
            .collect();
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
                let aliased_field = field_reads
                    .get(container)
                    .is_some_and(|slot| aliased_slots.contains(slot));
                // Elements that are boxed Copy values never escape as aliases:
                // every owner they reach takes its own box.
                let copied_elements = copied_element_containers.contains(container);
                if !copied_elements
                    && (closure_body_escapes || flow.elements_escaped(*container) || aliased_field)
                {
                    for desc in args.iter_mut().skip(1) {
                        *desc = MirValue::Literal(Literal::Int(ELEM_POD));
                    }
                }
            }
        }
    }
}

/// The oldest ancestor of the class `ty` names (after peeling references),
/// which identifies its whole inheritance family; `None` for a non-class.
pub(super) fn class_family(ty: &Ty, symbols: &SymbolTable) -> Option<DefId> {
    use crate::resolve::symbols::DefKind;
    let Ty::Class { name, .. } = ty.peel_refs() else {
        return None;
    };
    let mut family = symbols.type_def(name)?.id;
    let mut seen = HashSet::new();
    while seen.insert(family) {
        match symbols.get(family).map(|def| &def.kind) {
            Some(DefKind::Class { info }) => match info.parent {
                Some(parent) => family = parent,
                None => break,
            },
            _ => break,
        }
    }
    Some(family)
}

/// What a function does with what its parameters point at, for its callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ParamFlow {
    /// Something reachable from the parameter may be stored past the call.
    escapes: Vec<bool>,
    /// The return value may point into the parameter.
    returned: Vec<bool>,
}

impl ParamFlow {
    /// The starting point of the fixpoint: nothing escapes or is returned.
    fn none(func: &MirFunction) -> Self {
        ParamFlow {
            escapes: vec![false; func.params.len()],
            returned: vec![false; func.params.len()],
        }
    }
}

/// The whole-program facts [`ElementFlow`] reads while analysing one
/// function.
pub(super) struct FlowCtx<'m> {
    /// Function name to its index in the program.
    index: &'m HashMap<String, usize>,
    functions: &'m [MirFunction],
    /// The summaries known so far, per (function, closure context).
    summaries: &'m HashMap<FlowKey, ParamFlow>,
    /// Contexts asked for that have no summary yet; the driver adds them.
    requests: &'m std::cell::RefCell<Vec<FlowKey>>,
    /// Closure body name to what each of its capture slots may hold.
    captures: &'m CaptureTargets,
    symbols: &'m SymbolTable,
    /// The by-value FFI slots: an owned value passed there moves into C.
    ffi: &'m FfiByValue,
    /// The FFI declarations whose result is owned (`-> owned T`).
    declared: &'m crate::mir::lower::runtime_abi::DeclaredResults,
}

/// A function's index and what each of its parameters may hold.
pub(super) type FlowKey = (usize, Vec<ClosureTargets>);

/// Closure body name to what each of its capture slots may hold.
pub(super) type CaptureTargets = HashMap<String, Vec<ClosureTargets>>;

impl FlowCtx<'_> {
    /// The summary of function `callee` when its parameters hold `held`:
    /// the one computed so far, or "nothing escapes" until it is (the
    /// request makes the driver compute it and iterate).
    fn summary(&self, callee: usize, held: Vec<ClosureTargets>) -> ParamFlow {
        let key = (callee, held);
        match self.summaries.get(&key) {
            Some(summary) => summary.clone(),
            None => {
                self.requests.borrow_mut().push(key);
                ParamFlow::none(&self.functions[callee])
            }
        }
    }

    /// Whether argument `i` of a call through a pair holding `bodies`, with
    /// arguments holding `args`, may be stored past the call. A closure body
    /// never frees its parameters, so the call only lends the argument
    /// unless one of those bodies lets that parameter escape (or the pair's
    /// origin is unknown).
    fn closure_arg_escapes(
        &self,
        bodies: &ClosureTargets,
        args: &[ClosureTargets],
        i: usize,
    ) -> bool {
        match bodies {
            ClosureTargets::Any => true,
            ClosureTargets::Known(bodies) => bodies.iter().any(|body| {
                self.index.get(body).is_none_or(|&b| {
                    let held = held_by_params(&self.functions[b], args);
                    self.summary(b, held)
                        .escapes
                        .get(i)
                        .copied()
                        .unwrap_or(true)
                })
            }),
        }
    }
}

/// What each parameter of `callee` holds when it is called with arguments
/// holding `args` (missing arguments hold nothing).
pub(super) fn held_by_params(callee: &MirFunction, args: &[ClosureTargets]) -> Vec<ClosureTargets> {
    (0..callee.params.len())
        .map(|i| args.get(i).cloned().unwrap_or_else(ClosureTargets::none))
        .collect()
}

/// Which closure bodies (`__closure_<n>`) a local may hold.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum ClosureTargets {
    /// Exactly these bodies; empty when no closure reaches the local.
    Known(BTreeSet<String>),
    /// Any closure: a value read out of a field, a collection or a call
    /// result, or a parameter of a function reached from outside the
    /// program's direct calls.
    Any,
}

impl ClosureTargets {
    fn none() -> Self {
        ClosureTargets::Known(BTreeSet::new())
    }

    /// Widen `self` to include `other`; true when it changed.
    fn join(&mut self, other: &ClosureTargets) -> bool {
        match (&mut *self, other) {
            (ClosureTargets::Any, _) => false,
            (_, ClosureTargets::Any) => {
                *self = ClosureTargets::Any;
                true
            }
            (ClosureTargets::Known(mine), ClosureTargets::Known(theirs)) => {
                let before = mine.len();
                mine.extend(theirs.iter().cloned());
                mine.len() != before
            }
        }
    }
}

/// Per function, per local: the closure bodies the local may hold. A pair
/// built by a literal holds that literal's body; copies and borrows keep
/// it; a parameter holds what every direct caller passes; a closure body's
/// capture slot holds what every pair built from that literal stored there;
/// every other value may hold any closure. A function whose address is
/// taken, that sits in a vtable, or that is exported or the entry point has
/// callers this pass does not see, so its parameters may hold any closure.
/// Returns what each function's parameters may hold over all its callers,
/// and what each closure body's capture slots may hold.
pub(super) fn closure_targets(
    mir: &MirProgram,
    index: &HashMap<String, usize>,
) -> (Vec<Vec<ClosureTargets>>, CaptureTargets) {
    let mut external: HashSet<&str> = mir
        .functions
        .iter()
        .filter(|f| f.linkage == MirLinkage::Export)
        .map(|f| f.name.as_str())
        .chain(mir.entry.as_deref())
        .chain(mir.wasm_exports.iter().map(String::as_str))
        .chain(
            mir.vtables
                .iter()
                .flat_map(|v| v.method_symbols.iter().map(String::as_str)),
        )
        .collect();
    for inst in mir
        .functions
        .iter()
        .flat_map(|f| f.blocks.iter().flat_map(|b| b.instructions.iter()))
    {
        if let MirInst::FuncAddr { func_name, .. } = inst {
            external.insert(func_name);
        }
    }
    let mut params: Vec<Vec<ClosureTargets>> = mir
        .functions
        .iter()
        .map(|f| {
            let seed = if external.contains(f.name.as_str()) {
                ClosureTargets::Any
            } else {
                ClosureTargets::none()
            };
            vec![seed; f.params.len()]
        })
        .collect();
    // Closure body name to what each of its capture slots may hold.
    let mut captures: CaptureTargets = HashMap::new();
    loop {
        let locals: Vec<Vec<ClosureTargets>> = mir
            .functions
            .iter()
            .zip(&params)
            .map(|(f, p)| local_closure_targets(f, p, captures.get(&f.name), index, &mir.functions))
            .collect();
        let mut changed = false;
        for (func, targets) in mir.functions.iter().zip(&locals) {
            for inst in func.blocks.iter().flat_map(|b| b.instructions.iter()) {
                let MirInst::Call { callee, args, .. } = inst else {
                    continue;
                };
                let Some(&callee) = index.get(callee.as_str()) else {
                    continue;
                };
                for (slot, arg) in params[callee].iter_mut().zip(args) {
                    if let MirValue::Use(arg) = arg {
                        changed |= slot.join(&targets[*arg as usize]);
                    }
                }
            }
            for (body, slots) in pair_capture_stores(func, targets, index, &mir.functions) {
                let known = captures.entry(body).or_default();
                for (slot, held) in slots {
                    if known.len() <= slot {
                        known.resize(slot + 1, ClosureTargets::none());
                        changed = true;
                    }
                    changed |= known[slot].join(&held);
                }
            }
        }
        if !changed {
            return (params, captures);
        }
    }
}

/// For every closure pair `func` builds: the literal's body and what each
/// capture slot of its captures box receives (per `targets`).
pub(super) fn pair_capture_stores(
    func: &MirFunction,
    targets: &[ClosureTargets],
    index: &HashMap<String, usize>,
    functions: &[MirFunction],
) -> Vec<(String, Vec<(usize, ClosureTargets)>)> {
    let insts = || func.blocks.iter().flat_map(|b| b.instructions.iter());
    let bodies: HashMap<LocalId, &str> = closure_body_addrs(func, index, functions);
    let mut box_slots: HashMap<LocalId, Vec<(usize, ClosureTargets)>> = HashMap::new();
    let mut pair_body: HashMap<LocalId, &str> = HashMap::new();
    let mut pair_box: HashMap<LocalId, LocalId> = HashMap::new();
    for inst in insts() {
        let MirInst::SetField {
            base,
            field_index,
            value: MirValue::Use(value),
        } = inst
        else {
            continue;
        };
        match (*field_index, bodies.get(value)) {
            (CLOSURE_PAIR_FN, Some(body)) => {
                pair_body.insert(*base, body);
            }
            _ => {
                if *field_index == CLOSURE_PAIR_CAPTURES {
                    pair_box.insert(*base, *value);
                }
                box_slots
                    .entry(*base)
                    .or_default()
                    .push((*field_index, targets[*value as usize].clone()));
            }
        }
    }
    pair_body
        .into_iter()
        .map(|(pair, body)| {
            let slots = pair_box
                .get(&pair)
                .and_then(|b| box_slots.get(b))
                .cloned()
                .unwrap_or_default();
            (body.to_string(), slots)
        })
        .collect()
}

/// Locals holding a closure body's address (a `FuncAddr` of a function
/// whose role is [`MirFnRole::ClosureBody`]).
pub(super) fn closure_body_addrs<'f>(
    func: &'f MirFunction,
    index: &HashMap<String, usize>,
    functions: &[MirFunction],
) -> HashMap<LocalId, &'f str> {
    let is_body = |name: &str| {
        index
            .get(name)
            .is_some_and(|&i| functions[i].role == MirFnRole::ClosureBody)
    };
    func.blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .filter_map(|inst| match inst {
            MirInst::FuncAddr { dest, func_name } if is_body(func_name) => {
                Some((*dest, func_name.as_str()))
            }
            _ => None,
        })
        .collect()
}

/// [`closure_targets`] within one function, given what its parameters hold
/// and, for a closure body, what its capture slots hold (`None` when no
/// pair built from it was seen).
pub(super) fn local_closure_targets(
    func: &MirFunction,
    params: &[ClosureTargets],
    capture_slots: Option<&Vec<ClosureTargets>>,
    index: &HashMap<String, usize>,
    functions: &[MirFunction],
) -> Vec<ClosureTargets> {
    let mut targets = vec![ClosureTargets::none(); func.locals.len()];
    for (&param, held) in func.params.iter().zip(params) {
        targets[param as usize] = held.clone();
    }
    let is_body = func.role == MirFnRole::ClosureBody;
    let captures_param = func.params.first().copied().filter(|_| is_body);
    let bodies = closure_body_addrs(func, index, functions);
    let mut changed = true;
    while changed {
        changed = false;
        for inst in func.blocks.iter().flat_map(|b| b.instructions.iter()) {
            match inst {
                MirInst::SetField {
                    base,
                    field_index: CLOSURE_PAIR_FN,
                    value: MirValue::Use(addr),
                } => {
                    if let Some(body) = bodies.get(addr) {
                        let held = ClosureTargets::Known(BTreeSet::from([body.to_string()]));
                        changed |= targets[*base as usize].join(&held);
                    }
                }
                MirInst::GetField {
                    dest,
                    base,
                    field_index,
                } if Some(*base) == captures_param => {
                    let held = capture_slots
                        .map(|slots| {
                            slots
                                .get(*field_index)
                                .cloned()
                                .unwrap_or_else(ClosureTargets::none)
                        })
                        .unwrap_or(ClosureTargets::Any);
                    changed |= targets[*dest as usize].join(&held);
                }
                MirInst::Assign {
                    dest,
                    value: MirValue::Use(src),
                }
                | MirInst::Copy { dest, src }
                | MirInst::Move { dest, src }
                | MirInst::Ref { dest, src }
                | MirInst::RefMut { dest, src } => {
                    let held = targets[*src as usize].clone();
                    changed |= targets[*dest as usize].join(&held);
                }
                MirInst::Alloc { .. }
                | MirInst::StackAlloc { .. }
                | MirInst::Assign { .. }
                | MirInst::FuncAddr { .. } => {}
                other => {
                    if let Some(dest) = other.dest() {
                        changed |= targets[dest as usize].join(&ClosureTargets::Any);
                    }
                }
            }
        }
    }
    targets
}

/// Points-to facts for one function. A ROOT is a container-typed local or a
/// non-scalar parameter. `same[l]` is the roots `l` may BE (an alias or a
/// reference of the root); `elem[l]` the roots `l` may point INTO.
pub(super) struct ElementFlow {
    same: Vec<BTreeSet<LocalId>>,
    elem: Vec<BTreeSet<LocalId>>,
    escaped: BTreeSet<LocalId>,
    returned: BTreeSet<LocalId>,
    /// The callees of closure calls (`MirInst::CallIndirect::pair` names a
    /// closure-typed pair), mapped to that pair.
    closure_callees: HashMap<LocalId, LocalId>,
    /// Per local: the closure bodies it may hold in this context.
    targets: Vec<ClosureTargets>,
    /// Locals holding the value read through a slot-addressed `&var T`
    /// parameter (`GetField` slot 0 of a `&var String` param). Freeing one
    /// is the first half of `s = v` replacing the caller's value in place
    /// (the store back follows; E0744 forbids moving it anywhere else), not
    /// an escape of the slot's element.
    slot_loads: HashSet<LocalId>,
}

impl ElementFlow {
    /// The facts for `func` when its parameters hold `held`.
    fn analyze(func: &MirFunction, held: &[ClosureTargets], ctx: &FlowCtx) -> Self {
        let n = func.locals.len();
        let closure_callees = func
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter_map(|inst| match inst {
                MirInst::CallIndirect {
                    callee,
                    pair: Some(pair),
                    ..
                } if func.locals[*pair as usize].ty.is_callable() => Some((*callee, *pair)),
                _ => None,
            })
            .collect();
        let slot_loads = func
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter_map(|inst| match inst {
                MirInst::GetField {
                    dest,
                    base,
                    field_index: 0,
                } if func.params.contains(base)
                    && crate::mir::refmut_abi::is_slot_refmut(&func.locals[*base as usize].ty) =>
                {
                    Some(*dest)
                }
                _ => None,
            })
            .collect();
        let mut flow = ElementFlow {
            slot_loads,
            same: vec![BTreeSet::new(); n],
            elem: vec![BTreeSet::new(); n],
            escaped: BTreeSet::new(),
            returned: BTreeSet::new(),
            closure_callees,
            targets: local_closure_targets(
                func,
                held,
                ctx.captures.get(&func.name),
                ctx.index,
                ctx.functions,
            ),
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
                    flow.transfer(func, inst, ctx);
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

    fn transfer(&mut self, func: &MirFunction, inst: &MirInst, ctx: &FlowCtx) {
        use crate::mir::lower::runtime_abi::{callee_ownership, user_callee_param_is_ref};
        let (index, symbols) = (ctx.index, ctx.symbols);
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
            // A call through a closure pair (a block, a lambda, a Proc)
            // only lends an argument unless some closure body that could be
            // the target stores that parameter. Argument 0 is the pair's own
            // captures box, which the pair owns whoever holds the pair (a
            // capture a body moves out is given up by the closure's drop,
            // `capture_slot_still_owned`), so calling a closure read out of
            // a collection does not leak that collection's elements. Any
            // other indirect call (a vtable slot) may keep every argument.
            MirInst::CallIndirect {
                dest, callee, args, ..
            } => {
                let pair = self.closure_callees.get(callee).copied();
                let held = self.held_by_args(args);
                let mut reach = BTreeSet::new();
                for (i, arg) in args.iter().enumerate() {
                    let MirValue::Use(arg) = arg else {
                        continue;
                    };
                    let arg_reach = self.reach(*arg);
                    let escapes = match pair {
                        None => true,
                        Some(_) if i == 0 => false,
                        Some(pair) => {
                            ctx.closure_arg_escapes(&self.targets[pair as usize], &held, i)
                        }
                    };
                    if escapes {
                        self.escaped.extend(arg_reach.iter().copied());
                    }
                    reach.extend(arg_reach);
                }
                if let Some(dest) = dest.filter(|d| non_scalar(*d)) {
                    self.flow_into(dest, reach);
                }
            }
            MirInst::Call { dest, callee, args } => {
                let summary = index.get(callee.as_str()).map(|&i| {
                    ctx.summary(
                        i,
                        held_by_params(&ctx.functions[i], &self.held_by_args(args)),
                    )
                });
                let frees =
                    FREE_FAMILY_CALLEES.contains(&callee.as_str()) || callee.ends_with("_drop");
                let abi = callee_ownership(callee);
                let param_is_ref = if summary.is_none() && !frees && !abi.args_are_borrowed {
                    user_callee_param_is_ref(symbols, callee, args.len(), None)
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
                        // Freeing an element alias frees the slot's object,
                        // unless it is the value a `&var` slot parameter
                        // replaces in place.
                        if !self.slot_loads.contains(arg) {
                            self.escaped.extend(self.elem[*arg as usize].clone());
                        }
                        false
                    } else if let Some(summary) = &summary {
                        if summary.returned.get(i).copied().unwrap_or(true) {
                            result_reach.extend(reach.iter().copied());
                        }
                        summary.escapes.get(i).copied().unwrap_or(true)
                    } else {
                        result_reach.extend(reach.iter().copied());
                        !(abi.borrows_first_arg && i == 0)
                            && (ctx.ffi.takes_by_value(callee, i)
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
                        if summary.is_none() && ctx.ffi.takes_by_value(callee, i) && aliased {
                            if let Some(MirValue::Use(target)) = args.first() {
                                self.escaped.extend(self.reach(*target));
                            }
                        }
                        self.escaped.extend(reach);
                    }
                }
                if let Some(dest) = dest.filter(|d| non_scalar(*d)) {
                    let fresh_owned = summary.is_none()
                        && crate::mir::lower::runtime_abi::result_ownership(callee, ctx.declared)
                            == crate::mir::lower::runtime_abi::ResultOwnership::Fresh
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

    /// What each argument of a call holds.
    fn held_by_args(&self, args: &[MirValue]) -> Vec<ClosureTargets> {
        args.iter()
            .map(|arg| match arg {
                MirValue::Use(local) => self.targets[*local as usize].clone(),
                _ => ClosureTargets::none(),
            })
            .collect()
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
