//! egui front end of RustView.

use std::path::PathBuf;

mod app;
mod callgraph_view;
mod code_view;
mod comm_view;
mod diagram;
mod editor;
mod export;
mod headless;
mod highlight;
mod loader;
mod program_view;
mod report_view;
mod threads;
mod tree;
mod window_frame;
mod wires;

pub use editor::{DEFAULT_TEMPLATE as DEFAULT_EDITOR, EditorCommand};
pub use headless::{export_all, report};

pub struct Options {
    /// Cargo project directory or single `.rs` file; `None` shows the start screen.
    pub path: Option<PathBuf>,
    pub editor: EditorCommand,
    /// Function to select after loading (qualified path or a `::`-suffix of it).
    pub select: Option<String>,
    /// Start with the thread overview in this mode (`graph`,
    /// `communication`, `functions`).
    pub threads: Option<String>,
    /// Start with the module call graph; a non-empty value opens that module.
    pub callgraph: Option<String>,
    /// Expression level for whole functions from the start.
    pub expressions: bool,
}

pub fn run(options: Options) -> eframe::Result {
    let native = eframe::NativeOptions {
        viewport: window_frame::viewport(
            egui::ViewportBuilder::default()
                .with_title("RustView")
                .with_inner_size([1600.0, 1000.0]),
        ),
        ..Default::default()
    };
    eframe::run_native(
        "RustView",
        native,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, options)))),
    )
}
