//! The syntactic call graph: which call site calls which project function.

use std::collections::HashMap;

use rv_ir::{FnId, NodeId, Project};

use super::resolver::Resolver;

#[derive(Clone, Debug)]
pub struct CallEdge {
    pub caller: FnId,
    pub node: NodeId,
    /// Index into the node's `calls`.
    pub call: usize,
    pub callee: FnId,
}

pub struct CallGraph {
    pub edges: Vec<CallEdge>,
    by_caller: Vec<Vec<usize>>,
    by_site: HashMap<(FnId, NodeId, usize), FnId>,
}

impl CallGraph {
    pub fn new(project: &Project) -> Self {
        let r = Resolver::new(project);
        let mut edges = Vec::new();
        let mut by_caller = vec![Vec::new(); project.functions.len()];
        let mut by_site = HashMap::new();
        for (i, f) in project.functions.iter().enumerate() {
            let caller = FnId(i as u32);
            let Some(body) = &f.body else { continue };
            for (n, node) in body.nodes.iter().enumerate() {
                for (ci, call) in node.calls.iter().enumerate() {
                    if let Some(callee) = r.resolve(caller, call) {
                        let node = NodeId(n as u32);
                        by_caller[i].push(edges.len());
                        by_site.insert((caller, node, ci), callee);
                        edges.push(CallEdge {
                            caller,
                            node,
                            call: ci,
                            callee,
                        });
                    }
                }
            }
        }
        CallGraph {
            edges,
            by_caller,
            by_site,
        }
    }

    pub fn calls_from(&self, f: FnId) -> impl Iterator<Item = &CallEdge> {
        self.by_caller[f.index()].iter().map(|&i| &self.edges[i])
    }

    pub fn callee(&self, f: FnId, node: NodeId, call: usize) -> Option<FnId> {
        self.by_site.get(&(f, node, call)).copied()
    }
}
