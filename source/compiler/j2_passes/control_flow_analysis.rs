//! Control-flow, post-dominator, and control-dependence analysis.
//! Original implementation by Jayithi Gavva.

use rustc_data_structures::fx::FxHashSet;
use rustc_middle::mir::{BasicBlock, Body, START_BLOCK, TerminatorKind};
use smallvec::SmallVec;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// CFG node, basic block or EXIT
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum CfgNode {
    /// A regular basic block
    Block(BasicBlock),
    /// The single explicit EXIT node
    Exit,
}

impl CfgNode {

    pub fn as_block(&self) -> Option<BasicBlock> {
        match self {
            CfgNode::Block(bb) => Some(*bb),
            CfgNode::Exit => None,
        }
    }

    pub fn is_exit(&self) -> bool {
        matches!(self, CfgNode::Exit)
    }
}

impl fmt::Display for CfgNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CfgNode::Block(bb) => write!(f, "bb{}", bb.as_usize()),
            CfgNode::Exit => write!(f, "EXIT"),
        }
    }
}

impl From<BasicBlock> for CfgNode {
    fn from(bb: BasicBlock) -> Self {
        CfgNode::Block(bb)
    }
}

/// CFG edge classification by terminator type
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EdgeKind {
    /// Edge from goto terminator
    Goto,
    /// switchInt edge, discriminant value or otherwise
    SwitchInt(Option<u128>),
    /// Edge from assert terminator (success path)
    AssertSuccess,
    /// Edge from assert terminator (unwind/cleanup path)
    AssertUnwind,
    /// Edge to EXIT from return terminator
    Return,
    /// Edge from call terminator (normal return)
    CallReturn,
    /// Edge from call terminator (unwind)
    CallUnwind,
    /// Edge from drop terminator
    Drop,
    /// Edge from yield terminator
    Yield,
    /// Edge from inline assembly
    InlineAsm,
    /// Edge from false edge (for borrow checker)
    FalseEdge,
    /// Edge from false unwind
    FalseUnwind,
    /// Fallthrough or other edge types
    Other,
}

/// CFG edge with source, target, kind
#[derive(Clone, Debug)]
pub struct CfgEdge {
    pub from: CfgNode,
    pub to: CfgNode,
    pub kind: EdgeKind,
}

/// Control Flow Graph representation using adjacency lists
#[derive(Clone, Debug)]
pub struct ControlFlowGraph {
    /// Number of basic blocks (not including EXIT)

    _num_blocks: usize,
    /// Forward edges, node to (successor, kind)
    successors: BTreeMap<CfgNode, SmallVec<[(CfgNode, EdgeKind); 2]>>,
    /// Backward edges, node to (predecessor, kind)
    predecessors: BTreeMap<CfgNode, SmallVec<[(CfgNode, EdgeKind); 4]>>,
    /// All nodes in the CFG
    nodes: Vec<CfgNode>,
}

impl ControlFlowGraph {

    /// Build a CFG from a MIR Body
    pub fn build<'tcx>(body: &Body<'tcx>) -> Self {
        let num_blocks = body.basic_blocks.len();
        let mut successors: BTreeMap<CfgNode, SmallVec<[(CfgNode, EdgeKind); 2]>> = BTreeMap::new();
        let mut predecessors: BTreeMap<CfgNode, SmallVec<[(CfgNode, EdgeKind); 4]>> =
            BTreeMap::new();

        let mut nodes: Vec<CfgNode> =
            (0..num_blocks).map(|i| CfgNode::Block(BasicBlock::from_usize(i))).collect();
        nodes.push(CfgNode::Exit);

        // Initialize empty adjacency lists for all nodes
        for node in &nodes {
            successors.entry(*node).or_default();
            predecessors.entry(*node).or_default();
        }

        for (bb_idx, bb_data) in body.basic_blocks.iter_enumerated() {
            let from_node = CfgNode::Block(bb_idx);

            if let Some(terminator) = &bb_data.terminator {
                Self::add_edges_from_terminator(
                    from_node,
                    &terminator.kind,
                    &mut successors,
                    &mut predecessors,
                );
            }
        }

        ControlFlowGraph { _num_blocks: num_blocks, successors, predecessors, nodes }
    }

    fn add_edges_from_terminator(
        from: CfgNode,
        kind: &TerminatorKind<'_>,
        successors: &mut BTreeMap<CfgNode, SmallVec<[(CfgNode, EdgeKind); 2]>>,
        predecessors: &mut BTreeMap<CfgNode, SmallVec<[(CfgNode, EdgeKind); 4]>>,
    ) {
        match kind {

            TerminatorKind::Goto { target } => {
                let to = CfgNode::Block(*target);
                successors.entry(from).or_default().push((to, EdgeKind::Goto));
                predecessors.entry(to).or_default().push((from, EdgeKind::Goto));
            }

            TerminatorKind::SwitchInt { targets, .. } => {
                // Add edges for each value-target pair
                for (value, target) in targets.iter() {
                    let to = CfgNode::Block(target);
                    let edge_kind = EdgeKind::SwitchInt(Some(value));
                    successors.entry(from).or_default().push((to, edge_kind));
                    predecessors.entry(to).or_default().push((from, edge_kind));
                }
                // Add edge for "otherwise" target
                let otherwise = targets.otherwise();
                let to = CfgNode::Block(otherwise);
                let edge_kind = EdgeKind::SwitchInt(None);
                successors.entry(from).or_default().push((to, edge_kind));
                predecessors.entry(to).or_default().push((from, edge_kind));
            }

            TerminatorKind::Assert { target, unwind, .. } => {
                let to = CfgNode::Block(*target);
                successors.entry(from).or_default().push((to, EdgeKind::AssertSuccess));
                predecessors.entry(to).or_default().push((from, EdgeKind::AssertSuccess));

                // Add unwind edge if present
                if let rustc_middle::mir::UnwindAction::Cleanup(unwind_bb) = unwind {
                    let unwind_to = CfgNode::Block(*unwind_bb);
                    successors.entry(from).or_default().push((unwind_to, EdgeKind::AssertUnwind));
                    predecessors.entry(unwind_to).or_default().push((from, EdgeKind::AssertUnwind));
                }
            }

            TerminatorKind::Return => {
                let to = CfgNode::Exit;
                successors.entry(from).or_default().push((to, EdgeKind::Return));
                predecessors.entry(to).or_default().push((from, EdgeKind::Return));
            }

            TerminatorKind::Call { target, unwind, .. } => {
                if let Some(target_bb) = target {
                    let to = CfgNode::Block(*target_bb);
                    successors.entry(from).or_default().push((to, EdgeKind::CallReturn));
                    predecessors.entry(to).or_default().push((from, EdgeKind::CallReturn));
                } else {
                    // Diverging call goes to EXIT
                    successors.entry(from).or_default().push((CfgNode::Exit, EdgeKind::CallReturn));
                    predecessors
                        .entry(CfgNode::Exit)
                        .or_default()
                        .push((from, EdgeKind::CallReturn));
                }

                if let rustc_middle::mir::UnwindAction::Cleanup(unwind_bb) = unwind {
                    let unwind_to = CfgNode::Block(*unwind_bb);
                    successors.entry(from).or_default().push((unwind_to, EdgeKind::CallUnwind));
                    predecessors.entry(unwind_to).or_default().push((from, EdgeKind::CallUnwind));
                }
            }

            TerminatorKind::Drop { target, unwind, .. } => {
                let to = CfgNode::Block(*target);
                successors.entry(from).or_default().push((to, EdgeKind::Drop));
                predecessors.entry(to).or_default().push((from, EdgeKind::Drop));

                if let rustc_middle::mir::UnwindAction::Cleanup(unwind_bb) = unwind {
                    let unwind_to = CfgNode::Block(*unwind_bb);
                    successors.entry(from).or_default().push((unwind_to, EdgeKind::Drop));
                    predecessors.entry(unwind_to).or_default().push((from, EdgeKind::Drop));
                }
            }

            TerminatorKind::Yield { resume, drop, .. } => {
                let to = CfgNode::Block(*resume);
                successors.entry(from).or_default().push((to, EdgeKind::Yield));
                predecessors.entry(to).or_default().push((from, EdgeKind::Yield));

                if let Some(drop_bb) = drop {
                    let drop_to = CfgNode::Block(*drop_bb);
                    successors.entry(from).or_default().push((drop_to, EdgeKind::Yield));
                    predecessors.entry(drop_to).or_default().push((from, EdgeKind::Yield));
                }
            }

            TerminatorKind::InlineAsm { targets, unwind, .. } => {
                for target in targets.iter() {
                    let to = CfgNode::Block(*target);
                    successors.entry(from).or_default().push((to, EdgeKind::InlineAsm));
                    predecessors.entry(to).or_default().push((from, EdgeKind::InlineAsm));
                }

                if let rustc_middle::mir::UnwindAction::Cleanup(unwind_bb) = unwind {
                    let unwind_to = CfgNode::Block(*unwind_bb);
                    successors.entry(from).or_default().push((unwind_to, EdgeKind::InlineAsm));
                    predecessors.entry(unwind_to).or_default().push((from, EdgeKind::InlineAsm));
                }
            }

            TerminatorKind::FalseEdge { real_target, imaginary_target } => {
                let real_to = CfgNode::Block(*real_target);
                successors.entry(from).or_default().push((real_to, EdgeKind::FalseEdge));
                predecessors.entry(real_to).or_default().push((from, EdgeKind::FalseEdge));

                let imag_to = CfgNode::Block(*imaginary_target);
                successors.entry(from).or_default().push((imag_to, EdgeKind::FalseEdge));
                predecessors.entry(imag_to).or_default().push((from, EdgeKind::FalseEdge));
            }

            TerminatorKind::FalseUnwind { real_target, unwind } => {
                let to = CfgNode::Block(*real_target);
                successors.entry(from).or_default().push((to, EdgeKind::FalseUnwind));
                predecessors.entry(to).or_default().push((from, EdgeKind::FalseUnwind));

                if let rustc_middle::mir::UnwindAction::Cleanup(unwind_bb) = unwind {
                    let unwind_to = CfgNode::Block(*unwind_bb);
                    successors.entry(from).or_default().push((unwind_to, EdgeKind::FalseUnwind));
                    predecessors.entry(unwind_to).or_default().push((from, EdgeKind::FalseUnwind));
                }
            }

            TerminatorKind::UnwindResume
            | TerminatorKind::UnwindTerminate(_)
            | TerminatorKind::Unreachable
            | TerminatorKind::CoroutineDrop
            | TerminatorKind::TailCall { .. } => {
                successors.entry(from).or_default().push((CfgNode::Exit, EdgeKind::Other));
                predecessors.entry(CfgNode::Exit).or_default().push((from, EdgeKind::Other));
            }
        }
    }

    pub fn nodes(&self) -> &[CfgNode] {
        &self.nodes
    }

    pub fn successors(&self, node: CfgNode) -> impl Iterator<Item = (CfgNode, EdgeKind)> + '_ {
        self.successors.get(&node).map(|v| v.iter().copied()).into_iter().flatten()
    }

    pub fn predecessors(&self, node: CfgNode) -> impl Iterator<Item = (CfgNode, EdgeKind)> + '_ {
        self.predecessors.get(&node).map(|v| v.iter().copied()).into_iter().flatten()
    }

    pub fn num_successors(&self, node: CfgNode) -> usize {
        self.successors.get(&node).map(|v| v.len()).unwrap_or(0)
    }

    pub fn is_branch(&self, node: CfgNode) -> bool {
        self.num_successors(node) > 1
    }

    pub fn entry(&self) -> CfgNode {
        CfgNode::Block(START_BLOCK)
    }

    pub fn exit(&self) -> CfgNode {
        CfgNode::Exit
    }

    pub fn edges(&self) -> Vec<CfgEdge> {
        let mut edges = Vec::new();
        for (&from, succs) in &self.successors {
            for &(to, kind) in succs {
                edges.push(CfgEdge { from, to, kind });
            }
        }
        edges
    }

    pub fn reverse_postorder(&self) -> Vec<CfgNode> {
        let mut visited = FxHashSet::default();
        let mut postorder = Vec::new();

        fn dfs(
            node: CfgNode,
            cfg: &ControlFlowGraph,
            visited: &mut FxHashSet<CfgNode>,
            postorder: &mut Vec<CfgNode>,
        ) {
            if visited.contains(&node) {
                return;
            }
            visited.insert(node);

            for (succ, _) in cfg.successors(node) {
                dfs(succ, cfg, visited, postorder);
            }
            postorder.push(node);
        }

        dfs(self.entry(), self, &mut visited, &mut postorder);
        postorder.reverse();
        postorder
    }

    pub fn reverse_postorder_from_exit(&self) -> Vec<CfgNode> {
        let mut visited = FxHashSet::default();
        let mut postorder = Vec::new();

        fn dfs(
            node: CfgNode,
            cfg: &ControlFlowGraph,
            visited: &mut FxHashSet<CfgNode>,
            postorder: &mut Vec<CfgNode>,
        ) {
            if visited.contains(&node) {
                return;
            }
            visited.insert(node);

            for (pred, _) in cfg.predecessors(node) {
                dfs(pred, cfg, visited, postorder);
            }
            postorder.push(node);
        }

        dfs(CfgNode::Exit, self, &mut visited, &mut postorder);
        postorder.reverse();
        postorder
    }
}

/// Post-dominator tree computed from the CFG
#[derive(Clone, Debug)]
pub struct PostDominatorTree {
    /// Immediate post-dominator for each node
    ipostdom: BTreeMap<CfgNode, Option<CfgNode>>,
    /// Post-dominator sets for each node
    postdom_sets: BTreeMap<CfgNode, BTreeSet<CfgNode>>,
    /// Children in the post-dominator tree
    children: BTreeMap<CfgNode, Vec<CfgNode>>,
}

impl PostDominatorTree {

    /// Compute post-dominator analysis for the given CFG
    pub fn compute(cfg: &ControlFlowGraph) -> Self {
        let all_nodes: BTreeSet<CfgNode> = cfg.nodes().iter().copied().collect();
        let mut postdom_sets: BTreeMap<CfgNode, BTreeSet<CfgNode>> = BTreeMap::new();

        postdom_sets.insert(CfgNode::Exit, {
            let mut s = BTreeSet::new();
            s.insert(CfgNode::Exit);
            s
        });

        // all other nodes start with full set
        for node in cfg.nodes() {
            if *node != CfgNode::Exit {
                postdom_sets.insert(*node, all_nodes.clone());
            }
        }

        let rpo = cfg.reverse_postorder_from_exit();
        let mut changed = true;

        while changed {
            changed = false;

            for &node in &rpo {
                if node == CfgNode::Exit {
                    continue;
                }

                // PDOM[n] = {n} union the intersection of PDOM[s] for each successor s.
                let mut new_postdom: Option<BTreeSet<CfgNode>> = None;

                for (succ, _) in cfg.successors(node) {
                    if let Some(succ_postdom) = postdom_sets.get(&succ) {
                        match &mut new_postdom {
                            None => new_postdom = Some(succ_postdom.clone()),

                            Some(current) => {
                                *current = current.intersection(succ_postdom).copied().collect();
                            }
                        }
                    }
                }

                let mut new_postdom = new_postdom.unwrap_or_default();
                new_postdom.insert(node);

                if postdom_sets.get(&node) != Some(&new_postdom) {
                    postdom_sets.insert(node, new_postdom);
                    changed = true;
                }
            }
        }

        let mut ipostdom: BTreeMap<CfgNode, Option<CfgNode>> = BTreeMap::new();
        ipostdom.insert(CfgNode::Exit, None);

        for node in cfg.nodes() {
            if *node == CfgNode::Exit {
                continue;
            }

            // ipdom(n), closest post-dominator, largest set excluding n
            if let Some(pdom_set) = postdom_sets.get(node) {
                let mut candidates: Vec<CfgNode> =
                    pdom_set.iter().copied().filter(|&p| p != *node).collect();

                // Sort by postdom set size
                candidates.sort_by_key(|&candidate| {
                    postdom_sets.get(&candidate).map(|s| s.len()).unwrap_or(usize::MAX)
                });

                // ipdom is candidate with smallest postdom set
                let ipdom = candidates.into_iter().next();
                ipostdom.insert(*node, ipdom);
            } else {
                ipostdom.insert(*node, None);
            }
        }

        let mut children: BTreeMap<CfgNode, Vec<CfgNode>> = BTreeMap::new();
        for node in cfg.nodes() {
            children.insert(*node, Vec::new());
        }

        for (&node, &ipdom) in &ipostdom {
            if let Some(parent) = ipdom {
                children.entry(parent).or_default().push(node);
            }
        }

        PostDominatorTree { ipostdom, postdom_sets, children }
    }

    pub fn immediate_postdominator(&self, node: CfgNode) -> Option<CfgNode> {
        self.ipostdom.get(&node).copied().flatten()
    }

    pub fn postdominators(&self, node: CfgNode) -> Option<&BTreeSet<CfgNode>> {
        self.postdom_sets.get(&node)
    }

    pub fn postdominates(&self, dominator: CfgNode, dominated: CfgNode) -> bool {
        self.postdom_sets.get(&dominated).map(|set| set.contains(&dominator)).unwrap_or(false)
    }

    pub fn children(&self, node: CfgNode) -> &[CfgNode] {
        self.children.get(&node).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn iter_ipostdom(&self) -> impl Iterator<Item = (CfgNode, Option<CfgNode>)> + '_ {
        self.ipostdom.iter().map(|(&n, &ipdom)| (n, ipdom))
    }
}

/// An edge in the Control Dependence Graph.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ControlDependenceEdge {
    /// The node that controls the dependent node
    pub controller: CfgNode,
    /// The node that is control-dependent
    pub dependent: CfgNode,
    /// Which controller edge leads to dependent
    pub label: EdgeKind,
}

/// Control Dependence Graph computed using Ferrante-Ottenstein-Warren algorithm
#[derive(Clone, Debug)]
pub struct ControlDependenceGraph {
    /// Dependent to (controller, edge label) list
    dependences: BTreeMap<CfgNode, Vec<(CfgNode, EdgeKind)>>,
    /// Reverse map, controller to (dependent, label)
    controlled_by: BTreeMap<CfgNode, Vec<(CfgNode, EdgeKind)>>,
    /// All edges
    edges: Vec<ControlDependenceEdge>,
}

impl ControlDependenceGraph {

    /// Build CDG (Ferrante-Ottenstein-Warren) walking post-dominator tree
    pub fn build(cfg: &ControlFlowGraph, pdt: &PostDominatorTree) -> Self {
        let mut dependences: BTreeMap<CfgNode, Vec<(CfgNode, EdgeKind)>> = BTreeMap::new();
        let mut controlled_by: BTreeMap<CfgNode, Vec<(CfgNode, EdgeKind)>> = BTreeMap::new();
        let mut edges: Vec<ControlDependenceEdge> = Vec::new();

        for node in cfg.nodes() {
            dependences.insert(*node, Vec::new());
            controlled_by.insert(*node, Vec::new());
        }

        for edge in cfg.edges() {
            let a = edge.from; // Source of CFG edge
            let b = edge.to; // Target of CFG edge

            // Skip if B post-dominates A
            if pdt.postdominates(b, a) {
                continue;
            }

            let ipdom_a = pdt.immediate_postdominator(a);

            let mut current = Some(b);
            while let Some(node) = current {
                // Stop when we reach ipdom(A)
                if Some(node) == ipdom_a {
                    break;
                }

                let cd_edge =
                    ControlDependenceEdge { controller: a, dependent: node, label: edge.kind };

                if !edges.contains(&cd_edge) {
                    edges.push(cd_edge);
                    dependences.entry(node).or_default().push((a, edge.kind));
                    controlled_by.entry(a).or_default().push((node, edge.kind));
                }

                // Move up in the post-dominator tree
                current = pdt.immediate_postdominator(node);
            }
        }

        ControlDependenceGraph { dependences, controlled_by, edges }
    }

    pub fn control_dependences(
        &self,
        node: CfgNode,
    ) -> impl Iterator<Item = (CfgNode, EdgeKind)> + '_ {
        self.dependences.get(&node).map(|v| v.iter().copied()).into_iter().flatten()
    }

    pub fn controlled_nodes(
        &self,
        controller: CfgNode,
    ) -> impl Iterator<Item = (CfgNode, EdgeKind)> + '_ {
        self.controlled_by.get(&controller).map(|v| v.iter().copied()).into_iter().flatten()
    }

    pub fn edges(&self) -> &[ControlDependenceEdge] {
        &self.edges
    }

    pub fn is_control_dependent(&self, dependent: CfgNode, controller: CfgNode) -> bool {
        self.dependences
            .get(&dependent)
            .map(|deps| deps.iter().any(|(c, _)| *c == controller))
            .unwrap_or(false)
    }
}

/// All control flow analyses for a function
pub struct ControlFlowAnalysis {
    pub cfg: ControlFlowGraph,
    pub post_dominator_tree: PostDominatorTree,
    pub cdg: ControlDependenceGraph,
}

impl ControlFlowAnalysis {

    pub fn analyze<'tcx>(body: &Body<'tcx>) -> Self {
        let cfg = ControlFlowGraph::build(body);
        let post_dominator_tree = PostDominatorTree::compute(&cfg);
        let cdg = ControlDependenceGraph::build(&cfg, &post_dominator_tree);

        ControlFlowAnalysis { cfg, post_dominator_tree, cdg }
    }
}

/// Debug printing utilities for CFG and CDG.
pub mod debug {
    use super::*;

    /// Print all CFG edges
    pub fn print_cfg(cfg: &ControlFlowGraph) {
        println!("=== Control Flow Graph ===");
        println!("Nodes: {:?}", cfg.nodes().iter().map(|n| n.to_string()).collect::<Vec<_>>());
        println!("\nEdges:");
        for edge in cfg.edges() {
            println!("  {} -> {} [{:?}]", edge.from, edge.to, edge.kind);
        }
        println!();
    }

    /// Print CFG as adjacency list
    pub fn print_cfg_adjacency(cfg: &ControlFlowGraph) {
        println!("=== CFG Adjacency List ===");
        for node in cfg.nodes() {
            let succs: Vec<String> =
                cfg.successors(*node).map(|(s, k)| format!("{}({:?})", s, k)).collect();
            println!("  {} -> [{}]", node, succs.join(", "));
        }
        println!();
    }

    /// Print post-dominator tree and sets.
    pub fn print_post_dominators(pdt: &PostDominatorTree) {
        println!("=== Post-Dominator Analysis ===");
        println!("\nImmediate Post-Dominators:");
        for (node, ipdom) in pdt.iter_ipostdom() {
            let ipdom_str = ipdom.map(|n| n.to_string()).unwrap_or_else(|| "None".to_string());
            println!("  ipdom({}) = {}", node, ipdom_str);
        }

        println!("\nPost-Dominator Tree (parent -> children):");
        for (node, _) in pdt.iter_ipostdom() {
            let children = pdt.children(node);
            if !children.is_empty() {
                let children_str: Vec<String> = children.iter().map(|n| n.to_string()).collect();
                println!("  {} -> [{}]", node, children_str.join(", "));
            }
        }
        println!();
    }

    /// Print all CDG edges
    pub fn print_cdg(cdg: &ControlDependenceGraph) {
        println!("=== Control Dependence Graph ===");
        println!("(dependent is control-dependent on controller)\n");
        for edge in cdg.edges() {
            println!(
                "  {} --[{:?}]--> {} is control-dependent on {}",
                edge.controller, edge.label, edge.dependent, edge.controller
            );
        }
        println!();
    }

    /// Print each node's controllers
    pub fn print_cdg_dependences(cdg: &ControlDependenceGraph, cfg: &ControlFlowGraph) {
        println!("=== Control Dependences (node -> controllers) ===");
        for node in cfg.nodes() {
            let deps: Vec<String> =
                cdg.control_dependences(*node).map(|(c, k)| format!("{}({:?})", c, k)).collect();
            if !deps.is_empty() {
                println!("  {} depends on: [{}]", node, deps.join(", "));
            }
        }
        println!();
    }

    /// Print summary of all analyses
    pub fn print_analysis(analysis: &ControlFlowAnalysis) {
        print_cfg(&analysis.cfg);
        print_cfg_adjacency(&analysis.cfg);
        print_post_dominators(&analysis.post_dominator_tree);
        print_cdg(&analysis.cdg);
        print_cdg_dependences(&analysis.cdg, &analysis.cfg);
    }

    /// Format CFG edges as a string.
    pub fn cfg_to_string(cfg: &ControlFlowGraph) -> String {
        let mut result = String::new();
        result.push_str("CFG Edges:\n");
        for edge in cfg.edges() {
            result.push_str(&format!("  {} -> {} [{:?}]\n", edge.from, edge.to, edge.kind));
        }
        result
    }

    /// Format CDG edges as a string.
    pub fn cdg_to_string(cdg: &ControlDependenceGraph) -> String {
        let mut result = String::new();
        result.push_str("CDG Edges (dependent <- controller):\n");
        for edge in cdg.edges() {
            result.push_str(&format!(
                "  {} <- {} [{:?}]\n",
                edge.dependent, edge.controller, edge.label
            ));
        }
        result
    }

    /// CFG in Graphviz DOT format
    pub fn cfg_to_dot(cfg: &ControlFlowGraph) -> String {
        let mut dot = String::new();
        dot.push_str("digraph CFG {\n");
        dot.push_str("    // CHANGED BY JAYITHI GAVVA: Generated CFG visualization\n");
        dot.push_str("    rankdir=TB;\n");
        dot.push_str("    node [shape=box, style=filled];\n");
        dot.push_str("\n");

        dot.push_str("    // Node styling\n");
        dot.push_str("    bb0 [label=\"bb0\\n(ENTRY)\", fillcolor=lightgreen];\n");
        dot.push_str("    EXIT [label=\"EXIT\", fillcolor=lightcoral, shape=ellipse];\n");
        dot.push_str("\n");

        dot.push_str("    // Edges\n");
        for edge in cfg.edges() {
            let from_str = match edge.from {
                CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                CfgNode::Exit => "EXIT".to_string(),
            };
            let to_str = match edge.to {
                CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                CfgNode::Exit => "EXIT".to_string(),
            };

            let (color, label) = match edge.kind {
                EdgeKind::Goto => ("black", "goto"),
                EdgeKind::SwitchInt(Some(_v)) => ("blue", "switch"),
                EdgeKind::SwitchInt(None) => ("blue", "otherwise"),
                EdgeKind::AssertSuccess => ("green", "assert_ok"),
                EdgeKind::AssertUnwind => ("red", "assert_fail"),
                EdgeKind::Return => ("darkgreen", "return"),
                EdgeKind::CallReturn => ("purple", "call_ret"),
                EdgeKind::CallUnwind => ("red", "call_unwind"),
                EdgeKind::Drop => ("orange", "drop"),
                EdgeKind::Yield => ("cyan", "yield"),
                EdgeKind::InlineAsm => ("gray", "asm"),
                EdgeKind::FalseEdge => ("lightgray", "false_edge"),
                EdgeKind::FalseUnwind => ("lightgray", "false_unwind"),
                EdgeKind::Other => ("black", "other"),
            };

            dot.push_str(&format!(
                "    {} -> {} [label=\"{}\", color={}];\n",
                from_str, to_str, label, color
            ));
        }

        dot.push_str("}\n");
        dot
    }

    /// Generate Post-Dominator Tree in Graphviz DOT format.
    pub fn pdt_to_dot(pdt: &PostDominatorTree, _cfg: &ControlFlowGraph) -> String {
        let mut dot = String::new();
        dot.push_str("digraph PostDominatorTree {\n");
        dot.push_str(
            "    // CHANGED BY JAYITHI GAVVA: Generated Post-Dominator Tree visualization\n",
        );
        dot.push_str("    rankdir=BT;  // Bottom to top (EXIT at top)\n");
        dot.push_str("    node [shape=box, style=filled, fillcolor=lightyellow];\n");
        dot.push_str("\n");

        dot.push_str("    EXIT [label=\"EXIT\\n(root)\", fillcolor=lightcoral, shape=ellipse];\n");
        dot.push_str("\n");

        dot.push_str("    // Edges (node -> immediate post-dominator)\n");
        for (node, ipdom) in pdt.iter_ipostdom() {
            if let Some(parent) = ipdom {
                let node_str = match node {
                    CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                    CfgNode::Exit => "EXIT".to_string(),
                };
                let parent_str = match parent {
                    CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                    CfgNode::Exit => "EXIT".to_string(),
                };
                dot.push_str(&format!("    {} -> {};\n", node_str, parent_str));
            }
        }

        dot.push_str("}\n");
        dot
    }

    /// CDG in Graphviz DOT format
    pub fn cdg_to_dot(cdg: &ControlDependenceGraph, _cfg: &ControlFlowGraph) -> String {
        let mut dot = String::new();
        dot.push_str("digraph CDG {\n");
        dot.push_str(
            "    // CHANGED BY JAYITHI GAVVA: Generated Control Dependence Graph visualization\n",
        );
        dot.push_str("    rankdir=TB;\n");
        dot.push_str("    node [shape=box, style=filled, fillcolor=lightblue];\n");
        dot.push_str("\n");

        dot.push_str("    bb0 [label=\"bb0\\n(ENTRY)\", fillcolor=lightgreen];\n");
        dot.push_str("    EXIT [label=\"EXIT\", fillcolor=lightcoral, shape=ellipse];\n");
        dot.push_str("\n");

        dot.push_str("    // Control Dependence Edges (controller -> dependent)\n");
        for edge in cdg.edges() {
            let controller_str = match edge.controller {
                CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                CfgNode::Exit => "EXIT".to_string(),
            };
            let dependent_str = match edge.dependent {
                CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                CfgNode::Exit => "EXIT".to_string(),
            };

            let label = match edge.label {
                EdgeKind::SwitchInt(Some(v)) => format!("val={}", v),
                EdgeKind::SwitchInt(None) => "otherwise".to_string(),
                EdgeKind::AssertSuccess => "true".to_string(),
                EdgeKind::AssertUnwind => "false".to_string(),
                _ => format!("{:?}", edge.label),
            };

            dot.push_str(&format!(
                "    {} -> {} [label=\"{}\", style=dashed, color=red];\n",
                controller_str, dependent_str, label
            ));
        }

        dot.push_str("}\n");
        dot
    }

    /// Write cfg.dot, pdt.dot, cdg.dot to directory
    pub fn save_all_graphs_to_dot(
        analysis: &ControlFlowAnalysis,
        output_dir: &std::path::Path,
    ) -> std::io::Result<()> {
        use std::fs;
        use std::io::Write;

        fs::create_dir_all(output_dir)?;

        let cfg_path = output_dir.join("cfg.dot");
        let mut cfg_file = fs::File::create(&cfg_path)?;
        cfg_file.write_all(cfg_to_dot(&analysis.cfg).as_bytes())?;
        println!("Wrote CFG to: {}", cfg_path.display());

        let pdt_path = output_dir.join("pdt.dot");
        let mut pdt_file = fs::File::create(&pdt_path)?;
        pdt_file.write_all(pdt_to_dot(&analysis.post_dominator_tree, &analysis.cfg).as_bytes())?;
        println!("Wrote Post-Dominator Tree to: {}", pdt_path.display());

        let cdg_path = output_dir.join("cdg.dot");
        let mut cdg_file = fs::File::create(&cdg_path)?;
        cdg_file.write_all(cdg_to_dot(&analysis.cdg, &analysis.cfg).as_bytes())?;
        println!("Wrote CDG to: {}", cdg_path.display());

        Ok(())
    }

    /// Generate a graph with CFG edges and control-dependence edges.
    pub fn combined_cfg_cdg_to_dot(cfg: &ControlFlowGraph, cdg: &ControlDependenceGraph) -> String {
        let mut dot = String::new();
        dot.push_str("digraph Combined_CFG_CDG {\n");
        dot.push_str("    // CHANGED BY JAYITHI GAVVA: Combined CFG and CDG visualization\n");
        dot.push_str("    rankdir=TB;\n");
        dot.push_str("    node [shape=box, style=filled, fillcolor=lightyellow];\n");
        dot.push_str("\n");

        dot.push_str("    bb0 [label=\"bb0\\n(ENTRY)\", fillcolor=lightgreen];\n");
        dot.push_str("    EXIT [label=\"EXIT\", fillcolor=lightcoral, shape=ellipse];\n");
        dot.push_str("\n");

        dot.push_str("    // CFG Edges (solid)\n");
        for edge in cfg.edges() {
            let from_str = match edge.from {
                CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                CfgNode::Exit => "EXIT".to_string(),
            };
            let to_str = match edge.to {
                CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                CfgNode::Exit => "EXIT".to_string(),
            };
            dot.push_str(&format!("    {} -> {} [color=black];\n", from_str, to_str));
        }

        dot.push_str("\n");

        dot.push_str("    // CDG Edges (dashed red)\n");
        for edge in cdg.edges() {
            let controller_str = match edge.controller {
                CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                CfgNode::Exit => "EXIT".to_string(),
            };
            let dependent_str = match edge.dependent {
                CfgNode::Block(bb) => format!("bb{}", bb.as_usize()),
                CfgNode::Exit => "EXIT".to_string(),
            };
            dot.push_str(&format!(
                "    {} -> {} [style=dashed, color=red, constraint=false];\n",
                controller_str, dependent_str
            ));
        }

        dot.push_str("}\n");
        dot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cfg_node_display() {
        let bb0 = CfgNode::Block(BasicBlock::from_usize(0));
        assert_eq!(bb0.to_string(), "bb0");
        assert_eq!(CfgNode::Exit.to_string(), "EXIT");
    }

    #[test]
    fn test_cfg_node_is_exit() {
        assert!(!CfgNode::Block(BasicBlock::from_usize(0)).is_exit());
        assert!(CfgNode::Exit.is_exit());
    }

    #[test]
    fn test_cfg_node_as_block() {
        let bb = BasicBlock::from_usize(5);
        assert_eq!(CfgNode::Block(bb).as_block(), Some(bb));
        assert_eq!(CfgNode::Exit.as_block(), None);
    }
}
