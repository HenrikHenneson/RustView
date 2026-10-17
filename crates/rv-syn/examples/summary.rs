//! Print what the loader finds: `cargo run -p rv-syn --example summary -- <path>`

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let t0 = std::time::Instant::now();
    let p = rv_syn::load(path.as_ref())?;
    println!(
        "loaded in {:?}: {} files, {} modules, {} functions",
        t0.elapsed(),
        p.files.len(),
        p.modules.len(),
        p.functions.len()
    );
    for pkg in &p.packages {
        let ws = if pkg.in_workspace {
            ""
        } else {
            " (outside workspace)"
        };
        println!("package {}{ws}  tests: {:?}", pkg.name, pkg.tests);
        for t in &pkg.targets {
            let fns = t.root_module.map_or(0, |m| count(&p, m));
            println!("  {} {} -> {fns} fns", t.kind.label(), t.name);
        }
        for f in &pkg.unlinked_files {
            println!("  unlinked: {}", p.file(*f).path.display());
        }
    }
    let nodes: usize = p
        .functions
        .iter()
        .filter_map(|f| f.body.as_ref())
        .map(|b| b.nodes.len())
        .sum();
    if let Some(big) = p
        .functions
        .iter()
        .max_by_key(|f| f.body.as_ref().map_or(0, |b| b.nodes.len()))
    {
        let b = big.body.as_ref().unwrap();
        let depth = (0..b.nodes.len())
            .map(|i| depth(b, rv_ir::NodeId(i as u32)))
            .max()
            .unwrap_or(0);
        println!(
            "body nodes total: {nodes}; largest: {} with {} nodes, {} stmts, {} exits, depth {depth}",
            big.qual_path,
            b.nodes.len(),
            b.stmt_count(b.root),
            b.exits.len()
        );
    }
    if let Ok(name) = std::env::var("DUMP")
        && let Some(f) = p.functions.iter().find(|f| f.qual_path.ends_with(&name))
    {
        print!("{}", f.body.as_ref().unwrap().dump());
    }
    for d in &p.diagnostics {
        println!("{:?}: {} {:?}", d.severity, d.message, d.file);
    }
    Ok(())
}

fn count(p: &rv_ir::Project, m: rv_ir::ModuleId) -> usize {
    let module = p.module(m);
    module.functions.len() + module.children.iter().map(|&c| count(p, c)).sum::<usize>()
}

fn depth(b: &rv_ir::Body, mut n: rv_ir::NodeId) -> usize {
    let mut d = 0;
    while let Some(p) = b.node(n).parent {
        if matches!(b.node(p).kind, rv_ir::NodeKind::Structure { .. }) {
            d += 1;
        }
        n = p;
    }
    d
}
