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
    for d in &p.diagnostics {
        println!("{:?}: {} {:?}", d.severity, d.message, d.file);
    }
    Ok(())
}

fn count(p: &rv_ir::Project, m: rv_ir::ModuleId) -> usize {
    let module = p.module(m);
    module.functions.len() + module.children.iter().map(|&c| count(p, c)).sum::<usize>()
}
