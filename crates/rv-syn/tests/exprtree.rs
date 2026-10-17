//! Expression trees of statements (expression level).

fn trees(src: &str) -> Vec<String> {
    let p = rv_syn::load_str(src);
    let f = p.functions.iter().find(|f| f.name == "f").unwrap();
    let body = f.body.as_ref().unwrap();
    let frame = body.node(body.root).children[0];
    body.node(frame)
        .children
        .iter()
        .filter_map(|&c| body.node(c).expr.as_ref().map(|t| t.sexpr()))
        .collect()
}

#[test]
fn operands_become_children() {
    let got = trees(
        r#"
fn f(a: u32, v: &mut Vec<u32>) -> u32 {
    let x = a * 2 + offset(a);
    v.push(x as u32);
    total += v[0];
    log::info!("x = {}", x);
    let s = Settings { scale: x, ..Default::default() };
    data.lock()?.len()
}"#,
    );
    assert_eq!(
        got,
        [
            "(let x (+ (* a 2) (offset() a)))",
            "(.push() v (as u32 x))",
            "(+= total ([ ] v 0))",
            r#"(info! "x = {}" x)"#,
            "(let s (Settings {} x))",
            "(.len() (? (.lock() data)))",
        ]
    );
}

#[test]
fn structures_are_referenced() {
    let p = rv_syn::load_str(
        r#"
fn f() {
    let h = spawn(move || { work(); });
    let n = compute(match k { 0 => 1, _ => 2 });
}"#,
    );
    let body = p.functions[0].body.as_ref().unwrap();
    let frame = body.node(body.root).children[0];
    let first = body.node(body.node(frame).children[0]);
    let tree = first.expr.as_ref().unwrap();
    assert_eq!(tree.sexpr(), "(let h (spawn() move ||))");
    // The closure leaf points at the closure structure node.
    let leaf = tree
        .nodes
        .iter()
        .find(|n| matches!(n.kind, rv_ir::ExprKind::Structure(_)))
        .unwrap();
    let rv_ir::ExprKind::Structure(id) = leaf.kind else {
        unreachable!()
    };
    assert!(matches!(
        body.node(id).kind,
        rv_ir::NodeKind::Structure { .. }
    ));
    let second = body.node(body.node(frame).children[1]);
    assert_eq!(
        second.expr.as_ref().unwrap().sexpr(),
        "(let n (compute() match k))"
    );
}
