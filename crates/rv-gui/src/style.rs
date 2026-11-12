//! Colors shared by the views, so that the same thing has the same color in
//! the thread graph, the communication matrix, the call graph and the exports.

use egui::Color32;

/// Threads and tasks: borders of thread boxes and of everything that starts threads.
pub const THREAD: Color32 = Color32::from_rgb(0, 160, 120);
/// "Starts a thread" arrows.
pub const SPAWN: Color32 = Color32::from_rgb(130, 130, 130);
pub const CHANNEL: Color32 = Color32::from_rgb(0, 150, 165);
/// Shared state (`Arc<Mutex<…>>` and the like).
pub const SHARED: Color32 = Color32::from_rgb(150, 90, 210);
pub const JOIN: Color32 = Color32::from_rgb(210, 110, 0);

/// Calls leaving the module in focus.
pub const CALLS_OUT: Color32 = Color32::from_rgb(40, 110, 220);
/// Calls coming into the module in focus.
pub const CALLS_IN: Color32 = Color32::from_rgb(30, 150, 80);

/// Exported images always use light colors, whatever the theme of the window.
pub mod light {
    use egui::Color32;

    pub const FILL: Color32 = Color32::WHITE;
    pub const TITLE: Color32 = Color32::from_gray(20);
    pub const TEXT: Color32 = Color32::from_gray(40);
    pub const WEAK_TEXT: Color32 = Color32::from_gray(120);
}
