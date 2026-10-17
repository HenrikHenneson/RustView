//! Recognition of threads, tasks, channels and shared state.

use rv_analysis::{Concurrency, End, SpawnKind};
use rv_ir::Body;

fn analyze(src: &str) -> (Body, Concurrency) {
    let p = rv_syn::load_str(src);
    let f = p.functions.iter().find(|f| f.name == "f").expect("fn f");
    let body = f.body.clone().unwrap();
    let flow = rv_analysis::data_flow(&body);
    let c = rv_analysis::concurrency(&body, &flow, &p.type_aliases);
    (body, c)
}

/// Label of a node, shortened for assertions.
fn label(body: &Body, id: rv_ir::NodeId) -> String {
    body.node(id).label.chars().take(40).collect()
}

#[test]
fn threads_channels_and_shared_state_like_main() {
    let (body, c) = analyze(
        r#"
fn f() {
    let cmd = Arc::new(Mutex::new(CmdState::default()));
    let (sample_tx, rx) = std::sync::mpsc::sync_channel::<RawSample>(256);
    let cmd_proc = cmd.clone();
    std::thread::Builder::new()
        .name("processing".to_string())
        .spawn(move || processing_task::processing_task(rx, cmd_proc))
        .expect("spawn");
    let cmd_ec = cmd.clone();
    std::thread::Builder::new()
        .name("ec-task".to_string())
        .spawn(move || {
            pin_thread_to_core(3);
            smol::block_on(ethercat_task(cmd_ec, sample_tx));
        })
        .expect("spawn");
    cmd.lock().unwrap().running = true;
}"#,
    );

    let threads: Vec<_> = c
        .spawns
        .iter()
        .filter(|s| s.kind == SpawnKind::Thread)
        .collect();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].name.as_deref(), Some("processing"));
    assert!(threads[0].lanes.is_empty());
    assert_eq!(
        threads[0].inline,
        ["move || processing_task::processing_task(rx, cmd_proc)"]
    );
    assert_eq!(threads[0].runs, ["processing_task::processing_task"]);
    assert_eq!(threads[1].name.as_deref(), Some("ec-task"));
    assert_eq!(threads[1].lanes.len(), 1);
    // Calls are in evaluation order: arguments first.
    assert_eq!(
        threads[1].runs,
        ["pin_thread_to_core", "ethercat_task", "smol::block_on"]
    );
    assert!(!threads[1].in_loop);
    // `smol::block_on` inside the ec-task lane is recognized but not parallel.
    let block_on = c
        .spawns
        .iter()
        .find(|s| s.kind == SpawnKind::BlockOn)
        .unwrap();
    assert_eq!(c.lane_of(&body, block_on.node), Some(1));

    assert_eq!(c.channels.len(), 1);
    assert_eq!(c.channels[0].name, "sample_tx → rx");
    let mut ends: Vec<(String, End, Option<String>)> = c
        .channel_uses
        .iter()
        .map(|u| (label(&body, u.node), u.end, u.op.clone()))
        .collect();
    ends.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        ends,
        [
            (
                "smol::block_on(ethercat_task(cmd_ec, sam".to_string(),
                End::Send,
                None
            ),
            (
                "std::thread::Builder::new().name(\"proces".to_string(),
                End::Recv,
                None
            ),
        ]
    );

    assert_eq!(c.shared.len(), 1);
    assert_eq!(
        (c.shared[0].name.as_str(), c.shared[0].kind.as_str()),
        ("cmd", "Mutex")
    );
    assert_eq!(c.shared[0].bindings.len(), 3, "cmd, cmd_proc, cmd_ec");
    let mut uses: Vec<(String, Option<String>)> = c
        .shared_uses
        .iter()
        .map(|u| (label(&body, u.node), u.op.clone()))
        .collect();
    uses.sort();
    assert_eq!(
        uses,
        [
            (
                "cmd.lock().unwrap().running = true".to_string(),
                Some("lock".to_string())
            ),
            ("smol::block_on(ethercat_task(cmd_ec, sam".to_string(), None),
            (
                "std::thread::Builder::new().name(\"proces".to_string(),
                None
            ),
        ]
    );
}

#[test]
fn thread_per_connection_in_a_loop() {
    let (_, c) = analyze(
        r#"
fn f(lauscher: TcpListener, postfach: Arc<Postfach>) {
    std::thread::Builder::new()
        .name("mtd-fern".into())
        .spawn(move || {
            for strom in lauscher.incoming() {
                let postfach = postfach.clone();
                let _ = std::thread::Builder::new()
                    .name("mtd-fern-verb".into())
                    .spawn(move || bedienen(strom, postfach));
            }
        })
        .map_err(|e| e.to_string())?;
}"#,
    );
    let names: Vec<_> = c
        .spawns
        .iter()
        .map(|s| (s.name.clone().unwrap(), s.in_loop))
        .collect();
    assert_eq!(
        names,
        [
            ("mtd-fern".to_string(), false),
            ("mtd-fern-verb".to_string(), true)
        ]
    );
    // Shared state from the parameter type, followed through the clone.
    assert_eq!(c.shared.len(), 1);
    assert_eq!(c.shared[0].bindings.len(), 2);
}

#[test]
fn scoped_threads_rayon_tokio_and_select() {
    let (_, c) = analyze(
        r#"
fn f(data: Vec<u32>, rx: Receiver<u32>) {
    std::thread::scope(|s| {
        s.spawn(|| { work(1); });
        s.spawn(|| work(2));
    });
    let total: u32 = data.par_iter().map(|x| { heavy(*x) }).sum();
    let h = tokio::spawn(async move { serve().await; });
    tokio::select! {
        v = rx.recv() => handle(v),
        _ = h => {}
    }
    let (a, b) = rayon::join(|| left(), || right());
}"#,
    );
    let kinds: Vec<SpawnKind> = c.spawns.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            SpawnKind::ScopedThread,
            SpawnKind::ScopedThread,
            SpawnKind::ParallelIter,
            SpawnKind::AsyncTask,
            SpawnKind::Select,
            SpawnKind::Join,
        ]
    );
    assert_eq!(
        c.spawns[2].lanes.len(),
        1,
        "the map closure runs in parallel"
    );
    assert_eq!(c.spawns[5].inline.len(), 2);
}

#[test]
fn shared_state_through_type_alias() {
    let (_, c) = analyze(
        r#"
type SharedCmd = Arc<Mutex<CmdState>>;
fn f(cmd: SharedCmd, flag: &AtomicBool) {
    let c = cmd.lock_recover();
    flag.store(true, Ordering::SeqCst);
}"#,
    );
    let groups: Vec<_> = c
        .shared
        .iter()
        .map(|g| (g.name.as_str(), g.kind.as_str()))
        .collect();
    assert_eq!(groups, [("cmd", "Mutex"), ("flag", "Atomic")]);
    let ops: Vec<_> = c
        .shared_uses
        .iter()
        .map(|u| u.op.clone().unwrap())
        .collect();
    assert_eq!(ops, ["lock_recover", "store"]);
}
