//! The Markdown project report and the call resolution it relies on.

use rv_analysis::CallGraph;

const SRC: &str = r#"
fn main() {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Messung>(4);
    let stand = Arc::new(Mutex::new(0));
    let s = stand.clone();
    std::thread::Builder::new()
        .name("messen".into())
        .spawn(move || messen(tx, s))
        .unwrap();
    while let Ok(m) = rx.recv() {
        *stand.lock().unwrap() += 1;
    }
}

fn messen(tx: SyncSender<Messung>, stand: Arc<Mutex<u32>>) {
    loop {
        if *stand.lock().unwrap() > 3 {
            break;
        }
        tx.send(Messung).unwrap();
    }
}

struct Messung;
"#;

#[test]
fn report_lists_threads_channels_and_shared_state() {
    let p = rv_syn::load_str(SRC);
    let text = rv_analysis::report(&p);
    for expected in [
        "## Overview",
        "## Threads of `snippet::main`",
        "**#1 thread 'messen'** — started by #0 at snippet.rs:6 in `snippet::main`",
        "**`tx → rx`** #1 → #0",
        "message `Messung`, capacity 4",
        "send in #1 thread 'messen': `send` in `snippet::messen` (snippet.rs:20) — blocking",
        "recv in #0 main thread: `recv` in `snippet::main` (snippet.rs:10) — blocking",
        "**`stand`** `Mutex`: used by #0, #1",
        "## Hot spots",
        "- thread names: `\"messen\"`",
        "- channel ends: `tx`, `rx`",
        "## Limits of this report",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
}

#[test]
fn common_std_method_names_are_not_guessed() {
    let p = rv_syn::load_str(
        r#"
        struct Puffer;
        impl Puffer {
            fn len(&self) -> usize { 0 }
            fn leeren(&mut self) {}
        }
        fn f(v: Vec<u8>, p: &mut Puffer) -> usize {
            p.leeren();
            v.len()
        }
        "#,
    );
    let graph = CallGraph::new(&p);
    let callees: Vec<&str> = graph
        .edges
        .iter()
        .map(|e| p.function(e.callee).name.as_str())
        .collect();
    assert_eq!(
        callees,
        ["leeren"],
        "`v.len()` must not resolve to `Puffer::len`"
    );
}
