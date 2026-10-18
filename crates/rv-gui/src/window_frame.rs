//! The window without the system title bar: the menu bar doubles as title
//! bar (drag to move, double-click to maximize) with minimize, maximize and
//! close buttons. On macOS the native "traffic light" buttons stay and the
//! menu bar only moves under the transparent title bar.

use egui::{Context, Sense, Ui, ViewportCommand, vec2};

/// Width of the invisible resize border of the undecorated window.
#[cfg(not(target_os = "macos"))]
const RESIZE_BORDER: f32 = 5.0;

pub fn viewport(builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
    #[cfg(target_os = "macos")]
    {
        builder
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        builder.with_decorations(false)
    }
}

/// Space at the start of the menu bar (room for the macOS window buttons).
pub fn leading_space(ui: &mut Ui) {
    let fullscreen = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
    if cfg!(target_os = "macos") && !fullscreen {
        ui.add_space(68.0);
    }
}

/// Minimize, maximize/restore and close, right to left (call inside a
/// right-to-left layout). Nothing on macOS.
pub fn buttons(ui: &mut Ui) {
    if cfg!(target_os = "macos") {
        return;
    }
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    if icon_button(ui, Icon::Close, "Close") {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
    let (icon, tip) = if maximized {
        (Icon::Restore, "Restore")
    } else {
        (Icon::Maximize, "Maximize")
    };
    if icon_button(ui, icon, tip) {
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
    if icon_button(ui, Icon::Minimize, "Minimize") {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
    ui.separator();
}

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// A frameless button with a drawn icon (the default fonts have no window
/// symbols). Close turns red on hover, like the system buttons.
fn icon_button(ui: &mut Ui, icon: Icon, tip: &str) -> bool {
    let size = vec2(34.0, ui.available_height().max(20.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    if hovered {
        let fill = if icon == Icon::Close {
            egui::Color32::from_rgb(196, 43, 28)
        } else {
            ui.visuals().widgets.hovered.weak_bg_fill
        };
        painter.rect_filled(rect, 0.0, fill);
    }
    let color = if hovered && icon == Icon::Close {
        egui::Color32::WHITE
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    let stroke = egui::Stroke::new(1.0, color);
    let c = rect.center();
    let r = 5.0;
    match icon {
        Icon::Minimize => {
            painter.line_segment([c + vec2(-r, 0.0), c + vec2(r, 0.0)], stroke);
        }
        Icon::Maximize => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, vec2(2.0 * r, 2.0 * r)),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Restore => {
            let back = egui::Rect::from_min_size(
                c + vec2(-r + 2.0, -r),
                vec2(2.0 * r - 2.0, 2.0 * r - 2.0),
            );
            let front = back.translate(vec2(-2.0, 2.0));
            painter.line_segment([back.left_top(), back.right_top()], stroke);
            painter.line_segment([back.right_top(), back.right_bottom()], stroke);
            painter.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Middle);
        }
        Icon::Close => {
            painter.line_segment([c + vec2(-r, -r), c + vec2(r, r)], stroke);
            painter.line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke);
        }
    }
    response.on_hover_text(tip).clicked()
}

/// The remaining space of the menu bar moves the window; a double click
/// maximizes or restores it.
pub fn drag_area(ui: &mut Ui) {
    let response = ui.allocate_response(ui.available_size(), Sense::click_and_drag());
    if response.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if response.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// Resizing at the window border, which the undecorated window lacks.
/// Call before the panels so the border wins over widgets below it.
pub fn resize_border(ctx: &Context) {
    #[cfg(not(target_os = "macos"))]
    {
        use egui::CursorIcon;
        use egui::viewport::ResizeDirection;

        let (maximized, fullscreen) = ctx.input(|i| {
            let v = i.viewport();
            (v.maximized.unwrap_or(false), v.fullscreen.unwrap_or(false))
        });
        if maximized || fullscreen {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let rect = ctx.content_rect();
        let near = |a: f32, b: f32| (a - b).abs() <= RESIZE_BORDER;
        let (w, e) = (near(pos.x, rect.left()), near(pos.x, rect.right()));
        let (n, s) = (near(pos.y, rect.top()), near(pos.y, rect.bottom()));
        let dir = match (n, s, w, e) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (true, _, _, true) => ResizeDirection::NorthEast,
            (_, true, true, _) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::North,
            (_, true, ..) => ResizeDirection::South,
            (_, _, true, _) => ResizeDirection::West,
            (_, _, _, true) => ResizeDirection::East,
            _ => return,
        };
        ctx.set_cursor_icon(match dir {
            ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
            ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
        });
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
    #[cfg(target_os = "macos")]
    let _ = ctx;
}
