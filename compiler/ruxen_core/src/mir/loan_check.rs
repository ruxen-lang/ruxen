//! Loan checking on MIR: a value that points into a place (a loan) must not
//! outlive a write, a move or a mutation of that place while the value is
//! still used (E1002, E1009). It runs on every body before drop
//! elaboration, so it sees operands in the order the program evaluates them.
//!
//! The checker reads what lowering recorded and reconstructs nothing from
//! types or instruction shapes: the [`View`] of each value
//! (`MirFunction::views`), each callee's argument modes (`ParamMode`, on
//! its declaration or on the `CallIndirect`), and each closure pair a body
//! builds (`View::Closure`). Where nothing is recorded it assumes the worst:
//! a value may point into every place its instruction reads, a call result
//! into every non-scalar argument, and an argument of an unknown callee may
//! be written through (`param_modes::unknown_param_mode`).
//!
//! - A place is a local and a path of field slots; `INSIDE` stands for
//!   "somewhere inside the value" (a call result pointing into an argument,
//!   an enum payload) and `DEREF` for what a parameter the caller lends
//!   points at. A local denotes a set of places: one for a view, its own
//!   storage for a fresh value, several after a join of paths that bound it
//!   differently or for a call result that may point into several
//!   arguments. An access through it is checked at every one of them.
//! - A loan starts where an instruction makes a view of a place: a field
//!   or payload read, `Ref`, `RefMut`, `FieldAddr`, or a call result
//!   pointing into an argument that owns its storage. A value made from a
//!   value that holds loans holds them too, and an object or container that
//!   stores such a value (`SetField`, or a call taking the value by value
//!   next to a by-reference argument) holds them from then on.
//! - A loan is live while some local holding it is live (`dataflow::Liveness`),
//!   so it ends at its last use (NLL). The arguments of a call are in use at
//!   the call, so a mutation through one conflicts with a loan another holds.
//! - A write of place P, or a move of the local owning it, conflicts with a
//!   live loan at or inside P; a
//!   mutation through a `ParamMode::Mut` argument conflicts with a live loan
//!   strictly inside P (a callee cannot replace a handle it was lent), or at P
//!   too when the argument is a slot address (`refmut_abi::is_slot_refmut`).
//!   Passing a `View::Assignee` address is a write. Only a loan another live
//!   local holds conflicts: a value made through the accessed local shares
//!   its loans and keeps them live.
//! - A closure body's accesses and loans below its captures box (parameter
//!   0) are its effects. A closure pair passed to a call, or called, does
//!   them to the places its captures box slots were filled from: it takes
//!   those loans, then each access is checked with the call's other
//!   operands in use. Effects are summarised before the bodies are checked,
//!   rerunning the closure bodies until they stop growing (a pair passed
//!   inside a body is an access below that body's captures), at most
//!   `SUMMARY_ROUNDS` times.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::borrow_check::errors::{BorrowError, ErrorCode, SpanLabel};
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::mir::dataflow::{Liveness, LocalSet};
use crate::mir::lang_intrinsics::runtime_name;
use crate::mir::nodes::{
    BlockId, LocalId, LocalKind, MirFnRole, MirFunction, MirInst, MirProgram, MirValue, ParamMode,
    View,
};
use crate::mir::param_modes::unknown_param_mode;
use crate::mir::residual_abi::RESIDUAL_RUNTIME_FNS;
use crate::resolve::symbols::SymbolTable;

/// A path element meaning "somewhere inside the value".
const INSIDE: usize = usize::MAX;

/// A path element meaning "what this reference points at": the place a
/// parameter taken by reference denotes, which the body does not own.
const DEREF: usize = usize::MAX - 1;

/// How many times the closure bodies are rerun to summarise their effects:
/// enough for closures nested that deep, and a bound by construction.
const SUMMARY_ROUNDS: usize = 8;

/// How many field slots a place's path holds before its tail becomes
/// `INSIDE` (`Place::child`). A body names finitely many locals and field
/// slots, so with paths this long at most there are finitely many places:
/// the alias sets, which only grow at a join, reach a fixpoint even for a
/// view rebuilt from itself in a loop (`p = p.next`).
const MAX_PLACE_DEPTH: usize = 8;

/// Check every body of `mir` lowered from the program's own source (not the
/// prelude: `MirFunction::from_prelude`), before drop elaboration. Each error is
/// reported once per code and span, however many monomorphised copies of a
/// body share it.
///
/// `elided` holds, by function name, the instructions a later pass deletes
/// (`drops::elided_rebind_frees`): they never run, so they are skipped.
pub fn check_program(
    mir: &MirProgram,
    symbols: &SymbolTable,
    elided: &HashMap<&str, HashSet<(BlockId, usize)>>,
) -> Vec<BorrowError> {
    let modes = CalleeModes::new(mir);
    let none = HashSet::new();
    let skipped = |func: &MirFunction| elided.get(func.name.as_str()).unwrap_or(&none);
    let checked = |func: &MirFunction| {
        !func.from_prelude && matches!(func.role, MirFnRole::Plain | MirFnRole::ClosureBody)
    };
    let closures: Vec<&MirFunction> = mir
        .functions
        .iter()
        .filter(|func| checked(func) && func.role == MirFnRole::ClosureBody)
        .collect();
    // Each round checks every closure body with the effects the last round
    // found; effects only grow, so a round that finds the same ones is the
    // fixpoint, and its errors are the closure bodies' final ones.
    let mut effects = Effects::new();
    let mut closure_errors: Option<HashMap<&str, Vec<BorrowError>>> = None;
    for _ in 0..SUMMARY_ROUNDS {
        let mut found = Effects::new();
        let mut round_errors = HashMap::new();
        for func in &closures {
            let (errors, effects_of) =
                FnChecker::new(func, &modes, symbols, &effects, skipped(func)).run();
            if !effects_of.is_empty() {
                found.insert(func.name.clone(), effects_of);
            }
            round_errors.insert(func.name.as_str(), errors);
        }
        if found == effects {
            closure_errors = Some(round_errors);
            break;
        }
        effects = found;
    }
    let mut closure_errors = closure_errors.unwrap_or_default();

    let mut errors = Vec::new();
    let mut reported = HashSet::new();
    for func in &mir.functions {
        if !checked(func) {
            continue;
        }
        let errors_of = match closure_errors.remove(func.name.as_str()) {
            Some(errors) => errors,
            None => {
                FnChecker::new(func, &modes, symbols, &effects, skipped(func))
                    .run()
                    .0
            }
        };
        for error in errors_of {
            let key = (
                error.code.code_str(),
                error.primary.span.line,
                error.primary.span.column,
            );
            if reported.insert(key) {
                errors.push(error);
            }
        }
    }
    errors
}

/// Every callee's declared [`ParamMode`]s, by the names a `Call` spells:
/// the runtime symbols the compiler emits itself (`residual_abi`), then the
/// `.rx` declarations (which win), then the program's functions.
struct CalleeModes<'a> {
    by_callee: HashMap<&'a str, &'a [ParamMode]>,
}

impl<'a> CalleeModes<'a> {
    fn new(mir: &'a MirProgram) -> Self {
        let mut by_callee: HashMap<&str, &[ParamMode]> = RESIDUAL_RUNTIME_FNS
            .iter()
            .map(|residual| (residual.name, residual.modes))
            .collect();
        for lib in &mir.ffi_libs {
            for decl in &lib.functions {
                by_callee.insert(decl.name.as_str(), &decl.param_modes);
                by_callee.insert(decl.ruxen_name.as_str(), &decl.param_modes);
            }
        }
        for func in &mir.functions {
            by_callee.insert(func.name.as_str(), &func.param_modes);
        }
        CalleeModes { by_callee }
    }

    /// The modes `callee` declares, under its own name or the runtime
    /// symbol it is linked as (`lang_intrinsics::runtime_name`); empty when
    /// it declares none.
    fn of(&self, callee: &str) -> &[ParamMode] {
        let linked = runtime_name(callee).unwrap_or(callee);
        [callee, linked]
            .iter()
            .find_map(|name| self.by_callee.get(name).copied())
            .unwrap_or(&[])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Place {
    root: LocalId,
    path: Vec<usize>,
}

impl Place {
    fn local(root: LocalId) -> Self {
        Place {
            root,
            path: Vec::new(),
        }
    }

    /// The place at `slot` inside this one; past `MAX_PLACE_DEPTH` slots,
    /// the capped place `INSIDE` this one, which stands for every deeper
    /// place.
    fn child(&self, slot: usize) -> Self {
        let mut path = self.path.clone();
        if path.len() < MAX_PLACE_DEPTH {
            path.push(slot);
        } else if path.len() == MAX_PLACE_DEPTH {
            path.push(INSIDE);
        }
        Place {
            root: self.root,
            path,
        }
    }

    /// Whether the path was cut at `MAX_PLACE_DEPTH`: the place stands for
    /// places of any depth below it.
    fn capped(&self) -> bool {
        self.path.len() > MAX_PLACE_DEPTH
    }

    /// Whether this place may be `outer` or lie inside it. `INSIDE` on
    /// either side stands for any path below that point.
    fn within(&self, outer: &Place) -> bool {
        if self.root != outer.root {
            return false;
        }
        for (index, theirs) in outer.path.iter().enumerate() {
            match self.path.get(index) {
                Some(&INSIDE) => return true,
                _ if *theirs == INSIDE => return true,
                Some(mine) if mine == theirs => {}
                _ => return false,
            }
        }
        true
    }
}

struct Loan {
    place: Place,
    span: Option<Span>,
}

type LoanId = usize;

/// The places a local's value may denote: its own storage
/// (`Place::local` of itself) when it may hold a value of its own, and each
/// place it may be a view of (or was moved out of).
type Places = BTreeSet<Place>;

/// A local's own storage, alone: a fresh value.
fn own(local: LocalId) -> Places {
    BTreeSet::from([Place::local(local)])
}

/// The place at `slot` inside each of `places`.
fn children(places: &Places, slot: usize) -> Places {
    places.iter().map(|place| place.child(slot)).collect()
}

/// What every local holds at one program point. A local missing from
/// `alias` has not been assigned on any path yet; otherwise its places are
/// the union over the paths reaching the point.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct State {
    holds: BTreeMap<LocalId, BTreeSet<LoanId>>,
    alias: BTreeMap<LocalId, Places>,
}

impl State {
    fn join(&mut self, other: &State) -> bool {
        let mut changed = false;
        for (local, loans) in &other.holds {
            let mine = self.holds.entry(*local).or_default();
            let before = mine.len();
            mine.extend(loans);
            changed |= mine.len() != before;
        }
        for (local, places) in &other.alias {
            let mine = self.alias.entry(*local).or_default();
            let before = mine.len();
            mine.extend(places.iter().cloned());
            changed |= mine.len() != before;
        }
        changed
    }

    fn held(&self, local: LocalId) -> BTreeSet<LoanId> {
        self.holds.get(&local).cloned().unwrap_or_default()
    }

    /// Whether `local` may hold a value of its own, so moving it may move
    /// that storage (moving a view moves only the view).
    fn may_own(&self, local: LocalId) -> bool {
        self.alias
            .get(&local)
            .is_none_or(|places| places.contains(&Place::local(local)))
    }

    fn places_of(&self, local: LocalId) -> Places {
        self.alias
            .get(&local)
            .cloned()
            .unwrap_or_else(|| own(local))
    }

    fn define(&mut self, local: LocalId, holds: BTreeSet<LoanId>, places: Places) {
        self.holds.insert(local, holds);
        self.alias.insert(local, places);
    }

    /// `local` now stores values holding `loans`: so do it and each local
    /// owning a place it denotes (a payload or field view stores into its
    /// object).
    fn store(&mut self, local: LocalId, loans: &BTreeSet<LoanId>) {
        let owners: BTreeSet<LocalId> = self
            .places_of(local)
            .iter()
            .map(|place| place.root)
            .chain([local])
            .collect();
        for holder in owners {
            self.holds
                .entry(holder)
                .or_default()
                .extend(loans.iter().copied());
        }
    }
}

/// What an instruction does to a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Access {
    Write,
    Move,
    /// A `ParamMode::Mut` argument; `slot` when the callee receives the
    /// place's own address and may replace its value.
    Mutate {
        slot: bool,
    },
}

impl Access {
    fn code(self) -> ErrorCode {
        match self {
            Access::Write | Access::Move => ErrorCode::E1009,
            Access::Mutate { .. } => ErrorCode::E1002,
        }
    }

    fn conflicts_with(self, loan: &Place, accessed: &Place) -> bool {
        match self {
            Access::Write | Access::Move | Access::Mutate { slot: true } => loan.within(accessed),
            // A capped loan may lie deeper than any place it is within.
            Access::Mutate { slot: false } => {
                loan.within(accessed) && (loan.path.len() > accessed.path.len() || loan.capped())
            }
        }
    }
}

/// What a closure body does below one of its captures: the captures box
/// `slot`, the path below it, and the access, or `None` for a loan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Effect {
    slot: usize,
    suffix: Vec<usize>,
    access: Option<Access>,
}

/// Every closure body's effects, by its function name.
type Effects = HashMap<String, BTreeSet<Effect>>;

/// The closure pairs a body builds (`View::Closure`), followed through the
/// locals they are copied into, and what each object slot was filled from.
#[derive(Default)]
struct Pairs {
    /// Each local that may hold a pair, to every local such a pair was
    /// built in: a join of paths holding different closures holds them all.
    pair_of: HashMap<LocalId, BTreeSet<LocalId>>,
    /// The places each object slot was last filled from, by object and slot.
    stored: HashMap<(LocalId, usize), Places>,
}

impl Pairs {
    fn track(&mut self, func: &MirFunction, state: &State, inst: &MirInst) {
        match inst {
            MirInst::Assign {
                dest,
                value: MirValue::Use(src),
            }
            | MirInst::Copy { dest, src }
            | MirInst::Move { dest, src } => {
                if let Some(pairs) = self.pair_of.get(src).cloned() {
                    self.pair_of.entry(*dest).or_default().extend(pairs);
                }
            }
            MirInst::SetField {
                base,
                field_index,
                value: MirValue::Use(value),
            } if !self.pair_of.contains_key(base) => {
                self.stored
                    .insert((*base, *field_index), state.places_of(*value));
            }
            _ => {}
        }
        if let Some(dest) = inst.dest() {
            if matches!(func.view(dest), Some(View::Closure { .. })) {
                self.pair_of.entry(dest).or_default().insert(dest);
            }
        }
    }

    /// The body and captures box of each pair `local` may hold.
    fn closures<'f>(&self, func: &'f MirFunction, local: LocalId) -> Vec<(&'f str, LocalId)> {
        self.pair_of
            .get(&local)
            .into_iter()
            .flatten()
            .filter_map(|&pair| match func.view(pair)? {
                View::Closure {
                    body,
                    captures: Some(captures),
                } => Some((body.as_str(), *captures)),
                _ => None,
            })
            .collect()
    }
}

struct FnChecker<'a> {
    func: &'a MirFunction,
    modes: &'a CalleeModes<'a>,
    symbols: &'a SymbolTable,
    /// The closure bodies' effects, as summarised so far.
    effects: &'a Effects,
    /// Instructions a later pass deletes, which are not checked.
    skipped: &'a HashSet<(BlockId, usize)>,
    loans: Vec<Loan>,
    loan_ids: HashMap<(BlockId, usize, Place), LoanId>,
    pairs: Pairs,
    /// This body's own effects, when it is a closure body.
    found: BTreeSet<Effect>,
}

/// The facts the replay needs at one instruction to check its accesses.
struct Here<'s> {
    live_after: &'s LocalSet,
    errors: &'s mut Vec<BorrowError>,
}

impl<'a> FnChecker<'a> {
    fn new(
        func: &'a MirFunction,
        modes: &'a CalleeModes<'a>,
        symbols: &'a SymbolTable,
        effects: &'a Effects,
        skipped: &'a HashSet<(BlockId, usize)>,
    ) -> Self {
        FnChecker {
            func,
            modes,
            symbols,
            effects,
            skipped,
            loans: Vec::new(),
            loan_ids: HashMap::new(),
            pairs: Pairs::default(),
            found: BTreeSet::new(),
        }
    }

    /// The body's errors, and its effects when it is a closure body.
    fn run(mut self) -> (Vec<BorrowError>, BTreeSet<Effect>) {
        let func = self.func;
        let mut entry: Vec<Option<State>> = vec![None; func.blocks.len()];
        let mut start = State::default();
        // A parameter taken by value owns its storage; one taken by
        // reference denotes the caller's place. Without recorded modes, the
        // body owns each parameter (every access to it is checked).
        for (position, &param) in func.params.iter().enumerate() {
            let places = match func.param_modes.get(position) {
                Some(ParamMode::Read | ParamMode::Mut) => {
                    BTreeSet::from([Place::local(param).child(DEREF)])
                }
                Some(ParamMode::Move) | None => own(param),
            };
            start.define(param, BTreeSet::new(), places);
        }
        entry[func.entry_block] = Some(start);
        let mut pending = vec![func.entry_block];
        while let Some(block_id) = pending.pop() {
            let Some(mut state) = entry[block_id].clone() else {
                continue;
            };
            for index in 0..func.blocks[block_id].instructions.len() {
                self.step(&mut state, block_id, index, None);
            }
            for successor in func.blocks[block_id].terminator.successors() {
                let changed = match &mut entry[successor] {
                    Some(existing) => existing.join(&state),
                    slot @ None => {
                        *slot = Some(state.clone());
                        true
                    }
                };
                if changed {
                    pending.push(successor);
                }
            }
        }

        let liveness = Liveness::compute(func);
        let mut errors = Vec::new();
        for (block_id, block_entry) in entry.iter().enumerate() {
            let Some(mut state) = block_entry.clone() else {
                continue;
            };
            let live_after = liveness.after_each(func, block_id);
            for (index, live) in live_after.iter().enumerate() {
                let here = Here {
                    live_after: live,
                    errors: &mut errors,
                };
                self.step(&mut state, block_id, index, Some(here));
            }
        }
        for (_, _, place) in self.loan_ids.keys() {
            if let Some((slot, suffix)) = self.below_captures(place) {
                self.found.insert(Effect {
                    slot,
                    suffix,
                    access: None,
                });
            }
        }
        (errors, self.found)
    }

    /// The captures box slot and the path below it, when `place` lies inside
    /// this closure body's captures box.
    fn below_captures(&self, place: &Place) -> Option<(usize, Vec<usize>)> {
        if self.func.role != MirFnRole::ClosureBody || self.func.params.first() != Some(&place.root)
        {
            return None;
        }
        let path = place.path.strip_prefix(&[DEREF]).unwrap_or(&place.path);
        let (&slot, suffix) = path.split_first()?;
        Some((slot, suffix.to_vec()))
    }

    fn ty(&self, local: LocalId) -> &'a Ty {
        &self.func.locals[local as usize].ty
    }

    fn is_scalar(&self, local: LocalId) -> bool {
        self.func.view(local) == Some(&View::Scalar)
    }

    /// How the callee takes the argument at `position`: as declared, else
    /// as an unknown callee would (`unknown_param_mode`).
    fn mode(&self, declared: &[ParamMode], position: usize, arg: &MirValue) -> ParamMode {
        match (declared.get(position), arg.local()) {
            (Some(&mode), _) => mode,
            (None, Some(local)) => unknown_param_mode(self.ty(local), self.symbols),
            (None, None) => ParamMode::Read,
        }
    }

    fn loan(&mut self, block: BlockId, index: usize, place: Place) -> LoanId {
        let key = (block, index, place.clone());
        if let Some(&id) = self.loan_ids.get(&key) {
            return id;
        }
        let id = self.loans.len();
        self.loans.push(Loan {
            place,
            span: self.func.blocks[block].span_at(index),
        });
        self.loan_ids.insert(key, id);
        id
    }

    /// `dest` is a view of `places` (inside `base`), holding a loan on each
    /// and every loan `base` holds, unless its recorded view says it owns
    /// its value.
    fn view_into(
        &mut self,
        state: &mut State,
        block: BlockId,
        index: usize,
        dest: LocalId,
        base: LocalId,
        places: Places,
    ) {
        if matches!(self.func.view(dest), Some(View::Scalar | View::Own)) {
            state.define(dest, BTreeSet::new(), own(dest));
            return;
        }
        let mut holds = state.held(base);
        for place in &places {
            holds.insert(self.loan(block, index, place.clone()));
        }
        state.define(dest, holds, places);
    }

    /// Apply instruction `index` of `block` to `state`; with `here`, first
    /// check what it does to places against the loans live there.
    fn step(&mut self, state: &mut State, block: BlockId, index: usize, mut here: Option<Here>) {
        if self.skipped.contains(&(block, index)) {
            return;
        }
        let func = self.func;
        let inst = &func.blocks[block].instructions[index];
        self.pairs.track(func, state, inst);
        match inst {
            MirInst::GetField {
                dest,
                base,
                field_index,
            }
            | MirInst::FieldAddr {
                dest,
                base,
                field_index,
            } => {
                let places = children(&state.places_of(*base), *field_index);
                self.view_into(state, block, index, *dest, *base, places);
            }
            MirInst::GetPayload { dest, src, .. } => {
                let places = children(&state.places_of(*src), INSIDE);
                self.view_into(state, block, index, *dest, *src, places);
            }
            MirInst::Ref { dest, src } | MirInst::RefMut { dest, src } => {
                let places = state.places_of(*src);
                self.view_into(state, block, index, *dest, *src, places);
            }
            MirInst::Assign {
                dest,
                value: MirValue::Use(src),
            }
            | MirInst::Copy { dest, src }
            | MirInst::Move { dest, src } => {
                if let Some(here) = here.as_mut() {
                    self.check_rebind(state, block, index, *dest, here);
                    // A `Copy` copies; an `Assign` or `Move` of a source
                    // binding moves the storage it owns.
                    if !matches!(inst, MirInst::Copy { .. })
                        && self.func.locals[*src as usize].kind == LocalKind::User
                        && state.may_own(*src)
                        && *dest != *src
                    {
                        self.check(
                            state,
                            block,
                            index,
                            Access::Move,
                            &own(*src),
                            *src,
                            &[],
                            here,
                        );
                    }
                }
                // `dest` denotes what `src` does, `src`'s own storage
                // becoming `dest`'s, unless it is a scalar or owns its value.
                if matches!(self.func.view(*dest), Some(View::Scalar | View::Own)) {
                    state.define(*dest, BTreeSet::new(), own(*dest));
                } else {
                    let src_own = Place::local(*src);
                    let alias = state
                        .places_of(*src)
                        .into_iter()
                        .map(|place| {
                            if place == src_own {
                                Place::local(*dest)
                            } else {
                                place
                            }
                        })
                        .collect();
                    state.define(*dest, state.held(*src), alias);
                }
            }
            MirInst::Assign { dest, .. } => {
                if let Some(here) = here.as_mut() {
                    self.check_rebind(state, block, index, *dest, here);
                }
                state.define(*dest, BTreeSet::new(), own(*dest));
            }
            MirInst::Call { dest, callee, args } => {
                // Field drop glue (recorded by its one emitter) frees what
                // the object owns: it consumes the object, as the free
                // after it does.
                let declared = if func.field_glue_calls.contains_key(callee) {
                    &[ParamMode::Move]
                } else {
                    self.modes.of(callee)
                };
                let modes: Vec<ParamMode> = args
                    .iter()
                    .enumerate()
                    .map(|(position, arg)| self.mode(declared, position, arg))
                    .collect();
                self.closure_effects(state, block, index, args, None, here.as_mut());
                self.call(state, block, index, *dest, args, &modes, here);
            }
            MirInst::CallIndirect {
                dest,
                args,
                modes,
                pair,
                ..
            } => {
                let modes: Vec<ParamMode> = args
                    .iter()
                    .enumerate()
                    .map(|(position, arg)| self.mode(modes, position, arg))
                    .collect();
                self.closure_effects(state, block, index, args, *pair, here.as_mut());
                self.call(state, block, index, *dest, args, &modes, here);
            }
            MirInst::SetField {
                base,
                field_index,
                value,
            } => {
                if let Some(here) = here.as_mut() {
                    let places = children(&state.places_of(*base), *field_index);
                    self.check(
                        state,
                        block,
                        index,
                        Access::Write,
                        &places,
                        *base,
                        &[],
                        here,
                    );
                }
                if let Some(stored) = value.local() {
                    let stored = state.held(stored);
                    state.store(*base, &stored);
                }
            }
            other => {
                if let Some(dest) = other.dest() {
                    state.define(dest, BTreeSet::new(), own(dest));
                }
            }
        }
    }

    /// A call: check each argument's access against the loans live after
    /// the call and those the other arguments hold, then let a by-reference
    /// argument absorb what the call takes by value, and bind the result to
    /// the arguments its recorded view says it may point into (every
    /// non-scalar argument when none is recorded).
    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        state: &mut State,
        block: BlockId,
        index: usize,
        dest: Option<LocalId>,
        args: &[MirValue],
        modes: &[ParamMode],
        mut here: Option<Here>,
    ) {
        let arg_locals: Vec<(usize, LocalId)> = args
            .iter()
            .enumerate()
            .filter_map(|(position, arg)| arg.local().map(|local| (position, local)))
            .collect();
        if let Some(here) = here.as_mut() {
            for &(position, local) in &arg_locals {
                let others: Vec<LocalId> = arg_locals
                    .iter()
                    .filter(|(other, _)| *other != position)
                    .map(|(_, other)| *other)
                    .collect();
                let access = match modes[position] {
                    ParamMode::Read => continue,
                    // Lowering's address for an `op=` target: the call
                    // assigns the place (E1009), as `=` does.
                    ParamMode::Mut if self.func.view(local) == Some(&View::Assignee) => {
                        Access::Write
                    }
                    ParamMode::Mut => Access::Mutate {
                        slot: crate::mir::refmut_abi::is_slot_refmut(self.ty(local)),
                    },
                    ParamMode::Move if state.may_own(local) => Access::Move,
                    ParamMode::Move => continue,
                };
                // A move takes only the storage of its own; a mutation
                // reaches every place the argument may denote.
                let places = match access {
                    Access::Move => own(local),
                    _ => state.places_of(local),
                };
                self.check(state, block, index, access, &places, local, &others, here);
            }
        }
        // A callee can keep what it takes by value in any other argument it
        // takes by reference (`push(x: T)` into an `Array[&String]`, a
        // closure into a field); a scalar keeps nothing.
        for &(position, stored) in &arg_locals {
            if modes[position] != ParamMode::Move {
                continue;
            }
            let loans = state.held(stored);
            if loans.is_empty() {
                continue;
            }
            for &(other, container) in &arg_locals {
                if other != position
                    && modes[other] != ParamMode::Move
                    && !self.is_scalar(container)
                {
                    state.store(container, &loans);
                }
            }
        }
        let Some(dest) = dest else {
            return;
        };
        let into: Vec<usize> = match self.func.view(dest) {
            Some(View::Args(positions)) => positions.clone(),
            Some(View::Scalar | View::Own) => Vec::new(),
            _ => arg_locals
                .iter()
                .filter(|(_, local)| !self.is_scalar(*local))
                .map(|(position, _)| *position)
                .collect(),
        };
        let mut holds = BTreeSet::new();
        let mut pointed_into = Places::new();
        // A value the callee took may be kept in a result that can hold
        // anything (`wrap(v)` returning an object holding `v`): the result
        // keeps that value's loans, whether or not it points into it.
        let result_holds = !self.is_scalar(dest);
        for &(position, local) in &arg_locals {
            let taken = modes[position] == ParamMode::Move;
            if !into.contains(&position) {
                if taken && result_holds {
                    holds.extend(state.held(local));
                }
                continue;
            }
            holds.extend(state.held(local));
            // A result made from a value the callee took keeps that value's
            // loans but points into nothing of the caller's.
            if taken {
                continue;
            }
            let inside = children(&state.places_of(local), INSIDE);
            if state.may_own(local) {
                for place in &inside {
                    holds.insert(self.loan(block, index, place.clone()));
                }
            }
            pointed_into.extend(inside);
        }
        // A view into arguments denotes a place inside each of them
        // (`ruxen_vec_slot_addr(its, 0)` is a slot of `its`;
        // `pick(c, a, b)` is inside `a` or `b`).
        let alias = if pointed_into.is_empty() {
            own(dest)
        } else {
            pointed_into
        };
        state.define(dest, holds, alias);
    }

    /// The closure pairs among a call's operands (the `called` pair when it
    /// calls one) do their bodies' effects: each takes its body's loans on
    /// the places its captures came from, then each access is checked with
    /// the call's other operands in use.
    fn closure_effects(
        &mut self,
        state: &mut State,
        block: BlockId,
        index: usize,
        args: &[MirValue],
        called: Option<LocalId>,
        mut here: Option<&mut Here>,
    ) {
        let operands: Vec<LocalId> = called
            .into_iter()
            .chain(args.iter().filter_map(MirValue::local))
            .collect();
        let effects = self.effects;
        let mut accesses = Vec::new();
        for &pair in &operands {
            for (body, captures) in self.pairs.closures(self.func, pair) {
                for effect in effects.get(body).into_iter().flatten() {
                    let Some(stored) = self.pairs.stored.get(&(captures, effect.slot)) else {
                        continue;
                    };
                    let places: Places = stored
                        .iter()
                        .map(|place| {
                            effect
                                .suffix
                                .iter()
                                .fold(place.clone(), |place, &slot| place.child(slot))
                        })
                        .collect();
                    match effect.access {
                        None => {
                            for place in places {
                                let loan = self.loan(block, index, place);
                                state.holds.entry(pair).or_default().insert(loan);
                            }
                        }
                        Some(access) => accesses.push((pair, access, places)),
                    }
                }
            }
        }
        let Some(here) = here.as_mut() else {
            return;
        };
        for (pair, access, places) in accesses {
            let others: Vec<LocalId> = operands.iter().copied().filter(|o| *o != pair).collect();
            self.check(state, block, index, access, &places, pair, &others, here);
        }
    }

    /// Reassigning a source binding that owns its value writes its storage.
    fn check_rebind(
        &mut self,
        state: &State,
        block: BlockId,
        index: usize,
        dest: LocalId,
        here: &mut Here,
    ) {
        let local = &self.func.locals[dest as usize];
        if local.kind == LocalKind::User && state.alias.contains_key(&dest) && state.may_own(dest) {
            self.check(
                state,
                block,
                index,
                Access::Write,
                &own(dest),
                dest,
                &[],
                here,
            );
        }
    }

    /// Report `access` to `places` (each place the accessed value may be)
    /// through `accessor` if a loan held by another live local (or another
    /// operand in use) lies where the access reaches one of them.
    #[allow(clippy::too_many_arguments)]
    fn check(
        &mut self,
        state: &State,
        block: BlockId,
        index: usize,
        access: Access,
        places: &Places,
        accessor: LocalId,
        in_use: &[LocalId],
        here: &mut Here,
    ) {
        for place in places {
            if let Some((slot, suffix)) = self.below_captures(place) {
                self.found.insert(Effect {
                    slot,
                    suffix,
                    access: Some(access),
                });
            }
        }
        let redefined = self.func.blocks[block].instructions[index].dest();
        let holders = state.holds.iter().filter(|(local, _)| {
            **local != accessor
                && Some(**local) != redefined
                && (here.live_after.contains(**local) || in_use.contains(local))
        });
        let conflict = holders
            .flat_map(|(_, loans)| loans.iter())
            .map(|&loan| &self.loans[loan])
            .find_map(|loan| {
                places
                    .iter()
                    .find(|place| access.conflicts_with(&loan.place, place))
                    .map(|place| (loan, place))
            });
        let Some((conflict, place)) = conflict else {
            return;
        };
        let Some(span) = self.func.blocks[block].span_at(index) else {
            return;
        };
        let name = self.place_name(place);
        let (primary, also) = match access {
            Access::Write => (
                format!("cannot assign to {name} while it is borrowed"),
                format!("{name} is borrowed here"),
            ),
            Access::Move => (
                format!("cannot move out of {name} while it is borrowed"),
                format!("{name} is borrowed here"),
            ),
            Access::Mutate { .. } => (
                format!("cannot mutably borrow {name}: already borrowed"),
                format!("{name} is also borrowed here"),
            ),
        };
        here.errors.push(BorrowError {
            code: access.code(),
            primary: SpanLabel {
                span,
                label: primary,
            },
            secondary: conflict
                .span
                .clone()
                .map(|span| SpanLabel { span, label: also })
                .into_iter()
                .collect(),
            help: vec!["ensure the previous borrow is no longer in use".to_string()],
        });
    }

    fn place_name(&self, place: &Place) -> String {
        let root = &self.func.locals[place.root as usize];
        match root.kind {
            LocalKind::User => format!("`{}`", root.name),
            LocalKind::Temp => "a temporary".to_string(),
        }
    }
}
