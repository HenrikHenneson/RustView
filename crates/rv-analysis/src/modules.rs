//! The call graph aggregated to modules.

use std::collections::HashMap;

use rv_ir::{FnId, ModuleId, Project};

use crate::program::{CallGraph, Facts};

#[derive(Clone, Debug)]
pub struct ModuleGraph {
    pub nodes: Vec<ModuleNode>,
    pub edges: Vec<ModuleEdge>,
}

#[derive(Clone, Debug)]
pub struct ModuleNode {
    /// Module path cut to the grouping depth, e.g. `mtd::ec`.
    pub key: String,
    pub modules: Vec<ModuleId>,
    pub functions: Vec<FnId>,
    pub lines: u32,
    /// Calls between functions of this node.
    pub internal_calls: usize,
    /// Places that start threads or tasks.
    pub spawns: usize,
}

#[derive(Clone, Debug)]
pub struct ModuleEdge {
    pub from: usize,
    pub to: usize,
    /// (caller, callee) pairs, one per call site.
    pub calls: Vec<(FnId, FnId)>,
}

/// Crate names (first path segment of the modules), in project order.
pub fn crates(project: &Project) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in &project.modules {
        if let Some(c) = m.path.first()
            && !out.contains(c)
        {
            out.push(c.clone());
        }
    }
    out
}

/// Modules of `crate_name` grouped to `depth` path segments (the crate
/// itself counts as one), with the calls between the groups.
pub fn module_graph(
    project: &Project,
    graph: &CallGraph,
    facts: &Facts,
    crate_name: &str,
    depth: usize,
) -> ModuleGraph {
    let depth = depth.max(1);
    let mut nodes: Vec<ModuleNode> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut node_of_module: HashMap<ModuleId, usize> = HashMap::new();
    for (i, m) in project.modules.iter().enumerate() {
        if m.path.first().map(String::as_str) != Some(crate_name) {
            continue;
        }
        let key = m.path[..m.path.len().min(depth)].join("::");
        let n = *index.entry(key.clone()).or_insert_with(|| {
            nodes.push(ModuleNode {
                key,
                modules: Vec::new(),
                functions: Vec::new(),
                lines: 0,
                internal_calls: 0,
                spawns: 0,
            });
            nodes.len() - 1
        });
        let id = ModuleId(i as u32);
        nodes[n].modules.push(id);
        node_of_module.insert(id, n);
        for &f in &m.functions {
            let func = project.function(f);
            nodes[n].functions.push(f);
            nodes[n].lines += func.span.line_count();
            if let Some(c) = facts.concurrency(f) {
                nodes[n].spawns += c.spawns.iter().filter(|s| s.kind.is_parallel()).count();
            }
        }
    }
    let node_of_fn = |f: FnId| node_of_module.get(&project.function(f).module).copied();
    let mut edge_index: HashMap<(usize, usize), usize> = HashMap::new();
    let mut edges: Vec<ModuleEdge> = Vec::new();
    for e in &graph.edges {
        let (Some(a), Some(b)) = (node_of_fn(e.caller), node_of_fn(e.callee)) else {
            continue;
        };
        if a == b {
            nodes[a].internal_calls += 1;
            continue;
        }
        let i = *edge_index.entry((a, b)).or_insert_with(|| {
            edges.push(ModuleEdge {
                from: a,
                to: b,
                calls: Vec::new(),
            });
            edges.len() - 1
        });
        edges[i].calls.push((e.caller, e.callee));
    }
    ModuleGraph { nodes, edges }
}
