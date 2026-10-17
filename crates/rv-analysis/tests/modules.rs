//! Call graph aggregated to modules.

use rv_analysis::{CallGraph, Facts};

const SRC: &str = r#"
fn main() {
    a::f();
    a::inner::k();
}

mod a {
    pub fn f() {
        crate::b::g();
        std::thread::spawn(|| crate::b::h());
    }
    pub mod inner {
        pub fn k() { super::f(); }
    }
}

mod b {
    pub fn g() { h(); }
    pub fn h() {}
}
"#;

#[test]
fn modules_and_calls_between_them() {
    let p = rv_syn::load_str(SRC);
    let facts = Facts::new(&p);
    let graph = CallGraph::new(&p);
    assert_eq!(rv_analysis::crates(&p), ["snippet"]);

    let g = rv_analysis::module_graph(&p, &graph, &facts, "snippet", 2);
    let keys: Vec<&str> = g.nodes.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(keys, ["snippet", "snippet::a", "snippet::b"]);
    let mut edges: Vec<(String, String, usize)> = g
        .edges
        .iter()
        .map(|e| {
            (
                g.nodes[e.from].key.clone(),
                g.nodes[e.to].key.clone(),
                e.calls.len(),
            )
        })
        .collect();
    edges.sort();
    assert_eq!(
        edges,
        [
            ("snippet".into(), "snippet::a".into(), 2),
            ("snippet::a".into(), "snippet::b".into(), 2),
        ]
    );
    let a = &g.nodes[1];
    assert_eq!(a.functions.len(), 2, "f and inner::k at depth 2");
    assert_eq!(a.internal_calls, 1, "inner::k -> f");
    assert_eq!(a.spawns, 1);
    assert_eq!(g.nodes[2].internal_calls, 1, "g -> h");

    // Depth 3 separates `a::inner`.
    let g3 = rv_analysis::module_graph(&p, &graph, &facts, "snippet", 3);
    assert_eq!(g3.nodes.len(), 4);
}
