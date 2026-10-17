//! Loading the project in the background and reloading it on file changes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use rv_ir::Project;

pub struct Loaded {
    pub path: PathBuf,
    pub result: anyhow::Result<Project>,
    pub elapsed: Duration,
}

pub struct Loader {
    path: Option<PathBuf>,
    /// Incremented when another project is opened; results of older loads are dropped.
    generation: u64,
    ctx: egui::Context,
    done_tx: Sender<(u64, Loaded)>,
    done_rx: Receiver<(u64, Loaded)>,
    busy: bool,
    /// A change arrived while loading; load again afterwards.
    dirty: bool,
    change_tx: Sender<()>,
    change_rx: Receiver<()>,
    watcher: Option<Debouncer<notify::RecommendedWatcher, RecommendedCache>>,
    pub watch_error: Option<String>,
}

impl Loader {
    pub fn new(ctx: egui::Context) -> Self {
        let (done_tx, done_rx) = channel();
        let (change_tx, change_rx) = channel();
        Loader {
            path: None,
            generation: 0,
            ctx,
            done_tx,
            done_rx,
            busy: false,
            dirty: false,
            change_tx,
            change_rx,
            watcher: None,
            watch_error: None,
        }
    }

    /// Switch to another project and start loading it.
    pub fn open(&mut self, path: PathBuf) {
        self.generation += 1;
        self.path = Some(path);
        self.busy = false;
        self.dirty = false;
        self.watcher = None;
        self.watch_error = None;
        self.change_rx.try_iter().for_each(drop);
        self.reload();
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    pub fn reload(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.busy {
            self.dirty = true;
            return;
        }
        self.busy = true;
        let generation = self.generation;
        let tx = self.done_tx.clone();
        let ctx = self.ctx.clone();
        // A fresh thread per load also frees proc-macro2's thread-local span data.
        std::thread::Builder::new()
            .name("rustview-load".into())
            .spawn(move || {
                let t0 = Instant::now();
                let result = rv_syn::load(&path);
                let loaded = Loaded {
                    path,
                    result,
                    elapsed: t0.elapsed(),
                };
                let _ = tx.send((generation, loaded));
                ctx.request_repaint();
            })
            .expect("spawn loader thread");
    }

    /// Call once per frame. Returns a finished load, if any.
    pub fn poll(&mut self) -> Option<Loaded> {
        if self.change_rx.try_iter().count() > 0 {
            self.reload();
        }
        let current = self.generation;
        let (_, loaded) = self.done_rx.try_iter().find(|(g, _)| *g == current)?;
        self.busy = false;
        if let Ok(project) = &loaded.result {
            self.watch(project);
        }
        if std::mem::take(&mut self.dirty) {
            self.reload();
        }
        Some(loaded)
    }

    /// Watch the directories of all loaded files (non-recursively, so that
    /// `target/` with its thousands of directories is never watched).
    fn watch(&mut self, project: &Project) {
        let mut dirs: BTreeSet<PathBuf> = project
            .files
            .iter()
            .filter_map(|f| f.path.parent().map(Path::to_path_buf))
            .collect();
        dirs.extend(project.packages.iter().map(|p| p.root_dir.clone()));

        let tx = self.change_tx.clone();
        let ctx = self.ctx.clone();
        let debouncer = new_debouncer(
            Duration::from_millis(300),
            None,
            move |res: DebounceEventResult| {
                let Ok(events) = res else { return };
                if events
                    .iter()
                    .any(|e| e.paths.iter().any(|p| is_relevant(p)))
                {
                    let _ = tx.send(());
                    ctx.request_repaint();
                }
            },
        );
        self.watcher = None;
        self.watch_error = None;
        match debouncer {
            Ok(mut d) => {
                for dir in &dirs {
                    if let Err(e) = d.watch(dir, RecursiveMode::NonRecursive) {
                        self.watch_error = Some(format!("{}: {e}", dir.display()));
                    }
                }
                self.watcher = Some(d);
            }
            Err(e) => self.watch_error = Some(e.to_string()),
        }
    }
}

fn is_relevant(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default();
    path.extension().is_some_and(|e| e == "rs") || name == "Cargo.toml"
}
