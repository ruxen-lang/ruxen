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
//!   operands in use. The loan effects come from the summary fixpoint
//!   ([`summary`]); the checking replay adds the access effects, rerunning
//!   the closure bodies until they stop growing (a pair passed inside a
//!   body is an access below that body's captures), at most
//!   `SUMMARY_ROUNDS` times.
//! - What a call result borrows and carries ([`CallFact`]) is the callee's
//!   declared fact plus its body summary: the transfer below run in summary
//!   mode, where each parameter holds a [`LoanOn::Held`] marker for the
//!   references it already holds ([`summary`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::borrow_check::errors::{BorrowError, ErrorCode, SpanLabel};
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::mir::dataflow::{Liveness, LocalSet};
use crate::mir::nodes::{
    BlockId, CallFact, LocalId, LocalKind, MirFnRole, MirFunction, MirInst, MirProgram, MirValue,
    ParamMode, View,
};
use crate::mir::param_modes::{call_stores, resolve_modes, CalleeModes};
use crate::resolve::symbols::SymbolTable;

mod closures;
pub mod summary;
pub use closures::Effects;
use closures::{Effect, Pairs};

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
/// `published` holds the closure bodies' loan effects the summary fixpoint
/// found ([`summary::fixpoint`]); the replay adds their access effects.
pub fn check_program(
    mir: &MirProgram,
    modes: &CalleeModes,
    symbols: &SymbolTable,
    elided: &HashMap<&str, HashSet<(BlockId, usize)>>,
    published: &Effects,
) -> Vec<BorrowError> {
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
    let mut effects = published.clone();
    let mut closure_errors: Option<HashMap<&str, Vec<BorrowError>>> = None;
    for _ in 0..SUMMARY_ROUNDS {
        let mut found = published.clone();
        let mut round_errors = HashMap::new();
        for func in &closures {
            let (errors, effects_of) =
                FnChecker::new(func, modes, symbols, &effects, skipped(func)).run();
            if !effects_of.is_empty() {
                found
                    .entry(func.name.clone())
                    .or_default()
                    .extend(effects_of);
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
                FnChecker::new(func, modes, symbols, &effects, skipped(func))
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
    on: LoanOn,
    span: Option<Span>,
}

/// What a loan is on: a place, or (summary mode only) the references
/// parameter `Held(p)` already held on entry. Conflict checks ignore
/// `Held` loans, so they never produce an error.
enum LoanOn {
    Place(Place),
    Held(usize),
}

type LoanId = usize;
/// A callee's argument modes and the `(from, into)` slots it may keep.
type CalleeShape<'a> = (&'a [ParamMode], Option<&'a [(usize, usize)]>);

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

    /// The loans stored in what `local` denotes: its own when it may own
    /// its value, else those of each local owning a place it denotes (a
    /// view's loan on that place is not stored in it).
    fn stored(&self, local: LocalId) -> BTreeSet<LoanId> {
        if self.may_own(local) {
            return self.held(local);
        }
        self.places_of(local)
            .iter()
            .flat_map(|place| self.held(place.root))
            .collect()
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

struct FnChecker<'a> {
    func: &'a MirFunction,
    modes: &'a CalleeModes,
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
        modes: &'a CalleeModes,
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
        let start = self.start();
        let entry = self.transfer(start);
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
        let found = self.loan_effects();
        (errors, found)
    }

    /// This closure body's effects found so far, with a loan effect for
    /// each loan below its captures.
    fn loan_effects(mut self) -> BTreeSet<Effect> {
        for (_, _, place) in self.loan_ids.keys() {
            if let Some((slot, suffix)) = self.below_captures(place) {
                self.found.insert(Effect {
                    slot,
                    suffix,
                    access: None,
                });
            }
        }
        self.found
    }

    /// The state on entry: a parameter taken by value owns its storage;
    /// one taken by reference denotes the caller's place. Without recorded
    /// modes, the body owns each parameter (every access to it is checked).
    fn start(&self) -> State {
        let func = self.func;
        let mut start = State::default();
        for (position, &param) in func.params.iter().enumerate() {
            let places = match func.param_modes.get(position) {
                Some(ParamMode::Read | ParamMode::Mut) => {
                    BTreeSet::from([Place::local(param).child(DEREF)])
                }
                Some(ParamMode::Move) | None => own(param),
            };
            start.define(param, BTreeSet::new(), places);
        }
        start
    }

    /// The state on entry to each block (`None` when unreachable), the
    /// transfer run to its fixpoint from `start`.
    fn transfer(&mut self, start: State) -> Vec<Option<State>> {
        let func = self.func;
        let mut entry: Vec<Option<State>> = vec![None; func.blocks.len()];
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
        entry
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

    fn loan(&mut self, block: BlockId, index: usize, place: Place) -> LoanId {
        let key = (block, index, place.clone());
        if let Some(&id) = self.loan_ids.get(&key) {
            return id;
        }
        let id = self.loans.len();
        self.loans.push(Loan {
            on: LoanOn::Place(place),
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
                let modes = self.modes.of_call(func, callee, args, self.symbols);
                let keeps = self.modes.keeps(callee);
                self.closure_effects(state, block, index, args, None, here.as_mut());
                self.call(state, block, index, *dest, args, (&modes, keeps), here);
            }
            MirInst::CallIndirect {
                dest,
                args,
                modes,
                pair,
                ..
            } => {
                let modes = resolve_modes(modes, func, args, self.symbols);
                self.closure_effects(state, block, index, args, *pair, here.as_mut());
                self.call(state, block, index, *dest, args, (&modes, None), here);
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
            // A value computed fresh: it holds no loan and owns itself.
            MirInst::BinOp { dest, .. }
            | MirInst::Negate { dest, .. }
            | MirInst::Not { dest, .. }
            | MirInst::Compare { dest, .. }
            | MirInst::Alloc { dest, .. }
            | MirInst::StackAlloc { dest, .. }
            | MirInst::GetTag { dest, .. }
            | MirInst::StringLiteral { dest, .. }
            | MirInst::FuncAddr { dest, .. }
            | MirInst::DataAddr { dest, .. }
            | MirInst::FloatToBits { dest, .. }
            | MirInst::BitsToFloat { dest, .. }
            // (No pass before codegen sees a `Convert`.)
            | MirInst::Convert { dest, .. } => {
                state.define(*dest, BTreeSet::new(), own(*dest));
            }
            // A tag write, a drop and a statement marker move no loan.
            MirInst::SetTag { .. } | MirInst::Drop { .. } | MirInst::Nop => {}
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
        (modes, keeps): CalleeShape<'_>,
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
        // What the callee may keep in another argument (`call_stores`):
        // that argument holds the stored value's loans from then on.
        let stores = call_stores(self.func, args, modes, keeps, |local| self.is_scalar(local));
        for (from, into) in stores {
            let (Some(value), Some(container)) = (args[from].local(), args[into].local()) else {
                continue;
            };
            let loans = state.held(value);
            state.store(container, &loans);
        }
        let Some(dest) = dest else {
            return;
        };
        // What the result points into (`borrows`) and whose loans it
        // carries, from its recorded call fact; with none recorded, every
        // non-scalar argument in both halves (when it may hold a
        // reference). A `Holds` result points into the arguments too, but
        // it is a fresh value: it keeps denoting its own storage.
        let owns_storage = matches!(self.func.view(dest), Some(View::Holds(_)));
        let fact = match self.func.view(dest) {
            Some(View::Args(fact) | View::Holds(fact)) => fact.clone(),
            Some(View::Scalar | View::Own) => CallFact::default(),
            _ => {
                let all: Vec<usize> = arg_locals
                    .iter()
                    .filter(|(_, local)| !self.is_scalar(*local))
                    .map(|(position, _)| *position)
                    .collect();
                let carries = if self.ty(dest).may_hold_reference(&mut |_| true) {
                    all.clone()
                } else {
                    Vec::new()
                };
                CallFact {
                    borrows: all,
                    carries,
                }
            }
        };
        let mut holds = BTreeSet::new();
        let mut pointed_into = Places::new();
        // A value the callee took may be kept in a result that can hold
        // anything (`wrap(v)` returning an object holding `v`): the result
        // keeps that value's loans, whether or not it points into it, as
        // it keeps the loans of each argument it carries.
        let result_holds = !self.is_scalar(dest);
        for &(position, local) in &arg_locals {
            let taken = modes[position] == ParamMode::Move;
            if !fact.borrows.contains(&position) {
                if taken && result_holds {
                    holds.extend(state.held(local));
                } else if fact.carries.contains(&position) && result_holds {
                    holds.extend(state.stored(local));
                }
                continue;
            }
            holds.extend(state.held(local));
            // A result made from a value the callee took keeps that value's
            // loans but points into nothing of the caller's.
            if taken {
                continue;
            }
            // The loan is on what the argument denotes, whether it owns
            // that storage or reaches it through a reference (a field
            // `@t`, a `&var` parameter, a `&` local).
            let inside = children(&state.places_of(local), INSIDE);
            for place in &inside {
                holds.insert(self.loan(block, index, place.clone()));
            }
            pointed_into.extend(inside);
        }
        // A view into arguments denotes a place inside each of them
        // (`ruxen_vec_slot_addr(its, 0)` is a slot of `its`;
        // `pick(c, a, b)` is inside `a` or `b`).
        let alias = if owns_storage || pointed_into.is_empty() {
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
                let LoanOn::Place(on) = &loan.on else {
                    return None;
                };
                places
                    .iter()
                    .find(|place| access.conflicts_with(on, place))
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
