//! How threads communicate: channel ends through return values and struct
//! fields, message types, capacities, operations and joins.

use rv_analysis::{CallGraph, Facts, Program};
use rv_ir::Project;

const SRC: &str = r#"
fn main() {
    let (cmd_tx, evt_rx) = spawn_worker();
    let app = App { cmd_tx, evt_rx };
    let h = std::thread::spawn(move || zaehlen());
    run(app);
    h.join().unwrap();
}

fn spawn_worker() -> (Sender<Cmd>, Receiver<Evt>) {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Cmd>();
    let (evt_tx, evt_rx) = std::sync::mpsc::sync_channel(8);
    std::thread::Builder::new()
        .name("worker".into())
        .spawn(move || worker(cmd_rx, evt_tx))
        .unwrap();
    (cmd_tx, evt_rx)
}

fn worker(rx: Receiver<Cmd>, tx: SyncSender<Evt>) {
    while let Ok(c) = rx.recv() {
        tx.send(Evt::Done).unwrap();
    }
}

fn zaehlen() {}

fn run(app: App) {
    app.einmal();
}

struct App {
    cmd_tx: Sender<Cmd>,
    evt_rx: Receiver<Evt>,
}

impl App {
    fn einmal(&self) {
        self.cmd_tx.send(Cmd::Go).unwrap();
        while let Ok(e) = self.evt_rx.try_recv() {
            zeigen(e);
        }
    }
}

fn zeigen(e: Evt) {}
"#;

fn setup() -> (Project, Program) {
    let p = rv_syn::load_str(SRC);
    let facts = Facts::new(&p);
    let graph = CallGraph::new(&p);
    let entry = rv_analysis::entry_points(&p)[0];
    let program = rv_analysis::program(&p, &facts, &graph, entry);
    (p, program)
}

/// `(thread, function, op)` of the sites, sorted.
fn sites(p: &Project, sites: &[rv_analysis::Site]) -> Vec<(usize, String, String)> {
    let mut v: Vec<_> = sites
        .iter()
        .map(|s| {
            (
                s.thread,
                p.function(s.func).name.clone(),
                s.op.clone().unwrap_or_default(),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn channels_through_returns_and_struct_fields() {
    let (p, program) = setup();
    let titles: Vec<String> = program.threads.iter().map(|t| t.title()).collect();
    assert_eq!(titles, ["main thread", "thread", "thread 'worker'"]);

    let cmd = program
        .channels
        .iter()
        .find(|c| c.name == "cmd_tx → cmd_rx")
        .unwrap();
    assert_eq!(cmd.message.as_deref(), Some("Cmd"));
    assert_eq!(cmd.capacity, None);
    assert_eq!(
        sites(&p, &cmd.senders),
        [
            (0, "einmal".into(), "send".into()),
            (0, "main".into(), "stored in App.cmd_tx".into())
        ]
    );
    assert_eq!(
        sites(&p, &cmd.receivers),
        [(2, "worker".into(), "recv".into())]
    );

    let evt = program
        .channels
        .iter()
        .find(|c| c.name == "evt_tx → evt_rx")
        .unwrap();
    assert_eq!(
        evt.message.as_deref(),
        Some("Evt"),
        "from the parameter type"
    );
    assert_eq!(evt.capacity.as_deref(), Some("8"));
    assert_eq!(
        sites(&p, &evt.senders),
        [(2, "worker".into(), "send".into())]
    );
    assert_eq!(
        sites(&p, &evt.receivers),
        [
            (0, "einmal".into(), "try_recv".into()),
            (0, "main".into(), "stored in App.evt_rx".into())
        ]
    );
}

#[test]
fn joins_are_found() {
    let (p, program) = setup();
    assert_eq!(program.joins.len(), 1);
    let j = &program.joins[0];
    assert_eq!(program.threads[j.thread].title(), "thread");
    assert_eq!(sites(&p, &j.waiters), [(0, "main".into(), "join".into())]);
}
