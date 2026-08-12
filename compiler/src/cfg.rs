use crate::mir::{BlockId, Function, Terminator};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct ControlFlowGraph {
    successors: Vec<Vec<BlockId>>,
    predecessors: Vec<Vec<BlockId>>,
}

impl ControlFlowGraph {
    pub fn new(function: &Function) -> Self {
        let mut successors = Vec::with_capacity(function.blocks.len());
        for block in &function.blocks {
            let mut edges = match &block.terminator {
                Terminator::Goto(target) => vec![*target],
                Terminator::SwitchBool {
                    true_block,
                    false_block,
                    ..
                } => vec![*true_block, *false_block],
                Terminator::SwitchValue {
                    targets, otherwise, ..
                } => targets
                    .iter()
                    .map(|(_, block)| *block)
                    .chain([*otherwise])
                    .collect(),
                Terminator::SwitchEnum {
                    targets, otherwise, ..
                } => targets
                    .iter()
                    .map(|(_, block)| *block)
                    .chain([*otherwise])
                    .collect(),
                Terminator::Call { next, unwind, .. } => {
                    std::iter::once(*next).chain(*unwind).collect()
                }
                Terminator::Spawn { next, .. } => vec![*next],
                Terminator::Await { next, .. } => vec![*next],
                Terminator::Return | Terminator::Unreachable => Vec::new(),
            };
            edges.sort_unstable();
            edges.dedup();
            successors.push(edges);
        }
        let mut predecessors = vec![Vec::new(); function.blocks.len()];
        for (source, targets) in successors.iter().enumerate() {
            for target in targets {
                predecessors[target.0].push(BlockId(source));
            }
        }
        for edges in &mut predecessors {
            edges.sort_unstable();
            edges.dedup();
        }
        Self {
            successors,
            predecessors,
        }
    }

    pub fn successors(&self, block: BlockId) -> &[BlockId] {
        &self.successors[block.0]
    }
    pub fn predecessors(&self, block: BlockId) -> &[BlockId] {
        &self.predecessors[block.0]
    }
    pub fn reachable(&self) -> HashSet<BlockId> {
        let mut seen = HashSet::new();
        let mut stack = vec![BlockId(0)];
        while let Some(block) = stack.pop() {
            if seen.insert(block) {
                stack.extend(self.successors(block));
            }
        }
        seen
    }
    pub fn unreachable(&self) -> Vec<BlockId> {
        let reachable = self.reachable();
        (0..self.successors.len())
            .map(BlockId)
            .filter(|block| !reachable.contains(block))
            .collect()
    }
    pub fn reverse_postorder(&self) -> Vec<BlockId> {
        fn visit(
            cfg: &ControlFlowGraph,
            block: BlockId,
            seen: &mut HashSet<BlockId>,
            order: &mut Vec<BlockId>,
        ) {
            if seen.insert(block) {
                for next in cfg.successors(block) {
                    visit(cfg, *next, seen, order);
                }
                order.push(block);
            }
        }
        let mut order = Vec::new();
        visit(self, BlockId(0), &mut HashSet::new(), &mut order);
        order.reverse();
        order
    }
    pub fn has_back_edge(&self) -> bool {
        let order = self.reverse_postorder();
        let position = order
            .iter()
            .enumerate()
            .map(|(i, block)| (*block, i))
            .collect::<std::collections::HashMap<_, _>>();
        self.successors.iter().enumerate().any(|(source, targets)| {
            targets.iter().any(|target| {
                position
                    .get(target)
                    .zip(position.get(&BlockId(source)))
                    .is_some_and(|(target, source)| target <= source)
            })
        })
    }
}
