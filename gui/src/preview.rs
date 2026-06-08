//! Interactive chart preview: a camera-transformed texture with egui gestures.
//!
//! [`preview`] renders the current [`BuilderState`] to an SVG through the
//! plotters backend, rasterizes it to an `egui::ColorImage`, and paints that as
//! a texture transformed by a [`Camera`]. The texture is cached in a
//! [`View`] the host holds across frames and only re-uploaded when the
//! chart's SVG actually changes (keyed on a hash of the SVG string), mirroring
//! the foundation's one-slot render cache idea.
//!
//! Gestures are read from egui's `InputState` exactly as
//! `hiker-canvas/view/src/widget.rs::handle_zoom` does: a pinch or Ctrl/Cmd
//! scroll (both folded by egui into `zoom_delta`) zooms toward the cursor via
//! `zoom_to_cursor`; any other scroll pans via `pan_by_screen`; and the keyboard
//! `+`/`-`/`0` keys zoom in, zoom out, and fit-to-content. No winit dependency:
//! everything flows through egui, so the host's forked-winit pinch support comes
//! in for free.

use std::hash::{Hash, Hasher};

use egui::{Key, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use hiker_charts_plotters::PlottersSvg;

use crate::camera::Camera;
use crate::model::BuilderState;

/// The factor a single `+`/`-` keypress zooms by (matches hiker-canvas).
const KEY_ZOOM_STEP: f32 = 1.2;

/// Which pane the right side of the builder shows: the interactive chart preview, or the
/// read-only parsed-data grid. Held across frames on [`View`] so the tab choice persists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneMode {
    /// The rendered chart, pan/zoomable (the default).
    #[default]
    Chart,
    /// The source table with inferred types and coerced/formatted values.
    Data,
}

/// Cross-frame state for the preview: the uploaded texture and the hash of the
/// SVG that produced it, so the texture re-uploads only when the chart changes.
#[derive(Default)]
pub struct View {
    /// The uploaded chart texture, `None` until the first successful render.
    texture: Option<TextureHandle>,
    /// Hash of the SVG string currently held in `texture`; re-upload on change.
    key: Option<u64>,
    /// Whether the camera has been framed to the chart once. The first frame with
    /// a real viewport + content auto-fits so the whole chart is visible; after
    /// that the user's pan/zoom is left alone (re-fit on demand with the `0` key).
    fitted: bool,
    /// Which pane (chart vs data) the right side currently shows.
    pub mode: PaneMode,
}

impl View {
    /// A fresh view with no texture uploaded yet, showing the chart pane.
    #[must_use]
    pub const fn new() -> Self {
        Self { texture: None, key: None, fitted: false, mode: PaneMode::Chart }
    }
}

/// Hash an SVG string into a cache key for the texture slot.
fn svg_key(svg: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    svg.hash(&mut hasher);
    hasher.finish()
}

/// Render `state` to a texture in `view`, re-uploading only when the chart's SVG
/// changed. Returns the texture's world-space pixel size, or `None` when the
/// chart has no drawable data (so the caller can paint a placeholder).
fn ensure_texture(state: &mut BuilderState, view: &mut View, ui: &egui::Ui) -> Option<Vec2> {
    let svg = state.render(&PlottersSvg)?.svg.clone();
    let key = svg_key(&svg);
    if view.key != Some(key) {
        let image = crate::raster::rasterize(&svg, ui.ctx().pixels_per_point())?;
        match view.texture.as_mut() {
            Some(tex) => tex.set(image, TextureOptions::LINEAR),
            None => {
                view.texture =
                    Some(ui.ctx().load_texture("hiker-chart-preview", image, TextureOptions::LINEAR));
            }
        }
        view.key = Some(key);
    }
    view.texture.as_ref().map(|t| {
        let [w, h] = t.size();
        Vec2::new(w as f32, h as f32)
    })
}

/// Feed this frame's egui gestures into `camera`, mirroring hiker-canvas
/// `handle_zoom`: keyboard `+`/`-`/`0`, then pinch/Ctrl-scroll as zoom-to-cursor
/// and any other scroll as a pan. `content` is the texture's world size, used to
/// fit on the `0` key.
fn handle_gestures(camera: &mut Camera, ui: &egui::Ui, viewport: Rect, content: Vec2) {
    let (zin, zout, fit, scroll, zoom, pointer) = ui.input(|i| {
        (
            i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals),
            i.key_pressed(Key::Minus),
            i.key_pressed(Key::Num0),
            i.smooth_scroll_delta,
            i.zoom_delta(),
            i.pointer.hover_pos(),
        )
    });
    let anchor = pointer.filter(|p| viewport.contains(*p)).unwrap_or_else(|| viewport.center());
    if fit {
        camera.zoom_to_fit(viewport, content);
    } else if zin {
        camera.zoom_to_cursor(viewport, anchor, KEY_ZOOM_STEP);
    } else if zout {
        camera.zoom_to_cursor(viewport, anchor, 1.0 / KEY_ZOOM_STEP);
    }
    // Pointer-driven gestures only apply when the cursor is over the viewport.
    let Some(cursor) = pointer.filter(|p| viewport.contains(*p)) else { return };
    if (zoom - 1.0).abs() > f32::EPSILON {
        camera.zoom_to_cursor(viewport, cursor, zoom);
    } else if scroll.length() > 0.5 {
        camera.pan_by_screen(scroll);
    }
}

/// Paint `texture` (world size `content`) into `viewport` under `camera`.
fn paint_texture(
    ui: &egui::Ui,
    viewport: Rect,
    camera: &Camera,
    texture: &TextureHandle,
    content: Vec2,
) {
    let top_left = camera.world_to_screen(viewport, Pos2::ZERO);
    let bottom_right = camera.world_to_screen(viewport, content.to_pos2());
    let rect = Rect::from_two_pos(top_left, bottom_right);
    let painter = ui.painter_at(viewport);
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    painter.image(texture.id(), rect, uv, egui::Color32::WHITE);
}

/// The interactive preview widget: render + cache the chart texture, allocate a
/// response rect, paint the texture transformed by `camera`, and feed this
/// frame's gestures into `camera`. The host holds `view` across frames so the
/// texture is reused; the host holds `camera` so pan/zoom persist.
pub fn preview(
    state: &mut BuilderState,
    camera: &mut Camera,
    view: &mut View,
    ui: &mut egui::Ui,
) -> egui::Response {
    let desired = ui.available_size_before_wrap().max(Vec2::new(160.0, 120.0));
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let viewport = rect.intersect(ui.clip_rect());
    if let Some(content) = ensure_texture(state, view, ui) {
        // Frame the chart into the viewport the first time we have both a real
        // viewport and content, so it isn't painted zoomed-in past the edges.
        if !view.fitted && viewport.width() > 1.0 && viewport.height() > 1.0 {
            camera.zoom_to_fit(viewport, content);
            view.fitted = true;
        }
        if let Some(texture) = view.texture.as_ref() {
            paint_texture(ui, viewport, camera, texture, content);
        }
        if response.dragged() {
            camera.pan_by_screen(response.drag_delta());
        }
        handle_gestures(camera, ui, viewport, content);
    } else {
        ui.painter_at(viewport)
            .text(viewport.center(), egui::Align2::CENTER_CENTER, "no data to chart", egui::FontId::default(), ui.visuals().weak_text_color());
    }
    response
}

#[cfg(test)]
mod tests {
    use super::{preview, svg_key, View};
    use crate::camera::Camera;
    use crate::model::BuilderState;
    use hiker_charts_core::backend::Size;
    use hiker_charts_core::data::Table;
    use hiker_charts_core::dsl::ChartSpec;
    use hiker_charts_core::theme::Theme;

    fn state() -> BuilderState {
        let spec = ChartSpec::from_yaml("mark: line\nx: month\ny: revenue\n").unwrap();
        let table = Table::from_csv(b"month,revenue\njan,100\nfeb,140\n").unwrap();
        BuilderState::new(spec, table, Theme::default(), Size { width: 320, height: 240 })
    }

    #[test]
    fn svg_key_differs_by_content() {
        assert_ne!(svg_key("<svg/>"), svg_key("<svg></svg>"));
        assert_eq!(svg_key("<svg/>"), svg_key("<svg/>"));
    }

    #[test]
    fn preview_runs_a_frame_without_panicking() {
        let mut s = state();
        let mut cam = Camera::default();
        let mut view = View::new();
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = preview(&mut s, &mut cam, &mut view, ui);
            });
        });
        // A successful frame uploaded the chart texture and recorded its key.
        assert!(view.texture.is_some());
        assert!(view.key.is_some());
    }
}
