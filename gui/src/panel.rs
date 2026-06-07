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
use hiker_charts_core::dsl::{DataType, Interpolate, Mark, Orientation, Scale, ScaleKind};
use hiker_charts_core::registry::caps;
use hiker_charts_core::theme::{Color, Palette, Theme};

use crate::model::{Axis, BuilderState, Channel, SeriesMode};
use crate::preview::{preview, View};

/// All marks, in dropdown order, with their human labels.
const MARKS: [(Mark, &str); 6] = [
    (Mark::Bar, "Bar"),
    (Mark::Line, "Line"),
    (Mark::Point, "Point"),
    (Mark::Area, "Area"),
    (Mark::Histogram, "Histogram"),
    (Mark::Arc, "Arc"),
];

/// The axis scale kinds, in dropdown order, with their human labels.
const SCALE_KINDS: [(ScaleKind, &str); 3] =
    [(ScaleKind::Linear, "Linear"), (ScaleKind::Log, "Log"), (ScaleKind::Sqrt, "Sqrt")];

/// All declarable data types, in dropdown order, with their human labels.
const TYPES: [(DataType, &str); 4] = [
    (DataType::Quantitative, "Quantitative"),
    (DataType::Temporal, "Temporal"),
    (DataType::Ordinal, "Ordinal"),
    (DataType::Nominal, "Nominal"),
];

/// All selectable palettes, in dropdown order, with their human labels.
const PALETTES: [(Palette, &str); 5] = [
    (Palette::Category10, "Category10"),
    (Palette::Pastel, "Pastel"),
    (Palette::Warm, "Warm"),
    (Palette::Cool, "Cool"),
    (Palette::Mono, "Mono"),
];

/// The host-held global theme selection, carried across frames so the radio +
/// palette dropdown stay in sync. The panel applies it via `BuilderState::set_theme`
/// whenever it changes (`Theme::from_dark_mode(dark).with_palette(palette)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeChoice {
    /// Whether the dark preset is selected (else light).
    pub dark: bool,
    /// The categorical palette applied globally to series colors.
    pub palette: Palette,
}

impl Default for ThemeChoice {
    /// Light preset with the strong `Category10` palette — the `BuilderState`
    /// default theme.
    fn default() -> Self {
        Self { dark: false, palette: Palette::Category10 }
    }
}

impl ThemeChoice {
    /// Build the `Theme` this choice represents.
    fn theme(self) -> Theme {
        Theme::from_dark_mode(self.dark).with_palette(self.palette)
    }
}

/// Render the whole builder: the control column on the left and the interactive
/// preview filling the rest. Returns the exported ```chart block when the Export
/// button is pressed this frame (the host copies it to the clipboard), else
/// `None`. Every control routes through a `BuilderState` transition.
pub fn panel(
    state: &mut BuilderState,
    theme: &mut ThemeChoice,
    camera: &mut crate::camera::Camera,
    view: &mut View,
    ui: &mut egui::Ui,
) -> Option<String> {
    let mut exported = None;
    ui.horizontal_top(|ui| {
        // Fixed-width control column: an explicit allocation caps its width so the
        // full-width `separator()`s inside it can't expand the column to fill the
        // row and starve the preview of space. The column now holds many controls,
        // so its contents scroll vertically and never overflow the window.
        let column = egui::vec2(240.0, ui.available_height());
        ui.allocate_ui_with_layout(column, egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_width(240.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                exported = control_column(state, theme, ui);
            });
        });
        ui.separator();
        let _ = preview(state, camera, view, ui);
    });
    exported
}

/// The scrollable stack of every control, top to bottom, gated by the mark's
/// capabilities from the registry so only applicable controls appear. Returns the
/// exported block when the Export button is clicked this frame, else `None`.
fn control_column(
    state: &mut BuilderState,
    theme: &mut ThemeChoice,
    ui: &mut egui::Ui,
) -> Option<String> {
    // The single source of truth: which channels/options this mark supports
    // (IMPLEMENTATION §17.6). Every section below is gated on it; switching marks
    // only hides controls — the underlying spec values are preserved.
    let mut exported = None;
    mark_combo(state, ui);
    ui.separator();
    channel_section(state, ui);
    ui.separator();
    options_section(state, ui);
    if caps(state.spec().mark).options.scales {
        ui.separator();
        scales_section(state, ui);
    }
    if caps(state.spec().mark).cartesian {
        ui.separator();
        colors_section(state, ui);
    }
    ui.separator();
    theme_section(state, theme, ui);
    ui.separator();
    if ui.button("Export chart block").clicked() {
        exported = Some(state.to_block());
    }
    exported
}

/// The encoding-channel pickers, each gated by `caps.channels`: x, color, the y
/// multiselect, a bubble-size picker, and an arc-theta picker. Channels the mark
/// does not use are hidden (the spec value, if any, stays for forward-compat). The
/// wide/long series toggle is shown only for cartesian multi-series marks (those
/// with both a y and a color channel).
fn channel_section(state: &mut BuilderState, ui: &mut egui::Ui) {
    let channels = caps(state.spec().mark).channels;
    if channels.x {
        field_picker(state, Channel::X, "X axis", ui);
    }
    if channels.color {
        field_picker(state, Channel::Color, "Color", ui);
    }
    if channels.size {
        simple_field_picker(state, SimpleChannel::Size, "Size", ui);
    }
    if channels.theta {
        simple_field_picker(state, SimpleChannel::Theta, "Theta", ui);
    }
    if channels.y {
        ui.separator();
        y_multiselect(state, ui);
    }
    if channels.y && channels.color {
        ui.separator();
        series_mode_toggle(state, ui);
    }
}

/// Title/legend config plus the mark's option widgets, each gated by `caps.options`:
/// stacking, interpolation, point size, line width, fill opacity, orientation, bins,
/// and donut inner-radius. The always-useful titles and the gridline toggle (for
/// cartesian marks) live here too.
fn options_section(state: &mut BuilderState, ui: &mut egui::Ui) {
    config_controls(state, ui);
    let opts = caps(state.spec().mark).options;
    if opts.stack {
        stack_checkbox(state, ui);
    }
    if opts.interpolate {
        interpolate_toggle(state, ui);
    }
    if opts.point_size {
        point_size_slider(state, ui);
    }
    if opts.line_width {
        line_width_slider(state, ui);
    }
    if opts.fill_opacity {
        fill_opacity_slider(state, ui);
    }
    if opts.orientation {
        orientation_toggle(state, ui);
    }
    if opts.bins {
        bins_drag(state, ui);
    }
    if opts.inner_radius {
        inner_radius_slider(state, ui);
    }
    if caps(state.spec().mark).cartesian {
        grid_checkbox(state, ui);
    }
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

/// A typeless single-field channel: which of size/theta a [`simple_field_picker`]
/// drives. These bind a bare column (no per-field type combo) via `set_size`/`set_theta`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SimpleChannel {
    Size,
    Theta,
}

/// A column picker for a typeless channel (bubble `size` or arc `theta`): a single
/// `ComboBox` of `(none)` plus the columns, with no type sub-combo. Selecting routes
/// through `set_size`/`set_theta`.
fn simple_field_picker(
    state: &mut BuilderState,
    channel: SimpleChannel,
    label: &str,
    ui: &mut egui::Ui,
) {
    let current = simple_current(state, channel).map(str::to_string);
    let selected = current.as_deref().unwrap_or("(none)");
    ui.horizontal(|ui| {
        ComboBox::from_id_salt((label, "simple")).selected_text(selected).show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), "(none)").clicked() {
                simple_set(state, channel, None);
            }
            let columns: Vec<String> = state.columns().iter().map(|c| (*c).to_string()).collect();
            for col in columns {
                if ui.selectable_label(current.as_deref() == Some(&col), &col).clicked() {
                    simple_set(state, channel, Some(&col));
                }
            }
        });
        ui.label(label);
    });
}

/// The currently bound column for a typeless channel, if any.
fn simple_current(state: &BuilderState, channel: SimpleChannel) -> Option<&str> {
    match channel {
        SimpleChannel::Size => state.spec().size_field().map(|(n, _)| n),
        SimpleChannel::Theta => state.spec().theta_field().map(|(n, _)| n),
    }
}

/// Apply a column binding to a typeless channel.
fn simple_set(state: &mut BuilderState, channel: SimpleChannel, column: Option<&str>) {
    match channel {
        SimpleChannel::Size => state.set_size(column),
        SimpleChannel::Theta => state.set_theta(column),
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

/// The title/axis-title text fields and the legend checkbox, all routed through
/// transitions. An empty text field clears the title to `None`.
fn config_controls(state: &mut BuilderState, ui: &mut egui::Ui) {
    let mut title = state.spec().config.title.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Title");
        if ui.text_edit_singleline(&mut title).changed() {
            state.set_title(opt(&title));
        }
    });
    let mut x_title = state.spec().config.x_title.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("X title");
        if ui.text_edit_singleline(&mut x_title).changed() {
            state.set_x_title(opt(&x_title));
        }
    });
    let mut y_title = state.spec().config.y_title.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Y title");
        if ui.text_edit_singleline(&mut y_title).changed() {
            state.set_y_title(opt(&y_title));
        }
    });
    let mut legend = state.spec().config.legend;
    if ui.checkbox(&mut legend, "Legend").changed() {
        state.toggle_legend();
    }
}

/// The interior-gridline checkbox (cartesian marks), routed through `set_show_grid`.
fn grid_checkbox(state: &mut BuilderState, ui: &mut egui::Ui) {
    let mut grid = state.spec().config.show_grid;
    if ui.checkbox(&mut grid, "Gridlines").changed() {
        state.set_show_grid(grid);
    }
}

/// The bar/area orientation Vertical/Horizontal toggle, routed through `set_orientation`.
/// `None` (the vertical default) is shown as Vertical.
fn orientation_toggle(state: &mut BuilderState, ui: &mut egui::Ui) {
    let is_horizontal = state.spec().config.orientation == Some(Orientation::Horizontal);
    ui.label("Orientation");
    ui.horizontal(|ui| {
        if ui.selectable_label(!is_horizontal, "Vertical").clicked() {
            state.set_orientation(Some(Orientation::Vertical));
        }
        if ui.selectable_label(is_horizontal, "Horizontal").clicked() {
            state.set_orientation(Some(Orientation::Horizontal));
        }
    });
}

/// The histogram bin-count `DragValue`, routed through `set_bins`. The resolver's
/// default of 20 is shown until the user changes it.
fn bins_drag(state: &mut BuilderState, ui: &mut egui::Ui) {
    let mut bins = state.spec().config.bins.unwrap_or(20);
    ui.horizontal(|ui| {
        ui.label("Bins");
        if ui.add(egui::DragValue::new(&mut bins).range(1..=200).speed(0.5)).changed() {
            state.set_bins(Some(bins));
        }
    });
}

/// The donut inner-radius slider over `0.0..=1.0` (`Arc`), routed through
/// `set_inner_radius`. 0 draws a full pie; higher values cut a wider hole.
fn inner_radius_slider(state: &mut BuilderState, ui: &mut egui::Ui) {
    let mut radius = state.spec().config.inner_radius.unwrap_or(0.0);
    ui.horizontal(|ui| {
        ui.label("Inner radius");
        if ui.add(egui::Slider::new(&mut radius, 0.0..=0.9)).changed() {
            state.set_inner_radius(Some(radius));
        }
    });
}

/// The stack checkbox (bar/area), routed through `set_stack`.
fn stack_checkbox(state: &mut BuilderState, ui: &mut egui::Ui) {
    let mut stack = state.spec().config.stack;
    if ui.checkbox(&mut stack, "Stack series").changed() {
        state.set_stack(stack);
    }
}

/// The line interpolation Linear/Step toggle, routed through `set_interpolate`.
/// `None` (the linear default) is shown as Linear.
fn interpolate_toggle(state: &mut BuilderState, ui: &mut egui::Ui) {
    let is_step = state.spec().config.interpolate == Some(Interpolate::Step);
    ui.label("Interpolate");
    ui.horizontal(|ui| {
        if ui.selectable_label(!is_step, "Linear").clicked() {
            state.set_interpolate(Some(Interpolate::Linear));
        }
        if ui.selectable_label(is_step, "Step").clicked() {
            state.set_interpolate(Some(Interpolate::Step));
        }
    });
}

/// The point radius `DragValue` (point mark), routed through `set_point_size`.
fn point_size_slider(state: &mut BuilderState, ui: &mut egui::Ui) {
    let mut size = state.spec().config.point_size.unwrap_or(3.0);
    ui.horizontal(|ui| {
        ui.label("Point size");
        if ui.add(egui::DragValue::new(&mut size).range(1.0..=20.0).speed(0.2)).changed() {
            state.set_point_size(Some(size));
        }
    });
}

/// The line/area stroke-width slider, routed through `set_line_width`.
fn line_width_slider(state: &mut BuilderState, ui: &mut egui::Ui) {
    let mut width = state.spec().config.line_width.unwrap_or(1.5);
    ui.horizontal(|ui| {
        ui.label("Line width");
        if ui.add(egui::Slider::new(&mut width, 0.5..=10.0)).changed() {
            state.set_line_width(Some(width));
        }
    });
}

/// The area/bar fill-opacity slider over `0.0..=1.0`, routed through
/// `set_fill_opacity`.
fn fill_opacity_slider(state: &mut BuilderState, ui: &mut egui::Ui) {
    let mut opacity = state.spec().config.fill_opacity.unwrap_or(0.5);
    ui.horizontal(|ui| {
        ui.label("Fill opacity");
        if ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0)).changed() {
            state.set_fill_opacity(Some(opacity));
        }
    });
}

/// The per-series color section: one color button per resolved series name (writing
/// `set_series_color`) plus a "Reset colors" button calling `clear_palette`.
fn colors_section(state: &mut BuilderState, ui: &mut egui::Ui) {
    ui.label("Series colors");
    for (i, name) in state.series_names().into_iter().enumerate() {
        ui.horizontal(|ui| {
            let mut rgb = color_to_rgb(state.effective_series_color(i));
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                state.set_series_color(i, rgb_to_color(rgb));
            }
            ui.label(name);
        });
    }
    if ui.button("Reset colors").clicked() {
        state.clear_palette();
    }
}

/// The per-axis scales section (gated on `caps.options.scales`): for each cartesian
/// axis a kind combo (Linear/Log/Sqrt), an optional `(min, max)` domain, and an
/// include-zero checkbox. Drawn only for cartesian marks.
fn scales_section(state: &mut BuilderState, ui: &mut egui::Ui) {
    ui.label("Scales");
    scale_axis_controls(state, Axis::X, "X", ui);
    scale_axis_controls(state, Axis::Y, "Y", ui);
}

/// One axis's scale controls: a kind `ComboBox`, a domain auto/manual toggle with two
/// `DragValue`s, and an include-zero checkbox. Each routes through the model's scale
/// transitions (`set_scale_kind`/`set_scale_domain`/`set_scale_zero`).
fn scale_axis_controls(state: &mut BuilderState, axis: Axis, label: &str, ui: &mut egui::Ui) {
    let scale = current_scale(state, axis);
    let current_kind =
        SCALE_KINDS.iter().find(|(k, _)| *k == scale.kind).map_or("Linear", |(_, l)| *l);
    ui.horizontal(|ui| {
        ComboBox::from_id_salt((label, "scale-kind")).selected_text(current_kind).show_ui(ui, |ui| {
            for (kind, lbl) in SCALE_KINDS {
                if ui.selectable_label(scale.kind == kind, lbl).clicked() {
                    state.set_scale_kind(axis, kind);
                }
            }
        });
        ui.label(format!("{label} scale"));
    });
    domain_controls(state, axis, scale.domain, ui);
    let mut zero = scale.zero;
    if ui.checkbox(&mut zero, format!("{label}: include zero")).changed() {
        state.set_scale_zero(axis, zero);
    }
}

/// The auto/manual domain row for one axis: a checkbox flips between an auto domain
/// (`None`) and an explicit `(min, max)`; when manual, two `DragValue`s edit the bounds.
fn domain_controls(
    state: &mut BuilderState,
    axis: Axis,
    domain: Option<(f64, f64)>,
    ui: &mut egui::Ui,
) {
    let mut manual = domain.is_some();
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        if ui.checkbox(&mut manual, "manual domain").changed() {
            state.set_scale_domain(axis, manual.then_some((0.0, 1.0)));
        }
    });
    if let Some((mut lo, mut hi)) = domain {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let lo_changed = ui.add(egui::DragValue::new(&mut lo).prefix("min ")).changed();
            let hi_changed = ui.add(egui::DragValue::new(&mut hi).prefix("max ")).changed();
            if lo_changed || hi_changed {
                state.set_scale_domain(axis, Some((lo, hi)));
            }
        });
    }
}

/// The GLOBAL theme section: a Light/Dark selectable pair and a palette dropdown.
/// Any change rebuilds the theme via `ThemeChoice` and applies it with `set_theme`.
fn theme_section(state: &mut BuilderState, choice: &mut ThemeChoice, ui: &mut egui::Ui) {
    ui.label("Theme (global)");
    let before = *choice;
    ui.horizontal(|ui| {
        if ui.selectable_label(!choice.dark, "Light").clicked() {
            choice.dark = false;
        }
        if ui.selectable_label(choice.dark, "Dark").clicked() {
            choice.dark = true;
        }
    });
    let current = PALETTES.iter().find(|(p, _)| *p == choice.palette).map_or("Category10", |(_, l)| *l);
    ComboBox::from_label("Palette").selected_text(current).show_ui(ui, |ui| {
        for (palette, label) in PALETTES {
            if ui.selectable_label(choice.palette == palette, label).clicked() {
                choice.palette = palette;
            }
        }
    });
    if *choice != before {
        state.set_theme(choice.theme());
    }
}

/// Map an empty string to `None`, anything else to `Some`, for clearable titles.
const fn opt(s: &str) -> Option<&str> {
    if s.is_empty() { None } else { Some(s) }
}

/// Convert a core `Color` to egui's `[u8; 3]` sRGB triple for a color button.
const fn color_to_rgb(c: Color) -> [u8; 3] {
    [c.r, c.g, c.b]
}

/// Convert an egui `[u8; 3]` sRGB triple back to an opaque core `Color`.
const fn rgb_to_color(rgb: [u8; 3]) -> Color {
    Color::rgb(rgb[0], rgb[1], rgb[2])
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

/// The current scale for a cartesian axis, or the linear default when none is set, so
/// the controls always have a complete value to read.
fn current_scale(state: &BuilderState, axis: Axis) -> Scale {
    match axis {
        Axis::X => state.spec().config.x_scale,
        Axis::Y => state.spec().config.y_scale,
    }
    .unwrap_or_default()
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
    use super::{panel, ThemeChoice};
    use crate::camera::Camera;
    use crate::model::BuilderState;
    use crate::preview::View;
    use hiker_charts_core::backend::Size;
    use hiker_charts_core::data::Table;
    use hiker_charts_core::dsl::ChartSpec;
    use hiker_charts_core::theme::{Palette, Theme};

    fn state() -> BuilderState {
        let spec = ChartSpec::from_yaml("mark: line\nx: month\ny: revenue\n").unwrap();
        let table = Table::from_csv(b"month,revenue,profit\njan,100,20\nfeb,140,35\n").unwrap();
        BuilderState::new(spec, table, Theme::default(), Size { width: 320, height: 240 })
    }

    #[test]
    fn panel_runs_a_frame_without_panicking() {
        let mut s = state();
        let mut theme = ThemeChoice::default();
        let mut cam = Camera::default();
        let mut view = View::new();
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = panel(&mut s, &mut theme, &mut cam, &mut view, ui);
            });
        });
    }

    #[test]
    fn preview_gets_real_space_beside_controls() {
        // Regression: the control column must not expand to fill the row and
        // starve the preview. With a real screen rect the preview gets width, so
        // its first-frame auto-fit runs and moves the camera off the default.
        let mut s = state();
        let mut theme = ThemeChoice::default();
        let mut cam = Camera::default();
        let mut view = View::new();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = panel(&mut s, &mut theme, &mut cam, &mut view, ui);
            });
        });
        assert!(
            (cam.scale() - 1.0).abs() > f32::EPSILON,
            "preview should have received space and auto-fit the chart (scale {})",
            cam.scale()
        );
    }

    #[test]
    fn panel_runs_a_frame_for_every_mark() {
        // Each mark exercises a different registry capability profile (Arc is radial
        // with theta+inner-radius; Histogram has bins and no y channel), so running a
        // frame per mark proves the registry-driven gating builds every control path.
        for mark in ["bar", "line", "point", "area", "histogram", "arc"] {
            let yaml = format!("mark: {mark}\nx: month\ny: revenue\ntheta: revenue\ncolor: month\n");
            let spec = ChartSpec::from_yaml(&yaml).unwrap();
            let table =
                Table::from_csv(b"month,revenue,profit\njan,100,20\nfeb,140,35\n").unwrap();
            let mut s =
                BuilderState::new(spec, table, Theme::default(), Size { width: 320, height: 240 });
            let mut theme = ThemeChoice::default();
            let mut cam = Camera::default();
            let mut view = View::new();
            let ctx = egui::Context::default();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = panel(&mut s, &mut theme, &mut cam, &mut view, ui);
                });
            });
        }
    }

    #[test]
    fn theme_choice_builds_dark_palette_theme() {
        let choice = ThemeChoice { dark: true, palette: Palette::Warm };
        let theme = choice.theme();
        assert_eq!(theme.background, Theme::dark().background);
        assert_eq!(theme.series, Palette::Warm.colors());
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
