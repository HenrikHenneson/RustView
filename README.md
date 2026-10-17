# RustView

LabVIEW-style structural view of Rust code: nesting, data flow and
parallelism of a program at a glance. Analysis and display only — no
editing, no code generation.

```
cargo run --release -p rustview -- [<cargo project dir | file.rs>] [--select <fn path>] [--editor "<cmd>"]
```

Without a path RustView starts with a project selection screen. Projects can
also be opened via *File → Open Project Folder…* (Ctrl+O), *File → Recent*, or
by dropping a folder or `.rs` file onto the window. The release binary
(`target/release/rustview.exe`) can be started by double click.

- `--threads` starts with the project-wide thread overview.
- `--select ec_task::ethercat_task` selects a function after loading
  (full qualified path or any `::`-suffix of it).
- `--editor` defaults to `code --goto {file}:{line}:{col}`.

Click a function to show its structure diagram and source. In the diagram,
click a node to jump to its code, double-click to open it in the editor, use
the triangle to expand/collapse a structure and ◀ ▶ to switch cases. Clicking a
line in the code panel selects the matching node in the diagram. Drag to move,
Ctrl+wheel to zoom. Files are reloaded on change.

Data wires connect the place where a local variable gets its value to the
places that use it (hover a wire for the name). Changes (`=`, `+=`, `&mut`,
`push`, ...) are drawn as orange sequence wires; dashed means "a method call on
a `mut` variable that may change it" or "guessed from an unexpanded macro".
A structure that changes a variable is its source afterwards (like a LabVIEW
output tunnel). In `auto` mode large diagrams only show the wires of the
selected or hovered node.

Parallelism: started threads and tasks are recognized by API name
(`thread::spawn`, `thread::Builder` incl. the thread name, `thread::scope`,
tokio/smol/async-std `spawn`, `spawn_blocking`, `block_on`, rayon `par_iter`/
`join`/`scope`, `join!`/`select!`, `future::zip`/`race`). Their closures are
shown as colored lanes; thick teal wires connect the send and receive ends of
channels (std/crossbeam/tokio/async-channel/flume), dotted purple wires show
shared state (`Arc<Mutex/RwLock/Atomic…>`, followed through clones and type
aliases) handed to other lanes. The *Threads* tab (or `--threads`) lists all
threads of the project, one row per starting function, with links to where
they start and what they run.

## Crates

| crate      | role |
|------------|------|
| `rv-ir`    | backend-independent model (project, modules, functions, spans) |
| `rv-syn`   | frontend: `cargo metadata` + `syn` → `rv-ir` |
| `rv-analysis` | data flow (name resolution, def-use, mutation edges), concurrency (lanes, channels, shared state) |
| `rv-layout`| diagram geometry (nested frames, both orientations), no GUI dependency |
| `rv-gui`   | egui application |
| `rustview` | binary / CLI |

M5 (optional): call graph view on module level.

## Limits

Purely syntactic: macros are not expanded, names and types are not
resolved, `cfg` attributes are not evaluated. Test code (`#[cfg(test)]`,
`#[test]`, `tests/`, benches, examples) is counted, not shown.
