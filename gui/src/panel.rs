//! The comfy builder panel: column-populated dropdowns driving `BuilderState`.
//!
//! [`panel`] lays a column of controls beside the interactive [`preview`]
//! widget. Every control is a `BuilderState` transition, so the spec a panel
//! builds is byte-identical to a hand-authored one (SPEC §8.2): a `ComboBox` for
//! the mark; `x` and `color` field pickers populated from `state.columns()` (a
//! column can never be mistyped) each with a small type `ComboBox` offering
//! "Auto" plus the `DataType` variants and defaulting to the inferred type; a
//! `y` multiselect of checkboxes via `add_y`/`remove_y`; a "Series from"
//! wide/long toggle calling `set_series_mode` (SPEC §2.3.1); a title `TextEdit`;
//! a legend checkbox; and an Export button that yields `state.to_block()` for
//! the host to put on the clipboard. The function is factored into per-section
//! sub-functions to stay within the length and cognitive-complexity budgets.

use egui::ComboBox;
use hiker_charts_core::dsl::{DataType, Mark};

use crate::model::{BuilderState, Channel, SeriesMode};
use crate::preview::{preview, View};

/// All marks, in dropdown order, with their human labels.
const MARKS: [(Mark, &str); 4] =
    [(Mark::Bar, "Bar"), (Mark::Line, "Line"), (Mark::Point, "Point"), (Mark::Area, "Area")];

/// All declarable data types, in dropdown order, with their human labels.
const TYPES: [(DataType, &str); 4] = [
    (DataType::Quantitative, "Quantitative"),
    (DataType::Temporal, "Temporal"),
    (DataType::Ordinal, "Ordinal"),
    (DataType::Nominal, "Nominal"),
];

/// Render the whole builder: the control column on the left and the interactive
/// preview filling the rest. Returns the exported ```chart block when the Export
/// button is pressed this frame (the host copies it to the clipboard), else
/// `None`. Every control routes through a `BuilderState` transition.
pub fn panel(
    state: &mut BuilderState,
    camera: &mut crate::camera::Camera,
    view: &mut View,
    ui: &mut egui::Ui,
) -> Option<String> {
    let mut exported = None;
    ui.horizontal_top(|ui| {
        // Fixed-width control column: an explicit allocation caps its width so the
        // full-width `separator()`s inside it can't expand the column to fill the
        // row and starve the preview of space.
        let column = egui::vec2(240.0, ui.available_height());
        ui.allocate_ui_with_layout(column, egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_width(240.0);
            mark_combo(state, ui);
            ui.separator();
            field_picker(state, Channel::X, "X axis", ui);
            field_picker(state, Channel::Color, "Color", ui);
            ui.separator();
            y_multiselect(state, ui);
            ui.separator();
            series_mode_toggle(state, ui);
            ui.separator();
            config_controls(state, ui);
            ui.separator();
            if ui.button("Export chart block").clicked() {
                exported = Some(state.to_block());
            }
        });
        ui.separator();
        let _ = preview(state, camera, view, ui);
    });
    exported
}

/// The mark `ComboBox`. Selecting a mark calls `set_mark`.
fn mark_combo(state: &mut BuilderState, ui: &mut egui::Ui) {
    let current = MARKS.iter().find(|(m, _)| *m == state.spec().mark).map_or("Bar", |(_, l)| *l);
    ComboBox::from_label("Mark").selected_text(current).show_ui(ui, |ui| {
        for (mark, label) in MARKS {
            if ui.selectable_label(state.spec().mark == mark, label).clicked() {
                state.set_mark(mark);
            }
        }
    });
}

/// A single-field channel picker (x or color): a column `ComboBox` plus a type
/// `ComboBox`. Selecting a column calls `set_x`/`set_color`; selecting a type
/// calls `set_field_type` (Auto clears the declared type, falling back to the
/// inferred one).
fn field_picker(state: &mut BuilderState, channel: Channel, label: &str, ui: &mut egui::Ui) {
    let current = current_field(state, channel).map(str::to_string);
    let selected = current.as_deref().unwrap_or("(none)");
    ui.horizontal(|ui| {
        ComboBox::from_id_salt((label, "col")).selected_text(selected).show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), "(none)").clicked() {
                set_field(state, channel, None);
            }
            let columns: Vec<String> = state.columns().iter().map(|c| (*c).to_string()).collect();
            for col in columns {
                if ui.selectable_label(current.as_deref() == Some(&col), &col).clicked() {
                    set_field(state, channel, Some(&col));
                }
            }
        });
        ui.label(label);
    });
    if let Some(col) = current.as_deref() {
        type_combo(state, channel, col, ui);
    }
}

/// The per-field type `ComboBox` for a bound channel. The default shown is the
/// declared type, or "Auto" labeled with the inferred type when none is declared.
fn type_combo(state: &mut BuilderState, channel: Channel, column: &str, ui: &mut egui::Ui) {
    let declared = current_type(state, channel);
    let inferred = state.inferred_type(column);
    let selected = match declared {
        Some(ty) => type_label(ty).to_string(),
        None => format!("Auto ({})", inferred.map_or("?", type_label)),
    };
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ComboBox::from_id_salt((column, channel_tag(channel), "ty")).selected_text(selected).show_ui(
            ui,
            |ui| {
                if ui.selectable_label(declared.is_none(), "Auto").clicked() {
                    state.set_field_type(channel, None);
                }
                for (ty, lbl) in TYPES {
                    if ui.selectable_label(declared == Some(ty), lbl).clicked() {
                        state.set_field_type(channel, Some(ty));
                    }
                }
            },
        );
    });
}

/// The `y` multiselect: a checkbox per column, toggling `add_y`/`remove_y`.
fn y_multiselect(state: &mut BuilderState, ui: &mut egui::Ui) {
    ui.label("Y values");
    let active: Vec<String> =
        state.spec().y_fields().into_iter().map(|(n, _)| n.to_string()).collect();
    let columns: Vec<String> = state.columns().iter().map(|c| (*c).to_string()).collect();
    for col in columns {
        let mut on = active.iter().any(|f| f == &col);
        if ui.checkbox(&mut on, &col).changed() {
            if on {
                state.add_y(&col);
            } else {
                state.remove_y(&col);
            }
        }
    }
}

/// The wide/long series toggle. Wide draws series from the `y` columns; long
/// splits a single `y` value by the values in the color column. Both call
/// `set_series_mode` (SPEC §2.3.1).
fn series_mode_toggle(state: &mut BuilderState, ui: &mut egui::Ui) {
    ui.label("Series from");
    let is_long = state.spec().color_field().is_some();
    ui.horizontal(|ui| {
        if ui.selectable_label(!is_long, "columns").clicked() {
            state.set_series_mode(SeriesMode::Wide, None);
        }
        if ui.selectable_label(is_long, "values in a column").clicked() {
            let split = state.columns().first().map(|s| (*s).to_string());
            state.set_series_mode(SeriesMode::Long, split.as_deref());
        }
    });
}

/// The title `TextEdit` and the legend checkbox, both routed through transitions.
fn config_controls(state: &mut BuilderState, ui: &mut egui::Ui) {
    let mut title = state.spec().config.title.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Title");
        if ui.text_edit_singleline(&mut title).changed() {
            state.set_title(if title.is_empty() { None } else { Some(&title) });
        }
    });
    let mut legend = state.spec().config.legend;
    if ui.checkbox(&mut legend, "Legend").changed() {
        state.toggle_legend();
    }
}

/// The currently bound field name for a single-field channel, if any.
fn current_field(state: &BuilderState, channel: Channel) -> Option<&str> {
    match channel {
        Channel::X => state.spec().x_field().map(|(n, _)| n),
        Channel::Color => state.spec().color_field().map(|(n, _)| n),
    }
}

/// The declared type for a single-field channel, if one is declared.
fn current_type(state: &BuilderState, channel: Channel) -> Option<DataType> {
    match channel {
        Channel::X => state.spec().x_field().and_then(|(_, t)| t),
        Channel::Color => state.spec().color_field().and_then(|(_, t)| t),
    }
}

/// Apply a column binding to a single-field channel.
fn set_field(state: &mut BuilderState, channel: Channel, column: Option<&str>) {
    match channel {
        Channel::X => state.set_x(column),
        Channel::Color => state.set_color(column),
    }
}

/// A stable string tag for a channel, used to salt egui widget ids.
const fn channel_tag(channel: Channel) -> &'static str {
    match channel {
        Channel::X => "x",
        Channel::Color => "color",
    }
}

/// The human label for a data type.
const fn type_label(ty: DataType) -> &'static str {
    match ty {
        DataType::Quantitative => "Quantitative",
        DataType::Temporal => "Temporal",
        DataType::Ordinal => "Ordinal",
        DataType::Nominal => "Nominal",
    }
}

#[cfg(test)]
mod tests {
    use super::panel;
    use crate::camera::Camera;
    use crate::model::BuilderState;
    use crate::preview::View;
    use hiker_charts_core::backend::Size;
    use hiker_charts_core::data::Table;
    use hiker_charts_core::dsl::ChartSpec;
    use hiker_charts_core::theme::Theme;

    fn state() -> BuilderState {
        let spec = ChartSpec::from_yaml("mark: line\nx: month\ny: revenue\n").unwrap();
        let table = Table::from_csv(b"month,revenue,profit\njan,100,20\nfeb,140,35\n").unwrap();
        BuilderState::new(spec, table, Theme::default(), Size { width: 320, height: 240 })
    }

    #[test]
    fn panel_runs_a_frame_without_panicking() {
        let mut s = state();
        let mut cam = Camera::default();
        let mut view = View::new();
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = panel(&mut s, &mut cam, &mut view, ui);
            });
        });
    }

    #[test]
    fn preview_gets_real_space_beside_controls() {
        // Regression: the control column must not expand to fill the row and
        // starve the preview. With a real screen rect the preview gets width, so
        // its first-frame auto-fit runs and moves the camera off the default.
        let mut s = state();
        let mut cam = Camera::default();
        let mut view = View::new();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = panel(&mut s, &mut cam, &mut view, ui);
            });
        });
        assert!(
            (cam.scale() - 1.0).abs() > f32::EPSILON,
            "preview should have received space and auto-fit the chart (scale {})",
            cam.scale()
        );
    }

    #[test]
    fn export_returns_a_chart_block() {
        // The button isn't clicked in a headless frame, so exercise the export
        // path directly: the panel returns exactly `state.to_block()`.
        let s = state();
        let block = s.to_block();
        assert!(block.starts_with("```chart\n"));
        assert!(block.contains("mark: line"));
        assert!(block.ends_with("```"));
    }
}
