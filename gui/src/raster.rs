//! Rasterize a self-contained SVG string into an `egui::ColorImage` via resvg.
//!
//! Mirrors the proven path in `../notes` (`render_math.rs` + `render.rs`'s
//! `svg_fontdb`): a process-wide `OnceLock<fontdb::Database>` is loaded once with
//! the bundled Liberation Sans face and the generic `sans-serif` family is pointed
//! at it. This matters because plotters' SVG emits `<text font-family="sans-serif">`
//! with no glyph paths (the `ttf` feature is off, SPEC §4.2); without a populated
//! fontdb mapping that generic, resvg renders every label blank. The SVG is parsed
//! with that fontdb, rendered to a `tiny_skia::Pixmap` at `scale`, and the
//! premultiplied pixels are handed to `ColorImage::from_rgba_unmultiplied`.

use std::sync::{Arc, OnceLock};

use egui::ColorImage;
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::fontdb::Database;
use resvg::usvg::{Options, Tree};

/// The bundled font face. Same Liberation Sans the prototype ships; embedded so a
/// system with no `sans-serif` still resolves chart labels.
const FONT_BYTES: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

/// The family name to map the generic `sans-serif` onto.
const FONT_FAMILY: &str = "Liberation Sans";

/// The shared SVG font database, built once. Holds only the bundled face (the
/// component is host-agnostic and must not depend on system fonts), with the
/// generic `sans-serif`/`serif`/`monospace` families all pointed at it so any
/// `font-family` plotters emits resolves to a real face.
fn fontdb() -> Arc<Database> {
    static DB: OnceLock<Arc<Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = Database::new();
        db.load_font_data(FONT_BYTES.to_vec());
        db.set_sans_serif_family(FONT_FAMILY);
        db.set_serif_family(FONT_FAMILY);
        db.set_monospace_family(FONT_FAMILY);
        Arc::new(db)
    })
    .clone()
}

/// Rasterize an SVG document to an `egui::ColorImage` at `scale` (1.0 = the SVG's
/// intrinsic pixel size; pass the host's pixel ratio for sharp output). Returns
/// `None` on a parse failure or a degenerate size. tiny-skia stores premultiplied
/// pixels, so they are un-premultiplied via `from_rgba_unmultiplied`.
pub fn rasterize(svg: &str, scale: f32) -> Option<ColorImage> {
    let opts = Options { fontdb: fontdb(), ..Options::default() };
    let tree = Tree::from_data(svg.as_bytes(), &opts).ok()?;
    let size = tree.size();
    let w = ((size.width() * scale).round() as u32).max(1);
    let h = ((size.height() * scale).round() as u32).max(1);
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }
    let mut pixmap = Pixmap::new(w, h)?;
    resvg::render(&tree, Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(unpremultiply(&pixmap, w, h))
}

/// Convert a premultiplied tiny-skia pixmap to a straight-alpha `ColorImage`.
/// `ColorImage::from_rgba_unmultiplied` expects un-premultiplied RGBA, so each
/// pixel is run through tiny-skia's `demultiply` (alpha 0 yields transparent).
fn unpremultiply(pixmap: &Pixmap, w: u32, h: u32) -> ColorImage {
    let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba)
}

#[cfg(test)]
mod tests {
    use super::rasterize;
    use hiker_charts_core::backend::{
        Axis, AxisKind, Backend, ResolvedChart, Series, Size,
    };
    use hiker_charts_core::dsl::{Config, Mark};
    use hiker_charts_core::theme::Theme;
    use hiker_charts_plotters::PlottersSvg;

    /// Build a tiny resolved chart and render it to an SVG via the real backend.
    fn sample_svg() -> String {
        let chart = ResolvedChart {
            mark: Mark::Line,
            series: vec![Series {
                name: "rev".to_string(),
                points: vec![(0.0, 1.0), (1.0, 3.0), (2.0, 2.0)],
                sizes: Vec::new(),
            }],
            slices: Vec::new(),
            x_axis: Axis {
                title: "month".to_string(),
                kind: AxisKind::Quantitative,
                scale: hiker_charts_core::dsl::Scale::default(),
            },
            y_axis: Axis {
                title: "revenue".to_string(),
                kind: AxisKind::Quantitative,
                scale: hiker_charts_core::dsl::Scale::default(),
            },
            config: Config { title: Some("Sales".to_string()), ..Config::default() },
        };
        let size = Size { width: 320, height: 240 };
        PlottersSvg
            .render(&chart, &Theme::default(), size)
            .expect("render sample chart")
            .svg
    }

    #[test]
    fn rasterizes_chart_to_expected_dimensions() {
        let svg = sample_svg();
        let img = rasterize(&svg, 1.0).expect("rasterize svg");
        assert_eq!(img.size, [320, 240]);
    }

    #[test]
    fn scale_multiplies_dimensions() {
        let svg = sample_svg();
        let img = rasterize(&svg, 2.0).expect("rasterize svg");
        assert_eq!(img.size, [640, 480]);
    }

    #[test]
    fn produces_non_transparent_pixels() {
        let svg = sample_svg();
        let img = rasterize(&svg, 1.0).expect("rasterize svg");
        assert!(img.pixels.iter().any(|p| p.a() > 0), "no opaque pixels drawn");
    }

    #[test]
    fn rejects_invalid_svg() {
        assert!(rasterize("not an svg at all", 1.0).is_none());
    }
}
