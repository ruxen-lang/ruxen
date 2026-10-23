//! The closure facts the loan checker follows: each closure body's effects
//! on its captures ([`Effect`]) and the closure pairs a body builds and
//! copies ([`Pairs`]).

use std::collections::{BTreeSet, HashMap};

use super::{Access, Places, State};
use crate::mir::nodes::{LocalId, MirFunction, MirInst, MirValue, View};

/// What a closure body does below one of its captures: the captures box
/// `slot`, the path below it, and the access, or `None` for a loan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Effect {
    pub(super) slot: usize,
    pub(super) suffix: Vec<usize>,
    pub(super) access: Option<Access>,
}

/// Every closure body's effects, by its function name.
pub(super) type Effects = HashMap<String, BTreeSet<Effect>>;

/// The closure pairs a body builds (`View::Closure`), followed through the
/// locals they are copied into, and what each object slot was filled from.
#[derive(Default)]
pub(super) struct Pairs {
    /// Each local that may hold a pair, to every local such a pair was
    /// built in: a join of paths holding different closures holds them all.
    pub(super) pair_of: HashMap<LocalId, BTreeSet<LocalId>>,
    /// The places each object slot was last filled from, by object and slot.
    pub(super) stored: HashMap<(LocalId, usize), Places>,
}

impl Pairs {
    pub(super) fn track(&mut self, func: &MirFunction, state: &State, inst: &MirInst) {
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
    pub(super) fn closures<'f>(
        &self,
        func: &'f MirFunction,
        local: LocalId,
    ) -> Vec<(&'f str, LocalId)> {
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
