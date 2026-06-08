//! Windowed eframe demo of the `hiker-charts` comfy builder.
//!
//! Opens a native window hosting `hiker_charts_gui::panel`, seeded from a gallery of bundled
//! `examples/*.csv` + `*.chart.yaml` pairs picked from a dataset dropdown at the top, so the
//! dropdowns, live preview, and pan / pinch-zoom can be tried interactively against a range of
//! data shapes (line, bars, scatter, area, histogram, donut, …). It mirrors Hiker's host setup —
//! eframe 0.32 with the forked winit patched in at the workspace root — so Wayland touchpad
//! pinch/magnify drives the preview camera the same way it will inside the app. Clicking Export
//! copies the `chart` block to the clipboard.

use eframe::egui;
use hiker_charts_core::backend::Size;
use hiker_charts_core::block::parse_block;
use hiker_charts_core::data::Table;
use hiker_charts_core::dsl::ChartSpec;
use hiker_charts_core::theme::Theme;
use hiker_charts_gui::camera::Camera;
use hiker_charts_gui::model::BuilderState;
use hiker_charts_gui::panel::{panel, ThemeChoice};
use hiker_charts_gui::preview::View;

/// Where a preset's data + starting spec come from. Both forms are compiled in via
/// `include_str!` so the demo runs from any working directory with no files on disk.
enum Source {
    /// A separate CSV file and a separate `*.chart.yaml` spec file.
    Pair { csv: &'static str, yaml: &'static str },
    /// One self-contained chart block: YAML config, a `---` line, then inline CSV —
    /// exactly the shape a host like Hiker embeds in a note (SPEC §8.3).
    Block(&'static str),
}

/// A bundled demo dataset: a human label plus its data source.
struct Preset {
    /// Label shown in the dataset dropdown.
    name: &'static str,
    /// How the dataset's data and starting spec are supplied.
    source: Source,
}

/// Convenience for one `examples/<stem>` pair: the CSV at `examples/<csv stem>.csv`
/// and the spec at `examples/<spec stem>.chart.yaml`.
macro_rules! pair {
    ($name:literal, $csv_stem:literal, $spec_stem:literal) => {
        Preset {
            name: $name,
            source: Source::Pair {
                csv: include_str!(concat!("../../examples/", $csv_stem, ".csv")),
                yaml: include_str!(concat!("../../examples/", $spec_stem, ".chart.yaml")),
            },
        }
    };
}

/// Convenience for one self-contained `examples/<stem>.chart` block (config + `---` + CSV).
macro_rules! block {
    ($name:literal, $stem:literal) => {
        Preset {
            name: $name,
            source: Source::Block(include_str!(concat!("../../examples/", $stem, ".chart"))),
        }
    };
}

/// The gallery of datasets + starting chart configs offered in the dropdown. Several
/// share `sales.csv` to show how one dataset reshapes across marks; the rest bring
/// their own data tuned to a particular chart type. The last entry is a single
/// self-contained block with its CSV defined inline.
const PRESETS: &[Preset] = &[
    pair!("Monthly sales — line", "sales", "sales"),
    pair!("Revenue vs profit — grouped bars", "sales", "bars"),
    pair!("Monthly sales — stacked bars", "sales", "stacked-bars"),
    pair!("Monthly sales — step line", "sales", "step-line"),
    pair!("Revenue vs month — bubble", "sales", "bubble"),
    pair!("Revenue — log y axis", "sales", "log-scale"),
    pair!("Revenue share — donut", "sales", "donut"),
    pair!("Iris — scatter (color + size)", "iris", "iris"),
    pair!("Web traffic — stacked area", "traffic", "traffic"),
    pair!("Daily temperature — area band", "weather", "weather"),
    pair!("Exam scores — histogram", "scores", "scores"),
    pair!("Browser share — donut", "browsers", "browsers"),
    pair!("World population — horizontal bars", "population", "population"),
    block!("Signups vs churn — inline CSV", "inline"),
];

/// The canvas size every preset renders at.
const CANVAS: Size = Size { width: 800, height: 500 };

/// The persistent demo state the host carries across frames: the selected preset, the
/// builder model, the preview camera (pan/zoom), and the preview's cached texture handle.
struct Demo {
    /// Index into [`PRESETS`] of the dataset currently loaded.
    selected: usize,
    state: BuilderState,
    theme: ThemeChoice,
    camera: Camera,
    view: View,
}

impl Demo {
    /// Seed the builder with the first bundled preset.
    fn seeded() -> Self {
        let theme = ThemeChoice::default();
        Self {
            selected: 0,
            state: load_preset(0, theme),
            theme,
            camera: Camera::default(),
            view: View::new(),
        }
    }

    /// Swap in preset `idx`: rebuild the builder state from its CSV + spec, keep the
    /// current global theme, and reset the camera so the new chart fits the preview.
    fn select(&mut self, idx: usize) {
        if idx == self.selected {
            return;
        }
        self.selected = idx;
        self.state = load_preset(idx, self.theme);
        self.camera = Camera::default();
    }
}

/// Build a fresh `BuilderState` from preset `idx`, carrying the host's current theme
/// choice so a dataset switch doesn't drop the user's light/dark + palette selection.
/// A `Pair` preset parses its separate CSV + spec; a `Block` preset parses one
/// self-contained block (config + `---` + inline CSV) through the core block parser —
/// the same path a host uses for an inline chart block.
fn load_preset(idx: usize, theme: ThemeChoice) -> BuilderState {
    let resolved = Theme::from_dark_mode(theme.dark).with_palette(theme.palette);
    match &PRESETS[idx].source {
        Source::Pair { csv, yaml } => {
            let table = Table::from_csv(csv.as_bytes()).expect("bundled preset csv parses");
            let spec = ChartSpec::from_yaml(yaml).expect("bundled preset spec parses");
            BuilderState::new(spec, table, resolved, CANVAS)
        }
        Source::Block(body) => {
            let parsed = parse_block(body).expect("bundled inline block parses");
            let table = parsed.table.expect("inline block carries its CSV");
            BuilderState::new(parsed.spec, table, resolved, CANVAS)
        }
    }
}

impl eframe::App for Demo {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // A slim top bar: pick the dataset/starting-config to load into the builder below.
        egui::TopBottomPanel::top("dataset_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("Dataset:");
                let mut chosen = self.selected;
                egui::ComboBox::from_id_salt("dataset_picker")
                    .selected_text(PRESETS[self.selected].name)
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for (i, preset) in PRESETS.iter().enumerate() {
                            ui.selectable_value(&mut chosen, i, preset.name);
                        }
                    });
                self.select(chosen);
                ui.separator();
                ui.weak("Pick a dataset, then tweak its mark, encodings, and style in the panel below.");
            });
            ui.add_space(2.0);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            let exported =
                panel(&mut self.state, &mut self.theme, &mut self.camera, &mut self.view, ui);
            if let Some(block) = exported {
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
