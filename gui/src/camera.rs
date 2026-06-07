//! Pan/zoom viewport transform for the interactive preview widget.
//!
//! A [`Camera`] holds a pan offset (the world point pinned to the viewport's
//! top-left) plus a zoom `scale`, and converts between world space (the chart
//! texture's own pixel coordinates) and on-screen pixels. The math mirrors
//! `hiker-canvas/view-core/src/camera.rs`: `world_to_screen` maps a world point
//! `p` to `viewport.min + (p - pan) * scale`, and `zoom_to_cursor` re-pins `pan`
//! so the world point under the cursor stays fixed while zooming. It carries no
//! pixel geometry of its own — the viewport `Rect` is supplied per call — and
//! needs no egui `Context`, only the geometry types, so it is fully headless.

use egui::{Pos2, Rect, Vec2};

/// The smallest zoom factor a gesture clamps to (heavily zoomed out).
const MIN_SCALE: f32 = 0.05;
/// The largest zoom factor a gesture clamps to (heavily zoomed in).
const MAX_SCALE: f32 = 20.0;

/// A pan + zoom viewport over the chart texture's world coordinates.
///
/// `pan` is the world-space point that sits at the top-left (`viewport.min`) of
/// the on-screen rect; `scale` is screen pixels per world unit. Both gesture
/// helpers keep `scale` within `MIN_SCALE..=MAX_SCALE`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// The world-space point pinned to the viewport's top-left corner.
    pan: Vec2,
    /// Screen pixels per world unit.
    scale: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { pan: Vec2::ZERO, scale: 1.0 }
    }
}

impl Camera {
    /// The current zoom factor (screen pixels per world unit).
    #[must_use]
    pub const fn scale(&self) -> f32 {
        self.scale
    }

    /// The world point pinned to the viewport's top-left corner.
    #[must_use]
    pub const fn pan(&self) -> Vec2 {
        self.pan
    }

    /// Map a world-space point to screen pixels within `viewport`.
    #[must_use]
    pub fn world_to_screen(&self, viewport: Rect, p: Pos2) -> Pos2 {
        viewport.min + (p.to_vec2() - self.pan) * self.scale
    }

    /// Map a screen-pixel position within `viewport` back to world space.
    #[must_use]
    pub fn screen_to_world(&self, viewport: Rect, pos: Pos2) -> Pos2 {
        ((pos - viewport.min) / self.scale + self.pan).to_pos2()
    }

    /// Pan by a screen-pixel delta (the drag gesture). Dragging the content
    /// right (`+delta.x`) moves the pinned world point left, so the content
    /// follows the cursor.
    pub fn pan_by_screen(&mut self, delta: Vec2) {
        self.pan -= delta / self.scale;
    }

    /// Zoom by `factor` while keeping the world point under `cursor` fixed on
    /// screen (scroll/pinch toward the cursor). `factor > 1` zooms in. The new
    /// scale is clamped to `MIN_SCALE..=MAX_SCALE`.
    pub fn zoom_to_cursor(&mut self, viewport: Rect, cursor: Pos2, factor: f32) {
        let anchor = self.screen_to_world(viewport, cursor);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        // Re-pin `pan` so `anchor` maps back to the same screen `cursor`.
        self.pan = anchor.to_vec2() - (cursor - viewport.min) / self.scale;
    }

    /// Frame `content` (its size in world units, anchored at world origin) within
    /// `viewport`, leaving roughly 5% padding on each side and centering it. A
    /// degenerate content or viewport resets to scale 1 centered at the origin.
    pub fn zoom_to_fit(&mut self, viewport: Rect, content: Vec2) {
        let (vw, vh) = (viewport.width(), viewport.height());
        if content.x <= 0.0 || content.y <= 0.0 || vw <= 0.0 || vh <= 0.0 {
            self.scale = 1.0;
            self.pan = Vec2::ZERO;
            return;
        }
        let pad = 1.1;
        let sx = vw / (content.x * pad);
        let sy = vh / (content.y * pad);
        self.scale = sx.min(sy).min(MAX_SCALE);
        // Center: the world-space center should land at the viewport center.
        let center = content / 2.0;
        self.pan = center - viewport.size() / 2.0 / self.scale;
    }
}

#[cfg(test)]
mod tests {
    use super::Camera;
    use egui::{Pos2, Rect, Vec2};

    fn viewport() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(800.0, 600.0))
    }

    fn assert_near(a: Pos2, b: Pos2) {
        assert!((a.x - b.x).abs() < 1e-3, "x: {} vs {}", a.x, b.x);
        assert!((a.y - b.y).abs() < 1e-3, "y: {} vs {}", a.y, b.y);
    }

    #[test]
    fn world_screen_round_trips_when_panned_and_zoomed() {
        let mut cam = Camera::default();
        let vp = viewport();
        cam.pan_by_screen(Vec2::new(40.0, -25.0));
        cam.zoom_to_cursor(vp, Pos2::new(300.0, 200.0), 2.5);
        let p = Pos2::new(64.0, 96.0);
        assert_near(p, cam.screen_to_world(vp, cam.world_to_screen(vp, p)));
    }

    #[test]
    fn zoom_keeps_point_under_cursor_fixed() {
        let mut cam = Camera::default();
        let vp = viewport();
        let cursor = Pos2::new(420.0, 333.0);
        let before = cam.screen_to_world(vp, cursor);
        cam.zoom_to_cursor(vp, cursor, 3.0);
        let after = cam.screen_to_world(vp, cursor);
        assert_near(before, after);
        assert!((cam.scale() - 3.0).abs() < 1e-4);
    }

    #[test]
    fn pan_by_screen_shifts_world_origin() {
        let mut cam = Camera::default();
        let vp = viewport();
        let origin_before = cam.world_to_screen(vp, Pos2::ZERO);
        cam.pan_by_screen(Vec2::new(30.0, 12.0));
        let origin_after = cam.world_to_screen(vp, Pos2::ZERO);
        // Dragging right/down moves the content right/down on screen.
        assert!((origin_after.x - origin_before.x - 30.0).abs() < 1e-3);
        assert!((origin_after.y - origin_before.y - 12.0).abs() < 1e-3);
    }

    #[test]
    fn scale_clamps_to_bounds() {
        let mut cam = Camera::default();
        let vp = viewport();
        for _ in 0..200 {
            cam.zoom_to_cursor(vp, vp.center(), 2.0);
        }
        assert!(cam.scale() <= 20.0 + 1e-3);
        for _ in 0..400 {
            cam.zoom_to_cursor(vp, vp.center(), 0.5);
        }
        assert!(cam.scale() >= 0.05 - 1e-4);
    }

    #[test]
    fn zoom_to_fit_frames_content_centered() {
        let mut cam = Camera::default();
        let vp = viewport();
        let content = Vec2::new(400.0, 300.0);
        cam.zoom_to_fit(vp, content);
        // The content center maps to the viewport center.
        let screen_center = cam.world_to_screen(vp, (content / 2.0).to_pos2());
        assert_near(screen_center, vp.center());
        // The framed content fits inside the viewport.
        let tl = cam.world_to_screen(vp, Pos2::ZERO);
        let br = cam.world_to_screen(vp, content.to_pos2());
        assert!(br.x - tl.x <= vp.width() + 1.0);
        assert!(br.y - tl.y <= vp.height() + 1.0);
    }

    #[test]
    fn zoom_to_fit_degenerate_resets() {
        let mut cam = Camera::default();
        cam.zoom_to_cursor(viewport(), Pos2::new(10.0, 10.0), 4.0);
        cam.zoom_to_fit(viewport(), Vec2::ZERO);
        assert!((cam.scale() - 1.0).abs() < 1e-6);
        assert_eq!(cam.pan(), Vec2::ZERO);
    }
}
