//! Whole-program thread model on a small multi-function program.

use rv_analysis::{CallGraph, Facts, LinkKind, Program};
use rv_ir::Project;

const SRC: &str = r#"
fn main() {
    let cmd = Arc::new(Mutex::new(State::default()));
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    let cmd_w = cmd.clone();
    starte_messung(tx, cmd_w);
    std::thread::Builder::new()
        .name("auswertung".into())
        .spawn(move || auswerten(rx))
        .unwrap();
    eframe::run_native("x", opts, Box::new(|_cc| Ok(Box::new(App::new(cmd)))));
}

fn starte_messung(sender: SyncSender<u32>, cmd: Arc<Mutex<State>>) {
    std::thread::Builder::new()
        .name("messung".into())
        .spawn(move || {
            loop {
                let wert = lesen(&cmd);
                sender.send(wert).unwrap();
            }
        })
        .unwrap();
}

fn lesen(cmd: &Arc<Mutex<State>>) -> u32 {
    cmd.lock().unwrap().wert
}

fn auswerten(rx: Receiver<u32>) {
    while let Ok(v) = rx.recv() {
        speichern(v);
    }
}

fn speichern(v: u32) {}

struct App { cmd: Arc<Mutex<State>> }

impl App {
    fn new(cmd: Arc<Mutex<State>>) -> Self { App { cmd } }
    fn exportieren(&self) {
        std::thread::spawn(|| export_schreiben());
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context) {
        if knopf() { self.exportieren(); }
    }
}

fn export_schreiben() {}

fn nie_aufgerufen() {
    std::thread::spawn(|| speichern(1));
}
"#;

fn setup() -> (Project, Program) {
    let p = rv_syn::load_str(SRC);
    let facts = Facts::new(&p);
    let graph = CallGraph::new(&p);
    let entries = rv_analysis::entry_points(&p);
    assert_eq!(entries.len(), 1);
    let program = rv_analysis::program(&p, &facts, &graph, entries[0]);
    (p, program)
}

fn titles(program: &Program) -> Vec<String> {
    program.threads.iter().map(|t| t.title()).collect()
}

fn names(p: &Project, fns: &[rv_ir::FnId]) -> Vec<String> {
    fns.iter().map(|&f| p.function(f).name.clone()).collect()
}

#[test]
fn threads_are_found_through_calls_and_callbacks() {
    let (p, program) = setup();
    // Discovery order: spawns of a function before the functions it calls.
    assert_eq!(
        titles(&program),
        [
            "main thread",
            "thread 'auswertung'",
            "thread 'messung'",
            "thread"
        ]
    );
    // Who runs what.
    assert_eq!(
        names(&p, &program.threads[0].runs),
        ["main", "starte_messung", "new", "update", "exportieren"]
    );
    assert_eq!(
        names(&p, &program.threads[1].runs),
        ["auswerten", "speichern"]
    );
    assert_eq!(names(&p, &program.threads[2].runs), ["lesen"]);
    assert_eq!(names(&p, &program.threads[3].runs), ["export_schreiben"]);
    assert_eq!(names(&p, &program.threads[0].callbacks), ["update"]);
    // The spawn in a function nobody calls is reported.
    assert_eq!(program.unreached.len(), 1);
    assert_eq!(p.function(program.unreached[0].0).name, "nie_aufgerufen");
}

#[test]
fn links_follow_channels_and_shared_state_across_functions() {
    let (_, program) = setup();
    let mut links: Vec<(usize, usize, String)> = program
        .links
        .iter()
        .map(|l| {
            let what = match &l.kind {
                LinkKind::Spawn => "spawn".to_string(),
                LinkKind::Channel { name, .. } => format!("channel {name}"),
            };
            (l.from, l.to, what)
        })
        .collect();
    links.sort();
    assert_eq!(
        links,
        [
            (0, 1, "spawn".to_string()),
            (0, 2, "spawn".to_string()),
            (0, 3, "spawn".to_string()),
            (2, 1, "channel tx → rx".to_string()),
        ]
    );
    assert_eq!(program.resources.len(), 1);
    assert_eq!(program.resources[0].name, "cmd");
    assert_eq!(
        program.resources[0].users,
        [0, 2],
        "main (App) and messung (lesen)"
    );
}
