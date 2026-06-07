//! Windowed eframe demo of the `hiker-charts` comfy builder.
//!
//! Opens a native window hosting `hiker_charts_gui::panel`, seeded with the bundled
//! `examples/sales.csv` + spec, so the dropdowns, live preview, and pan / pinch-zoom can be
//! tried interactively. It mirrors Hiker's host setup — eframe 0.32 with the forked winit
//! patched in at the workspace root — so Wayland touchpad pinch/magnify drives the preview
//! camera the same way it will inside the app. Clicking Export copies the `chart` block to
//! the clipboard.

use eframe::egui;
use hiker_charts_core::backend::Size;
use hiker_charts_core::data::Table;
use hiker_charts_core::dsl::ChartSpec;
use hiker_charts_core::theme::Theme;
use hiker_charts_gui::camera::Camera;
use hiker_charts_gui::model::BuilderState;
use hiker_charts_gui::panel::panel;
use hiker_charts_gui::preview::View;

/// Sample data + spec compiled in so the demo runs from any working directory.
const SAMPLE_CSV: &str = include_str!("../../examples/sales.csv");
const SAMPLE_SPEC: &str = include_str!("../../examples/sales.chart.yaml");

/// The persistent demo state the host carries across frames: the builder model, the
/// preview camera (pan/zoom), and the preview's cached texture handle.
struct Demo {
    state: BuilderState,
    camera: Camera,
    view: View,
}

impl Demo {
    /// Seed the builder with the bundled sample CSV + chart spec.
    fn seeded() -> Self {
        let table = Table::from_csv(SAMPLE_CSV.as_bytes()).expect("bundled sample csv parses");
        let spec = ChartSpec::from_yaml(SAMPLE_SPEC).expect("bundled sample spec parses");
        let size = Size { width: 800, height: 500 };
        Self {
            state: BuilderState::new(spec, table, Theme::default(), size),
            camera: Camera::default(),
            view: View::new(),
        }
    }
}

impl eframe::App for Demo {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(block) = panel(&mut self.state, &mut self.camera, &mut self.view, ui) {
                ctx.copy_text(block);
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "hiker-charts builder",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(Demo::seeded()))
        }),
    )
}
