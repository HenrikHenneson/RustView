//! Single questions about a project, as the MCP server asks them.

use rv_analysis::{CallGraph, Facts, query};

const SRC: &str = r#"
fn main() {
    let (tx, rx) = std::sync::mpsc::channel::<u32>();
    std::thread::Builder::new()
        .name("zaehler".into())
        .spawn(move || zaehlen(tx))
        .unwrap();
    for n in rx.iter() {
        if n > 3 {
            break;
        }
        melden(n);
    }
}

fn zaehlen(tx: Sender<u32>) {
    let mut i = 0;
    loop {
        if tx.send(i).is_err() {
            return;
        }
        melden(i);
        i += 1;
    }
}

fn melden(n: u32) {}

struct Teil { menge: u32 }
impl Teil { fn melden(&self) {} }
"#;

struct Setup {
    p: rv_ir::Project,
    facts: Facts,
    calls: CallGraph,
    programs: Vec<(rv_ir::FnId, rv_analysis::Program)>,
}

fn setup() -> Setup {
    let p = rv_syn::load_str(SRC);
    let facts = Facts::new(&p);
    let calls = CallGraph::new(&p);
    let programs = rv_analysis::programs(&p, &facts, &calls);
    Setup {
        p,
        facts,
        calls,
        programs,
    }
}

#[test]
fn functions_resolve_by_suffix_and_ambiguity_lists_candidates() {
    let s = setup();
    let f = query::resolve_function(&s.p, "zaehlen").unwrap();
    assert_eq!(s.p.function(f).qual_path, "snippet::zaehlen");
    let err = query::resolve_function(&s.p, "melden").unwrap_err();
    assert!(err.contains("ambiguous (2 functions)"), "{err}");
    assert!(err.contains("`snippet::Teil::melden`"), "{err}");
    assert!(query::resolve_function(&s.p, "snippet::melden").is_ok());
    assert!(query::resolve_function(&s.p, "fehlt").is_err());
}

#[test]
fn callers_threads_and_find() {
    let s = setup();
    let melden = query::resolve_function(&s.p, "snippet::melden").unwrap();
    let callers = query::callers(&s.p, &s.calls, melden);
    assert!(
        callers.contains("`snippet::main` (snippet.rs:2) — line 12"),
        "{callers}"
    );
    assert!(
        callers.contains("`snippet::zaehlen` (snippet.rs:16) — line 22"),
        "{callers}"
    );

    let threads = query::threads_of(&s.p, &s.programs, melden);
    assert!(
        threads.contains("#0 main thread (reached through calls)"),
        "{threads}"
    );
    assert!(
        threads.contains("#1 thread 'zaehler' (reached through calls)"),
        "{threads}"
    );

    let found = query::find(&s.p, &s.programs, "zaehler");
    assert!(
        found.contains("Thread #1 'zaehler' of `snippet::main`, started at snippet.rs:4"),
        "{found}"
    );
    let found = query::find(&s.p, &s.programs, "tx");
    assert!(found.contains("Channel `tx → rx`"), "{found}");
    assert!(found.contains("senders: #1; receivers: #0"), "{found}");
    let found = query::find(&s.p, &s.programs, "Teil");
    assert!(
        found.contains("Struct `Teil` fields: menge: u32"),
        "{found}"
    );
    assert!(found.contains("`snippet::Teil::melden`"), "{found}");
}

#[test]
fn outline_shows_structures_thread_starts_and_exits() {
    let s = setup();
    let main = query::resolve_function(&s.p, "main").unwrap();
    let text = query::outline(&s.p, &s.facts, main, false);
    let lines: Vec<&str> = text.lines().skip(1).collect();
    assert_eq!(
        lines,
        [
            "4-7 std::thread::Builder::new().name(\"zaehler\".into()).spawn(move || zaehlen(tx)).unwrap()  [‖ starts thread 'zaehler']",
            "8-13 for n in rx.iter()",
            "  9-11 if n > 3",
            "    10 break  [→ break]",
        ],
        "{text}"
    );
}
