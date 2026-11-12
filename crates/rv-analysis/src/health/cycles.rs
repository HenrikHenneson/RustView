//! Modules that call each other in a cycle. Such modules cannot be
//! understood, tested or moved one at a time.

use std::collections::BTreeMap;

use rv_ir::{FnId, ModuleId, NodeId, Project};

use crate::CallGraph;

/// A group of modules in which every module reaches every other through
/// calls (a strongly connected component of the module call graph).
#[derive(Clone, Debug)]
pub struct Cycle {
    /// In project order.
    pub modules: Vec<ModuleId>,
    /// The calls between them, the link with the fewest call sites first:
    /// that one is usually the cheapest to remove.
    pub links: Vec<CycleLink>,
}

#[derive(Clone, Debug)]
pub struct CycleLink {
    pub from: ModuleId,
    pub to: ModuleId,
    /// Call sites (calling function, statement).
    pub calls: Vec<(FnId, NodeId)>,
}

/// Call sites between two different modules.
type Links = BTreeMap<(ModuleId, ModuleId), Vec<(FnId, NodeId)>>;

pub(super) fn find(project: &Project, calls: &CallGraph) -> Vec<Cycle> {
    let mut links = Links::new();
    for e in &calls.edges {
        let from = project.function(e.caller).module;
        let to = project.function(e.callee).module;
        if from != to {
            links
                .entry((from, to))
                .or_default()
                .push((e.caller, e.node));
        }
    }
    let mut successors = vec![Vec::new(); project.modules.len()];
    for &(from, to) in links.keys() {
        successors[from.index()].push(to.index());
    }
    let mut cycles: Vec<Cycle> = Components::new(&successors)
        .run()
        .into_iter()
        .filter(|c| c.len() > 1)
        .map(|c| cycle(c, &links))
        .collect();
    cycles.sort_by_key(|c| (std::cmp::Reverse(c.modules.len()), c.modules[0]));
    cycles
}

fn cycle(mut members: Vec<usize>, links: &Links) -> Cycle {
    members.sort_unstable();
    let modules: Vec<ModuleId> = members.iter().map(|&m| ModuleId(m as u32)).collect();
    let mut inside: Vec<CycleLink> = links
        .iter()
        .filter(|((from, to), _)| modules.contains(from) && modules.contains(to))
        .map(|(&(from, to), calls)| CycleLink {
            from,
            to,
            calls: calls.clone(),
        })
        .collect();
    inside.sort_by_key(|l| (l.calls.len(), l.from, l.to));
    Cycle {
        modules,
        links: inside,
    }
}

/// Strongly connected components (Tarjan's algorithm).
struct Components<'g> {
    successors: &'g [Vec<usize>],
    /// Order of discovery; `None` = not visited yet.
    index: Vec<Option<usize>>,
    /// Smallest index reachable from the node's subtree.
    low: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    next: usize,
    found: Vec<Vec<usize>>,
}

impl<'g> Components<'g> {
    fn new(successors: &'g [Vec<usize>]) -> Self {
        let n = successors.len();
        Components {
            successors,
            index: vec![None; n],
            low: vec![0; n],
            on_stack: vec![false; n],
            stack: Vec::new(),
            next: 0,
            found: Vec::new(),
        }
    }

    fn run(mut self) -> Vec<Vec<usize>> {
        for v in 0..self.successors.len() {
            if self.index[v].is_none() {
                self.visit(v);
            }
        }
        self.found
    }

    fn visit(&mut self, v: usize) {
        self.index[v] = Some(self.next);
        self.low[v] = self.next;
        self.next += 1;
        self.stack.push(v);
        self.on_stack[v] = true;
        let successors = self.successors;
        for &w in &successors[v] {
            match self.index[w] {
                None => {
                    self.visit(w);
                    self.low[v] = self.low[v].min(self.low[w]);
                }
                Some(i) if self.on_stack[w] => self.low[v] = self.low[v].min(i),
                Some(_) => {}
            }
        }
        if self.index[v] == Some(self.low[v]) {
            let mut component = Vec::new();
            while let Some(w) = self.stack.pop() {
                self.on_stack[w] = false;
                component.push(w);
                if w == v {
                    break;
                }
            }
            self.found.push(component);
        }
    }
}
