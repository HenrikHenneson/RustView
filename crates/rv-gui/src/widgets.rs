//! Small widgets that several views use.

use egui::{Response, RichText, Ui};

/// Spinner in the middle of a panel whose content is still being loaded.
pub fn loading(ui: &mut Ui) {
    ui.centered_and_justified(|ui| ui.spinner());
}

/// Link in the monospace font, as used for names from the code.
pub fn mono_link(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.link(RichText::new(text).monospace())
}

/// The "Export…" button of a view; true when it was clicked.
pub fn export_button(ui: &mut Ui, hover_text: &str) -> bool {
    ui.button("Export…").on_hover_text(hover_text).clicked()
}
