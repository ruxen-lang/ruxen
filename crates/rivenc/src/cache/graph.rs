//! File-level dependency graph with cycle detection and topological layering.
//!
//! The graph is a DAG: Riven forbids circular imports. Cycles are reported as
//! a `GraphError::Cycle` diagnostic rather than silently accepted.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

pub type FileId = usize;

/// Forward and reverse adjacency lists over file IDs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyGraph {
    /// `file → files it imports from`.
    pub dependencies: HashMap<FileId, HashSet<FileId>>,
    /// `file → files that import from it` (reverse of `dependencies`).
    pub dependents: HashMap<FileId, HashSet<FileId>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GraphError {
    /// Circular dependency detected. The returned path starts and ends on the
    /// same node, e.g. `[a, b, c, a]`.
    Cycle(Vec<FileId>),
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, file: FileId) {
        self.dependencies.entry(file).or_default();
        self.dependents.entry(file).or_default();
    }

    /// Record that `from` depends on `to`.
    pub fn add_edge(&mut self, from: FileId, to: FileId) {
        self.dependencies.entry(from).or_default().insert(to);
        self.dependents.entry(to).or_default().insert(from);
        // Keep both keys present even if they have no edges in the other direction.
        self.dependencies.entry(to).or_default();
        self.dependents.entry(from).or_default();
    }

    pub fn dependencies_of(&self, file: FileId) -> Option<&HashSet<FileId>> {
        self.dependencies.get(&file)
    }

    pub fn dependents_of(&self, file: FileId) -> Option<&HashSet<FileId>> {
        self.dependents.get(&file)
    }

    /// Collect all files transitively reachable from `start` through
    /// `dependents` edges (i.e. everyone downstream of `start`). The start
    /// node itself is included.
    pub fn transitive_dependents(&self, start: FileId) -> HashSet<FileId> {
        let mut out = HashSet::new();
        let mut stack = vec![start];
        while let Some(f) = stack.pop() {
            if !out.insert(f) {
                continue;
            }
            if let Some(ds) = self.dependents.get(&f) {
                for &d in ds {
                    if !out.contains(&d) {
                        stack.push(d);
                    }
                }
            }
        }
        out
    }

    /// Check for cycles via DFS. Returns `Err(Cycle)` with a representative
    /// path on the first cycle found.
    pub fn check_acyclic(&self) -> Result<(), GraphError> {
        #[derive(Copy, Clone, Eq, PartialEq)]
        enum State {
            Unvisited,
            InProgress,
            Done,
        }

        let mut state: HashMap<FileId, State> = self
            .dependencies
            .keys()
            .map(|&k| (k, State::Unvisited))
            .collect();

        let mut stack: Vec<FileId> = Vec::new();

        // Iterative DFS using an explicit work stack of (node, iter_index).
        let mut order: Vec<FileId> = self.dependencies.keys().copied().collect();
        order.sort(); // deterministic

        for &start in &order {
            if state[&start] != State::Unvisited {
                continue;
            }
            stack.clear();
            stack.push(start);
            state.insert(start, State::InProgress);

            let mut dfs: Vec<(FileId, std::vec::IntoIter<FileId>)> = Vec::new();
            let first_children: Vec<FileId> = self
                .dependencies
                .get(&start)
                .map(|s| {
                    let mut v: Vec<_> = s.iter().copied().collect();
                    v.sort();
                    v
                })
                .unwrap_or_default();
            dfs.push((start, first_children.into_iter()));

            while let Some((node, mut iter)) = dfs.pop() {
                match iter.next() {
                    Some(next) => {
                        // Re-push current frame.
                        dfs.push((node, iter));
                        match state.get(&next).copied().unwrap_or(State::Unvisited) {
                            State::InProgress => {
                                // Reconstruct a cycle by slicing `stack`
                                // from where `next` first appears.
                                let mut cycle: Vec<FileId> = dfs.iter().map(|(n, _)| *n).collect();
                                cycle.push(next);
                                if let Some(pos) = cycle.iter().position(|&n| n == next) {
                                    let c = cycle[pos..].to_vec();
                                    return Err(GraphError::Cycle(c));
                                }
                                return Err(GraphError::Cycle(cycle));
                            }
                            State::Unvisited => {
                                state.insert(next, State::InProgress);
                                let mut children: Vec<_> = self
                                    .dependencies
                                    .get(&next)
                                    .map(|s| s.iter().copied().collect())
                                    .unwrap_or_default();
                                children.sort();
                                dfs.push((next, children.into_iter()));
                            }
                            State::Done => {}
                        }
                    }
                    None => {
                        state.insert(node, State::Done);
                    }
                }
            }
        }
        Ok(())
    }

    /// Kahn-style topological layering: each level is a set of nodes whose
    /// dependencies are all in earlier levels. Within a level, nodes are
    /// independent and can be compiled in parallel.
    ///
    /// Returns `Err(Cycle)` if the graph is not a DAG.
    pub fn topological_levels(&self) -> Result<Vec<Vec<FileId>>, GraphError> {
        self.check_acyclic()?;

        let mut indeg: HashMap<FileId, usize> = self
            .dependencies
            .iter()
            .map(|(k, v)| (*k, v.len()))
            .collect();

        let mut queue: VecDeque<FileId> = indeg
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&k, _)| k)
            .collect();

        let mut levels: Vec<Vec<FileId>> = Vec::new();

        while !queue.is_empty() {
            let mut current: Vec<FileId> = queue.drain(..).collect();
            current.sort(); // deterministic ordering within a level
            let mut next_queue: VecDeque<FileId> = VecDeque::new();

            for &node in &current {
                if let Some(dependents) = self.dependents.get(&node) {
                    for &dep in dependents {
                        if let Some(d) = indeg.get_mut(&dep) {
                            *d = d.saturating_sub(1);
                            if *d == 0 {
                                next_queue.push_back(dep);
                            }
                        }
                    }
                }
            }

            levels.push(current);
            queue = next_queue;
        }

        Ok(levels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_is_acyclic() {
        let g = DependencyGraph::new();
        assert!(g.check_acyclic().is_ok());
        assert!(g.topological_levels().unwrap().is_empty());
    }

    #[test]
    fn simple_linear_chain_is_acyclic() {
        let mut g = DependencyGraph::new();
        // main -> task -> user
        g.add_edge(0, 1);
        g.add_edge(1, 2);
        assert!(g.check_acyclic().is_ok());
    }

    #[test]
    fn self_loop_is_a_cycle() {
        let mut g = DependencyGraph::new();
        g.add_edge(0, 0);
        match g.check_acyclic() {
            Err(GraphError::Cycle(_)) => {}
            other => panic!("expected Cycle, got {:?}", other),
        }
    }

    #[test]
    fn two_node_cycle_is_detected() {
        let mut g = DependencyGraph::new();
        g.add_edge(0, 1);
        g.add_edge(1, 0);
        assert!(matches!(g.check_acyclic(), Err(GraphError::Cycle(_))));
    }

    #[test]
    fn three_node_cycle_is_detected() {
        let mut g = DependencyGraph::new();
        g.add_edge(0, 1);
        g.add_edge(1, 2);
        g.add_edge(2, 0);
        assert!(matches!(g.check_acyclic(), Err(GraphError::Cycle(_))));
    }

    #[test]
    fn transitive_dependents_collects_downstream_files() {
        let mut g = DependencyGraph::new();
        // 2 is a leaf. 1 imports 2. 0 imports 1.
        g.add_edge(1, 2);
        g.add_edge(0, 1);
        let deps = g.transitive_dependents(2);
        assert!(deps.contains(&0));
        assert!(deps.contains(&1));
        assert!(deps.contains(&2));
    }

    #[test]
    fn topological_levels_order_leaves_first() {
        let mut g = DependencyGraph::new();
        // main(0) -> task(1) -> user(2)
        g.add_edge(0, 1);
        g.add_edge(1, 2);
        let levels = g.topological_levels().unwrap();
        assert_eq!(levels, vec![vec![2], vec![1], vec![0]]);
    }

    #[test]
    fn topological_levels_groups_independent_leaves() {
        let mut g = DependencyGraph::new();
        // main(0) depends on both 1 and 2, which are independent leaves.
        g.add_edge(0, 1);
        g.add_edge(0, 2);
        let levels = g.topological_levels().unwrap();
        assert_eq!(levels.len(), 2);
        let leaves: HashSet<_> = levels[0].iter().copied().collect();
        assert_eq!(leaves, [1, 2].into_iter().collect());
        assert_eq!(levels[1], vec![0]);
    }

    #[test]
    fn topological_levels_fails_on_cycle() {
        let mut g = DependencyGraph::new();
        g.add_edge(0, 1);
        g.add_edge(1, 0);
        assert!(matches!(g.topological_levels(), Err(GraphError::Cycle(_))));
    }

    #[test]
    fn graph_roundtrips_through_postcard() {
        let mut g = DependencyGraph::new();
        g.add_edge(0, 1);
        g.add_edge(2, 1);
        let bytes = postcard::to_allocvec(&g).unwrap();
        let recovered: DependencyGraph = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(g, recovered);
    }
}
