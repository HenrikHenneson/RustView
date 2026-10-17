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
    if std::env::var("THREADS").is_ok() {
        for f in &p.functions {
            let Some(body) = &f.body else { continue };
            let flow = rv_analysis::data_flow(body);
            let c = rv_analysis::concurrency(body, &flow, &p.type_aliases);
            if c.is_empty() {
                continue;
            }
            println!("== {} (line {})", f.qual_path, f.span.start.line);
            for s in &c.spawns {
                let many = if s.in_loop { " ×n" } else { "" };
                println!(
                    "   {}{many} {:?} via {}: runs {:?}",
                    s.kind.label(),
                    s.name,
                    s.api,
                    s.runs.iter().take(3).collect::<Vec<_>>()
                );
            }
            for ch in &c.channels {
                println!("   channel {} ({})", ch.name, ch.api);
            }
            for u in &c.channel_uses {
                println!(
                    "     {:?} {:?} at line {}",
                    u.end,
                    u.op,
                    body.node(u.node).span.start.line
                );
            }
            for g in &c.shared {
                let n = c
                    .shared_uses
                    .iter()
                    .filter(|u| c.shared[u.group].name == g.name)
                    .count();
                println!(
                    "   shared {} ({}) {} bindings, {n} uses",
                    g.name,
                    g.kind,
                    g.bindings.len()
                );
            }
        }
    }
    if std::env::var("PROGRAM").is_ok() {
        let t0 = std::time::Instant::now();
        let facts = rv_analysis::Facts::new(&p);
        let graph = rv_analysis::CallGraph::new(&p);
        println!(
            "facts + call graph ({} edges) in {:?}",
            graph.edges.len(),
            t0.elapsed()
        );
        for entry in rv_analysis::entry_points(&p) {
            let prog = rv_analysis::program(&p, &facts, &graph, entry);
            println!(
                "#### program {} ({} threads)",
                p.function(entry).qual_path,
                prog.threads.len()
            );
            for (i, t) in prog.threads.iter().enumerate() {
                let entries: Vec<&str> = t
                    .entries
                    .iter()
                    .take(5)
                    .map(|&f| p.function(f).name.as_str())
                    .collect();
                println!(
                    "  [{i}] {} spawned_by {:?} entries {:?} ({} fns reached)",
                    t.title(),
                    t.spawned_by,
                    entries,
                    t.runs.len()
                );
            }
            for l in &prog.links {
                if let rv_analysis::LinkKind::Channel { name, .. } = &l.kind {
                    println!("  channel {name}: [{}] -> [{}]", l.from, l.to);
                }
            }
            for r in &prog.resources {
                println!("  shared {} ({}) users {:?}", r.name, r.kind, r.users);
            }
            let site = |x: &rv_analysis::Site| {
                let line = p
                    .function(x.func)
                    .body
                    .as_ref()
                    .map_or(0, |b| b.node(x.node).span.start.line);
                format!(
                    "[{}] {}:{} {}",
                    x.thread,
                    p.function(x.func).name,
                    line,
                    x.op.clone().unwrap_or_else(|| "-".into())
                )
            };
            for c in &prog.channels {
                println!(
                    "  CHANNEL {} msg={:?} cap={:?}",
                    c.name, c.message, c.capacity
                );
                for x in &c.senders {
                    println!("      send  {}", site(x));
                }
                for x in &c.receivers {
                    println!("      recv  {}", site(x));
                }
            }
            for sh in &prog.shared {
                let ops: Vec<String> = sh.users.iter().map(&site).collect();
                println!("  SHARED {} ({}): {}", sh.name, sh.kind, ops.join(" | "));
            }
            for j in &prog.joins {
                let w: Vec<String> = j.waiters.iter().map(&site).collect();
                println!("  JOIN [{}] waited for by {}", j.thread, w.join(", "));
            }
            for (f, _) in &prog.unreached {
                println!("  unreached spawn in {}", p.function(*f).qual_path);
            }
        }
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
