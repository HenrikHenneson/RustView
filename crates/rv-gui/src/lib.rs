//! egui front end of RustView.

use std::path::PathBuf;

mod app;
mod code_view;
mod diagram;
mod editor;
mod highlight;
mod loader;
mod tree;

pub use editor::{DEFAULT_TEMPLATE as DEFAULT_EDITOR, EditorCommand};

pub struct Options {
    /// Cargo project directory or single `.rs` file; `None` shows the start screen.
    pub path: Option<PathBuf>,
    pub editor: EditorCommand,
    /// Function to select after loading (qualified path or a `::`-suffix of it).
    pub select: Option<String>,
}

pub fn run(options: Options) -> eframe::Result {
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RustView")
            .with_inner_size([1600.0, 1000.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RustView",
        native,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, options)))),
    )
}
