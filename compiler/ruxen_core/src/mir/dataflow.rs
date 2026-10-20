//! Control-flow facts every MIR pass shares: predecessors, a dense set of
//! locals, and backward liveness. Built from `Terminator::successors`,
//! `MirInst::dest` and `MirInst::read_locals`, so a pass never restates
//! which operands an instruction reads.

use crate::mir::nodes::{BlockId, LocalId, MirFunction};

/// Each block's predecessors, indexed by `BlockId`.
pub fn predecessors(func: &MirFunction) -> Vec<Vec<BlockId>> {
    let mut preds = vec![Vec::new(); func.blocks.len()];
    for (block_id, block) in func.blocks.iter().enumerate() {
        for successor in block.terminator.successors() {
            if let Some(list) = preds.get_mut(successor) {
                list.push(block_id);
            }
        }
    }
    preds
}

/// A set of locals of one function, one bit per `LocalId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSet {
    words: Vec<u64>,
}

impl LocalSet {
    pub fn new(local_count: usize) -> Self {
        LocalSet {
            words: vec![0; local_count.div_ceil(64)],
        }
    }

    pub fn insert(&mut self, local: LocalId) {
        let index = local as usize;
        self.words[index / 64] |= 1 << (index % 64);
    }

    pub fn remove(&mut self, local: LocalId) {
        let index = local as usize;
        self.words[index / 64] &= !(1 << (index % 64));
    }

    pub fn contains(&self, local: LocalId) -> bool {
        let index = local as usize;
        self.words
            .get(index / 64)
            .is_some_and(|word| word & (1 << (index % 64)) != 0)
    }

    /// Add every member of `other`; true when this set grew.
    pub fn union_with(&mut self, other: &LocalSet) -> bool {
        let mut grew = false;
        for (word, extra) in self.words.iter_mut().zip(&other.words) {
            let merged = *word | extra;
            grew |= merged != *word;
            *word = merged;
        }
        grew
    }
}

/// Which locals may still be read later: a local is live at a point when
/// some path from it reads the local before redefining it.
pub struct Liveness {
    live_out: Vec<LocalSet>,
}

impl Liveness {
    pub fn compute(func: &MirFunction) -> Self {
        let empty = LocalSet::new(func.locals.len());
        let mut live_in = vec![empty.clone(); func.blocks.len()];
        let mut live_out = vec![empty; func.blocks.len()];
        let mut changed = true;
        while changed {
            changed = false;
            for block_id in (0..func.blocks.len()).rev() {
                let mut out = LocalSet::new(func.locals.len());
                for successor in func.blocks[block_id].terminator.successors() {
                    if let Some(successor_in) = live_in.get(successor) {
                        out.union_with(successor_in);
                    }
                }
                let mut entry = out.clone();
                Self::step_block_backward(func, block_id, &mut entry, |_, _| {});
                changed |= live_out[block_id].union_with(&out);
                changed |= live_in[block_id].union_with(&entry);
            }
        }
        Liveness { live_out }
    }

    /// The live set right after each instruction of `block_id`, indexed by
    /// instruction position.
    pub fn after_each(&self, func: &MirFunction, block_id: BlockId) -> Vec<LocalSet> {
        let count = func.blocks[block_id].instructions.len();
        let mut after = vec![LocalSet::new(func.locals.len()); count];
        let mut live = self.live_out[block_id].clone();
        Self::step_block_backward(func, block_id, &mut live, |index, live_after| {
            after[index] = live_after.clone();
        });
        after
    }

    /// Walk `block_id` backward from `live` (its live-out), turning it into
    /// the live-in; `seen(index, live_after)` sees the set right after each
    /// instruction.
    fn step_block_backward(
        func: &MirFunction,
        block_id: BlockId,
        live: &mut LocalSet,
        mut seen: impl FnMut(usize, &LocalSet),
    ) {
        let block = &func.blocks[block_id];
        if let Some(local) = block.terminator.read_local() {
            live.insert(local);
        }
        for (index, inst) in block.instructions.iter().enumerate().rev() {
            seen(index, live);
            if let Some(dest) = inst.dest() {
                live.remove(dest);
            }
            for local in inst.read_locals() {
                live.insert(local);
            }
        }
    }
}
