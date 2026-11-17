//! Body summaries: what a function's result borrows and carries, by
//! argument position ([`CallFact`]), found by the loan checker's own
//! transfer run in summary mode, in one joint fixpoint with the closure
//! bodies' loan effects.
//!
//! In summary mode each parameter `p` starts with one [`LoanOn::Held`]`(p)`
//! loan, standing for the references its value already holds. A parameter
//! taken by value holds it itself. One taken by reference denotes a
//! phantom caller place (a local past the body's own) that holds `Held(p)`,
//! and holds a place loan on it, as the caller's `&x` argument does: a
//! result that stores or returns the reference borrows `p`, while one that
//! keeps only what the place stores (`h.remove(k)` through `h: &var Hash`)
//! carries `p`. At each `Return` the returned local "borrows p" when a
//! place it denotes is rooted at `p` or its phantom, or it holds a place
//! loan rooted there (the loans `h.keys` takes inside a fresh array count),
//! and "carries p" when it holds `Held(p)`. A body the transfer cannot follow (no recorded
//! parameter modes) gets every non-scalar parameter in both halves.
//!
//! Each round records the call views of every body whose callees' facts
//! grew (`record_views`, lowering's `drops::record_default_views` over the
//! facts so far), reruns the transfer on those bodies and unions what it
//! finds with the previous round. Facts and effects only grow and are
//! finite, so the loop ends by construction, with no round cap.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::{own, Effects, FnChecker, Loan, LoanOn, Place, State};
use crate::mir::nodes::{
    CallFact, LocalId, MirFnRole, MirFunction, MirInst, MirProgram, MirValue, ParamMode,
    Terminator, View,
};
use crate::mir::param_modes::CalleeModes;
use crate::resolve::symbols::SymbolTable;

/// Each body's summary, by function name.
pub type BodyFacts = HashMap<String, CallFact>;

/// What the fixpoint found: every body's summary, every closure body's
/// loan effects, and how many rounds it took.
pub struct Fixpoint {
    pub facts: BodyFacts,
    pub effects: Effects,
    pub rounds: usize,
}

/// Summarise every body of `mir`. `record_views` records a body's call
/// views from the facts found so far; it is called again for a body each
/// time it is rerun, and last with the final facts.
pub fn fixpoint(
    mir: &mut MirProgram,
    modes: &CalleeModes,
    symbols: &SymbolTable,
    mut record_views: impl FnMut(&mut MirFunction, &BodyFacts),
) -> Fixpoint {
    let mut callers: HashMap<&str, BTreeSet<usize>> = HashMap::new();
    for (index, func) in mir.functions.iter().enumerate() {
        for inst in func.blocks.iter().flat_map(|block| &block.instructions) {
            if let MirInst::Call { callee, .. } = inst {
                callers.entry(callee.as_str()).or_default().insert(index);
            }
        }
    }
    let callers: HashMap<String, BTreeSet<usize>> = callers
        .into_iter()
        .map(|(callee, at)| (callee.to_string(), at))
        .collect();
    let mut facts = BodyFacts::new();
    let mut effects = Effects::new();
    let mut dirty: BTreeSet<usize> = (0..mir.functions.len()).collect();
    let mut rounds = 0;
    let skipped = HashSet::new();
    while !dirty.is_empty() {
        rounds += 1;
        for &index in &dirty {
            record_views(&mut mir.functions[index], &facts);
        }
        let mut found = Vec::new();
        for &index in &dirty {
            let func = &mir.functions[index];
            let checker = FnChecker::new(func, modes, symbols, &effects, &skipped);
            found.push(match func.role {
                MirFnRole::ClosureBody => (index, None, Some(checker.summarize_effects())),
                _ => (index, Some(checker.summarize()), None),
            });
        }
        let mut grew = BTreeSet::new();
        let mut effects_grew = false;
        for (index, fact, effects_of) in found {
            let name = &mir.functions[index].name;
            if let Some(fact) = fact {
                let known = facts.entry(name.clone()).or_default();
                if union(&mut known.borrows, &fact.borrows)
                    | union(&mut known.carries, &fact.carries)
                {
                    grew.extend(callers.get(name).into_iter().flatten().copied());
                }
            }
            if let Some(effects_of) = effects_of.filter(|found| !found.is_empty()) {
                let known = effects.entry(name.clone()).or_default();
                let before = known.len();
                known.extend(effects_of);
                effects_grew |= known.len() != before;
            }
        }
        // A closure effect reaches the bodies that build a closure pair.
        if effects_grew {
            grew.extend(
                mir.functions
                    .iter()
                    .enumerate()
                    .filter_map(|(index, func)| {
                        func.views
                            .values()
                            .any(|view| matches!(view, Some(View::Closure { .. })))
                            .then_some(index)
                    }),
            );
        }
        dirty = grew;
    }
    Fixpoint {
        facts,
        effects,
        rounds,
    }
}

/// Add `found` to `known` (kept sorted); whether it grew.
fn union(known: &mut Vec<usize>, found: &[usize]) -> bool {
    let before = known.len();
    known.extend(
        found
            .iter()
            .filter(|slot| !known.contains(slot))
            .copied()
            .collect::<Vec<_>>(),
    );
    known.sort_unstable();
    known.len() != before
}

impl FnChecker<'_> {
    /// This closure body's loan effects, from the transfer alone.
    fn summarize_effects(mut self) -> BTreeSet<super::Effect> {
        let start = self.start();
        self.transfer(start);
        self.loan_effects()
    }

    /// What this body's result borrows and carries, by parameter position.
    fn summarize(mut self) -> CallFact {
        let func = self.func;
        let mut param_position: HashMap<LocalId, usize> = func
            .params
            .iter()
            .enumerate()
            .map(|(position, &param)| (param, position))
            .collect();
        if func.param_modes.len() != func.params.len() {
            let all: Vec<usize> = func
                .params
                .iter()
                .enumerate()
                .filter(|(_, &param)| !self.is_scalar(param))
                .map(|(position, _)| position)
                .collect();
            return CallFact {
                borrows: all.clone(),
                carries: all,
            };
        }
        let mut start: State = self.start();
        for (position, &param) in func.params.iter().enumerate() {
            let held = self.loans.len();
            self.loans.push(Loan {
                on: LoanOn::Held(position),
                span: None,
            });
            // A parameter taken by reference points at a caller place that
            // holds the references: it denotes that place and holds a loan
            // on it, as a `&x` argument does in the caller.
            let by_reference =
                matches!(func.param_modes[position], ParamMode::Read | ParamMode::Mut);
            if !by_reference {
                start.holds.entry(param).or_default().insert(held);
                continue;
            }
            let caller = (func.locals.len() + position) as LocalId;
            param_position.insert(caller, position);
            let place = Place::local(caller);
            let lent = self.loans.len();
            self.loans.push(Loan {
                on: LoanOn::Place(place.clone()),
                span: None,
            });
            start.define(caller, BTreeSet::from([held]), own(caller));
            start.define(param, BTreeSet::from([lent]), BTreeSet::from([place]));
        }
        let entry = self.transfer(start);
        let mut borrows = BTreeSet::new();
        let mut carries = BTreeSet::new();
        for (block_id, block) in func.blocks.iter().enumerate() {
            let Terminator::Return(Some(MirValue::Use(returned))) = &block.terminator else {
                continue;
            };
            let Some(mut state) = entry[block_id].clone() else {
                continue;
            };
            if self.is_scalar(*returned) {
                continue;
            }
            for index in 0..block.instructions.len() {
                self.step(&mut state, block_id, index, None);
            }
            let rooted = state
                .places_of(*returned)
                .into_iter()
                .map(|place| place.root);
            borrows.extend(rooted.filter_map(|root| param_position.get(&root).copied()));
            for loan in state.held(*returned) {
                match &self.loans[loan].on {
                    LoanOn::Place(place) => {
                        borrows.extend(param_position.get(&place.root).copied());
                    }
                    LoanOn::Held(position) => {
                        carries.insert(*position);
                    }
                }
            }
        }
        CallFact {
            borrows: borrows.into_iter().collect(),
            carries: carries.into_iter().collect(),
        }
    }
}
