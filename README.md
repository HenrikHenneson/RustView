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

- `--select ec_task::ethercat_task` selects a function after loading
  (full qualified path or any `::`-suffix of it).
- `--editor` defaults to `code --goto {file}:{line}:{col}`.

Click a function to show its structure diagram and source. In the diagram,
click a node to jump to its code, double-click to open it in the editor, use
the triangle to expand/collapse a structure and ◀ ▶ to switch cases. Clicking a
line in the code panel selects the matching node in the diagram. Drag to move,
Ctrl+wheel to zoom. Files are reloaded on change.

## Crates

| crate      | role |
|------------|------|
| `rv-ir`    | backend-independent model (project, modules, functions, spans) |
| `rv-syn`   | frontend: `cargo metadata` + `syn` → `rv-ir` |
| `rv-layout`| diagram geometry (nested frames, both orientations), no GUI dependency |
| `rv-gui`   | egui application |
| `rustview` | binary / CLI |

`rv-analysis` (def-use, concurrency) follows with M3/M4.

## Limits

Purely syntactic: macros are not expanded, names and types are not
resolved, `cfg` attributes are not evaluated. Test code (`#[cfg(test)]`,
`#[test]`, `tests/`, benches, examples) is counted, not shown.
