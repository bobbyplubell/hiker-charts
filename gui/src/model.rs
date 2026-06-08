//! `BuilderState`: the headless model behind the comfy builder and preview.
//!
//! (Module named `model` rather than `state` so the contract type `BuilderState`
//! does not trip `clippy::module_name_repetitions` — the same rename-the-module,
//! keep-the-type convention the `core` crate used for `spec`->`dsl`/`hash`->`identity`.)
//!
//! It owns the `ChartSpec` being edited, the resolved `Table` (for column lists and
//! inferred types), the `Theme`, the canvas `Size`, and a one-slot render cache keyed
//! on `core::identity::content_hash` (SPEC §7) so a static chart re-renders only when
//! an input actually changes. Every panel action is a transition method that mutates
//! the spec, so a panel-built spec is identical to a hand-authored one (SPEC §8.2).
//! Export wraps `to_yaml` in a chart fence (SPEC §8.2); open-in-editor keeps the
//! original block's inline CSV body and provenance so save can re-attach the data
//! verbatim into the exact byte range (SPEC §8.3). No egui types appear here — the
//! whole module is testable without a `Context`.

use std::ops::Range;

use hiker_charts_core::backend::{Backend, RenderOutput, Size};
use hiker_charts_core::block::parse_block;
use hiker_charts_core::data::Table;
use hiker_charts_core::dsl::{
    ChartSpec, DataType, FieldDef, FieldSpec, Interpolate, Mark, Orientation, Scale, ScaleKind,
    YEncoding,
};
use hiker_charts_core::identity::content_hash;
use hiker_charts_core::resolve::resolve;
use hiker_charts_core::theme::{Color, Theme};
use hiker_charts_core::typing::infer_type;

/// How a chart's multiple series are encoded: wide (one column per series, a `y`
/// list, no color) or long (one value column split by a `color` category column).
/// Both lower to the same `Vec<Series>` in the resolver (SPEC §2.3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeriesMode {
    /// `y: [a, b, ...]` — one series per named column.
    Wide,
    /// `y: value` + `color: metric` — one series per distinct value of `metric`.
    Long,
}

/// Where an opened chart block came from, so `save_block` can splice the
/// regenerated text back into exactly the right place (SPEC §8.3). The byte range
/// is the fence's inner range as reported by the host's fence detector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    /// The host's identifier for the note the block lives in.
    pub note_id: String,
    /// The byte range of the block body the save splices into.
    pub byte_range: Range<usize>,
}

/// The editable builder state: spec + data + theme + size, plus a render cache and
/// optional open-in-editor provenance.
pub struct BuilderState {
    spec: ChartSpec,
    table: Table,
    theme: Theme,
    size: Size,
    /// The verbatim inline CSV body of an opened block, re-attached on save. `None`
    /// for a fresh builder or a block that references external data (SPEC §8.3).
    csv_body: Option<String>,
    /// Where an opened block came from; `None` for a fresh, never-saved builder.
    provenance: Option<Provenance>,
    /// One-slot cache: the content hash of the inputs that produced `output`.
    cache: Option<(u64, RenderOutput)>,
}

impl BuilderState {
    /// Construct a builder around an existing spec, table, theme, and size. The
    /// render cache starts empty; no CSV body or provenance is attached.
    #[must_use]
    pub const fn new(spec: ChartSpec, table: Table, theme: Theme, size: Size) -> Self {
        Self {
            spec,
            table,
            theme,
            size,
            csv_body: None,
            provenance: None,
            cache: None,
        }
    }

    /// Open a chart block's config in the builder, seeded from its parsed YAML and
    /// the host-resolved `table`. The original `csv_body` (the block's inline CSV,
    /// if any) and `prov` are retained so `save_block` re-attaches the data verbatim
    /// into the original byte range (SPEC §8.3). Returns the parse error message if
    /// the YAML is not a valid `ChartSpec` (the concrete `serde_yml` error type is
    /// kept out of this crate's public surface).
    pub fn from_block(
        yaml: &str,
        table: Table,
        csv_body: Option<String>,
        prov: Provenance,
        theme: Theme,
        size: Size,
    ) -> Result<Self, String> {
        let spec = ChartSpec::from_yaml(yaml).map_err(|e| e.to_string())?;
        Ok(Self {
            spec,
            table,
            theme,
            size,
            csv_body,
            provenance: Some(prov),
            cache: None,
        })
    }

    /// Open a self-contained chart block — the YAML config, a `---` line, then a raw
    /// CSV body (SPEC §8.3) — splitting and parsing it in one step via
    /// [`hiker_charts_core::block::parse_block`]. This is the read half of the
    /// round-trip [`save_block`](Self::save_block) writes: the inline CSV becomes the
    /// builder's table and is retained verbatim for re-attachment on save. Returns an
    /// error message if the config or CSV is malformed, or if the block carries no
    /// `---` data section (use [`from_block`](Self::from_block) with a host-resolved
    /// table for an external-`data:` block).
    pub fn from_block_body(
        body: &str,
        prov: Provenance,
        theme: Theme,
        size: Size,
    ) -> Result<Self, String> {
        let parsed = parse_block(body).map_err(|diags| {
            diags.into_iter().map(|d| d.message).collect::<Vec<_>>().join("; ")
        })?;
        let table = parsed
            .table
            .ok_or("chart block has no inline data (expected a `---` line followed by CSV)")?;
        Ok(Self {
            spec: parsed.spec,
            table,
            theme,
            size,
            csv_body: parsed.csv_body,
            provenance: Some(prov),
            cache: None,
        })
    }

    /// The current spec being edited.
    #[must_use]
    pub const fn spec(&self) -> &ChartSpec {
        &self.spec
    }

    /// The resolved data table backing the column pickers.
    #[must_use]
    pub const fn table(&self) -> &Table {
        &self.table
    }

    /// The canvas size charts render at.
    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    /// The host theme applied to renders.
    #[must_use]
    pub const fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The table's column names, in order, for populating the field pickers.
    pub fn columns(&self) -> Vec<&str> {
        self.table.columns.iter().map(|c| c.name.as_str()).collect()
    }

    /// The inferred data type of a named column, or `None` if no such column. Drives
    /// the per-field type pickers' defaults (SPEC §3.4).
    #[must_use]
    pub fn inferred_type(&self, column: &str) -> Option<DataType> {
        self.table.column(column).map(|c| infer_type(&c.cells))
    }

    // --- Transitions (each mutates the spec; the next render recomputes) -------

    /// Set the geometric mark.
    pub const fn set_mark(&mut self, mark: Mark) {
        self.spec.mark = mark;
    }

    /// Bind (or clear) the x channel to a column by name.
    pub fn set_x(&mut self, column: Option<&str>) {
        self.spec.x = column.map(|c| FieldDef::Shorthand(c.to_string()));
    }

    /// Add a y field if not already present (wide multi-series). A `None` y becomes
    /// a single field; an existing one promotes to a `Many` list.
    pub fn add_y(&mut self, column: &str) {
        let mut fields = self.y_names();
        if fields.iter().any(|f| f == column) {
            return;
        }
        fields.push(column.to_string());
        self.spec.y = Some(encode_y(&fields));
    }

    /// Remove a y field by name; collapses back to a single field or `None`.
    pub fn remove_y(&mut self, column: &str) {
        let fields: Vec<String> = self.y_names().into_iter().filter(|f| f != column).collect();
        self.spec.y = if fields.is_empty() { None } else { Some(encode_y(&fields)) };
    }

    /// Bind (or clear) the color channel to a column by name.
    pub fn set_color(&mut self, column: Option<&str>) {
        self.spec.color = column.map(|c| FieldDef::Shorthand(c.to_string()));
    }

    /// Bind (or clear) the size channel to a column by name (bubble radius for `Point`).
    pub fn set_size(&mut self, column: Option<&str>) {
        self.spec.size = column.map(|c| FieldDef::Shorthand(c.to_string()));
    }

    /// Bind (or clear) the theta channel to a column by name (slice magnitude for `Arc`).
    pub fn set_theta(&mut self, column: Option<&str>) {
        self.spec.theta = column.map(|c| FieldDef::Shorthand(c.to_string()));
    }

    // --- Table mark (column selection/order + transpose) ----------------------

    /// The columns the `Table` mark currently shows, in display order: the explicit
    /// `spec.columns` list if set, otherwise every data column in natural order.
    #[must_use]
    pub fn shown_columns(&self) -> Vec<String> {
        match &self.spec.columns {
            Some(cols) => cols.clone(),
            None => self.table.columns.iter().map(|c| c.name.clone()).collect(),
        }
    }

    /// Whether `column` is in the table's current shown set.
    #[must_use]
    pub fn is_column_shown(&self, column: &str) -> bool {
        self.shown_columns().iter().any(|c| c == column)
    }

    /// Include or exclude `column` from the table, materializing the explicit `columns` list on
    /// first edit (so removing from the implicit "all" set keeps the others). A re-added column
    /// is appended at the end; reorder it with [`move_column`](Self::move_column).
    pub fn toggle_column(&mut self, column: &str) {
        let mut cols = self.shown_columns();
        if let Some(pos) = cols.iter().position(|c| c == column) {
            cols.remove(pos);
        } else {
            cols.push(column.to_string());
        }
        self.spec.columns = Some(cols);
    }

    /// Move the shown column at `index` one slot toward the front (`up`) or back, materializing
    /// the explicit list. A no-op at the ends or for an out-of-range index.
    pub fn move_column(&mut self, index: usize, up: bool) {
        let mut cols = self.shown_columns();
        let swap = if up { index.checked_sub(1) } else { index.checked_add(1) };
        if let Some(j) = swap
            && index < cols.len()
            && j < cols.len()
        {
            cols.swap(index, j);
            self.spec.columns = Some(cols);
        }
    }

    /// Transpose the table (fields down the left, records across) or restore natural layout.
    pub const fn set_transpose(&mut self, transpose: bool) {
        self.spec.config.transpose = transpose;
    }

    /// Switch between wide and long multi-series encodings (SPEC §2.3.1). Wide drops
    /// `color` and keeps the `y` list; long collapses `y` to its first field and
    /// moves the chosen split column into `color`. `long_split` is ignored in wide
    /// mode and supplies the color column in long mode (defaulting to the existing
    /// color binding when `None`).
    pub fn set_series_mode(&mut self, mode: SeriesMode, long_split: Option<&str>) {
        match mode {
            SeriesMode::Wide => self.spec.color = None,
            SeriesMode::Long => {
                if let Some(first) = self.y_names().into_iter().next() {
                    self.spec.y = Some(YEncoding::One(FieldDef::Shorthand(first)));
                }
                if let Some(split) = long_split {
                    self.spec.color = Some(FieldDef::Shorthand(split.to_string()));
                }
            }
        }
    }

    /// Declare (or clear) the data type of a channel's field, promoting a shorthand
    /// binding to the full object form so the type can be carried.
    pub fn set_field_type(&mut self, channel: Channel, ty: Option<DataType>) {
        let slot = match channel {
            Channel::X => &mut self.spec.x,
            Channel::Color => &mut self.spec.color,
        };
        if let Some(def) = slot.as_mut() {
            let (name, _) = def.parts();
            *def = FieldDef::Full(FieldSpec { field: name.to_string(), ty });
        }
    }

    /// Set (or clear) the chart title in the config.
    pub fn set_title(&mut self, title: Option<&str>) {
        self.spec.config.title = title.map(str::to_string);
    }

    /// Toggle the legend on or off.
    pub const fn toggle_legend(&mut self) {
        self.spec.config.legend = !self.spec.config.legend;
    }

    /// Set (or clear) the x-axis title in the config.
    pub fn set_x_title(&mut self, title: Option<&str>) {
        self.spec.config.x_title = title.map(str::to_string);
    }

    /// Set (or clear) the y-axis title in the config.
    pub fn set_y_title(&mut self, title: Option<&str>) {
        self.spec.config.y_title = title.map(str::to_string);
    }

    /// Set (or clear) the point/scatter radius in pixels.
    pub const fn set_point_size(&mut self, size: Option<f32>) {
        self.spec.config.point_size = size;
    }

    /// Set (or clear) the line/area stroke width in pixels.
    pub const fn set_line_width(&mut self, width: Option<f32>) {
        self.spec.config.line_width = width;
    }

    /// Set (or clear) the area/bar fill opacity (`0.0..=1.0`).
    pub const fn set_fill_opacity(&mut self, opacity: Option<f32>) {
        self.spec.config.fill_opacity = opacity;
    }

    /// Stack or unstack bar/area series on a per-x cumulative baseline.
    pub const fn set_stack(&mut self, stack: bool) {
        self.spec.config.stack = stack;
    }

    /// Set (or clear) the line interpolation; `None` is the linear default.
    pub const fn set_interpolate(&mut self, interpolate: Option<Interpolate>) {
        self.spec.config.interpolate = interpolate;
    }

    /// Show or hide the interior mesh gridlines.
    pub const fn set_show_grid(&mut self, show: bool) {
        self.spec.config.show_grid = show;
    }

    /// Set (or clear) the bar/area orientation; `None` restores the vertical default.
    pub const fn set_orientation(&mut self, orientation: Option<Orientation>) {
        self.spec.config.orientation = orientation;
    }

    /// Set (or clear) the histogram bucket count; `None` lets the resolver default apply.
    pub const fn set_bins(&mut self, bins: Option<usize>) {
        self.spec.config.bins = bins;
    }

    /// Set (or clear) the donut hole radius as a `0.0..=1.0` fraction (`Arc`); `None` = pie.
    pub const fn set_inner_radius(&mut self, radius: Option<f32>) {
        self.spec.config.inner_radius = radius;
    }

    /// Set (or clear) the whole x-axis scale; `None` restores the linear identity.
    pub const fn set_x_scale(&mut self, scale: Option<Scale>) {
        self.spec.config.x_scale = scale;
    }

    /// Set (or clear) the whole y-axis scale; `None` restores the linear identity.
    pub const fn set_y_scale(&mut self, scale: Option<Scale>) {
        self.spec.config.y_scale = scale;
    }

    /// Set the transform kind of one cartesian axis, preserving its domain/zero. Materializes
    /// a default `Scale` first so the panel can change kind without touching the other fields.
    pub fn set_scale_kind(&mut self, axis: Axis, kind: ScaleKind) {
        let scale = self.scale_or_default(axis);
        self.write_scale(axis, Scale { kind, ..scale });
    }

    /// Set (or clear) one cartesian axis's explicit `(min, max)` domain; `None` = auto.
    pub fn set_scale_domain(&mut self, axis: Axis, domain: Option<(f64, f64)>) {
        let scale = self.scale_or_default(axis);
        self.write_scale(axis, Scale { domain, ..scale });
    }

    /// Set whether one cartesian axis's auto domain includes zero.
    pub fn set_scale_zero(&mut self, axis: Axis, zero: bool) {
        let scale = self.scale_or_default(axis);
        self.write_scale(axis, Scale { zero, ..scale });
    }

    /// The current scale for `axis`, or the linear default when none is set, so a partial
    /// edit (just the kind, say) starts from a complete value.
    fn scale_or_default(&self, axis: Axis) -> Scale {
        match axis {
            Axis::X => self.spec.config.x_scale,
            Axis::Y => self.spec.config.y_scale,
        }
        .unwrap_or_default()
    }

    /// Write a fully-formed scale back to the chosen axis slot.
    const fn write_scale(&mut self, axis: Axis, scale: Scale) {
        match axis {
            Axis::X => self.spec.config.x_scale = Some(scale),
            Axis::Y => self.spec.config.y_scale = Some(scale),
        }
    }

    // --- Per-series color override -------------------------------------------

    /// Override the color of series `index` in `config.palette`, padding earlier
    /// entries from the current effective colors so positions before `index` keep
    /// the color they render with today. Writing the palette overrides the theme
    /// for those series (SPEC §6.3); the rest still fall back to the theme.
    pub fn set_series_color(&mut self, index: usize, color: Color) {
        let mut palette = self.spec.config.palette.take().unwrap_or_default();
        while palette.len() <= index {
            let pos = palette.len();
            palette.push(hex(self.effective_color(pos)));
        }
        palette[index] = hex(color);
        self.spec.config.palette = Some(palette);
    }

    /// Drop any per-series palette override, restoring the theme's palette.
    pub fn clear_palette(&mut self) {
        self.spec.config.palette = None;
    }

    /// The color series `index` currently renders with, for seeding a color picker:
    /// the palette override entry if present and parseable, else the theme color.
    #[must_use]
    pub fn effective_series_color(&self, index: usize) -> Color {
        self.effective_color(index)
    }

    /// The color series `pos` currently renders with: the existing palette override
    /// entry (if parseable) else the theme's palette indexed `pos % len`. Mirrors
    /// the backend's `series_color` resolution so padding is faithful.
    fn effective_color(&self, pos: usize) -> Color {
        if let Some(palette) = self.spec.config.palette.as_ref()
            && !palette.is_empty()
            && let Some(c) = parse_hex(&palette[pos % palette.len()])
        {
            return c;
        }
        let series = &self.theme.series;
        if series.is_empty() {
            self.theme.foreground
        } else {
            series[pos % series.len()]
        }
    }

    // --- Global theme ---------------------------------------------------------

    /// Replace the global theme applied to renders (light/dark + palette).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    // --- Series introspection -------------------------------------------------

    /// Resolve the spec against the table and return the series display names in
    /// render order: the y field names for wide encodings, the distinct color
    /// values for long ones. Returns an empty vec when the chart cannot resolve
    /// (e.g. no bound channels), so the panel can show one color picker per series.
    #[must_use]
    pub fn series_names(&self) -> Vec<String> {
        match resolve(&self.spec, &self.table) {
            Ok(chart) => chart.series.into_iter().map(|s| s.name).collect(),
            Err(_) => Vec::new(),
        }
    }

    // --- Render cache ---------------------------------------------------------

    /// Render the chart through `backend`, recomputing only when the content hash of
    /// the inputs (spec + table + theme + size) changes (SPEC §7). Returns `None`
    /// when the chart has no drawable data or the backend errors. The cached
    /// `RenderOutput` is returned on a hash hit without touching the backend.
    pub fn render(&mut self, backend: &dyn Backend) -> Option<&RenderOutput> {
        let hash = content_hash(&self.spec, &self.table, &self.theme, self.size);
        let hit = matches!(self.cache, Some((h, _)) if h == hash);
        if !hit {
            let chart = resolve(&self.spec, &self.table).ok()?;
            let output = backend.render(&chart, &self.theme, self.size).ok()?;
            self.cache = Some((hash, output));
        }
        self.cache.as_ref().map(|(_, out)| out)
    }

    // --- Export / save-back ---------------------------------------------------

    /// Serialize the spec as a chart block for "Copy" (SPEC §8.2). Defaults to a
    /// renderable block: if the spec already references external data (`data:`),
    /// emit that reference (config only); otherwise emit a **self-contained**
    /// block (config + `---` + the data as CSV) so a builder seeded from a raw
    /// table — e.g. a `.csv` opened in a tab — still produces a block that
    /// renders anywhere. The host picks an explicit mode with
    /// [`to_block_inline`](Self::to_block_inline) /
    /// [`to_block_reference`](Self::to_block_reference).
    #[must_use]
    pub fn to_block(&self) -> String {
        if self.spec.data.is_some() {
            let yaml = self.spec.to_yaml().unwrap_or_default();
            format!("```chart\n{}```", ensure_trailing_newline(&yaml))
        } else {
            self.to_block_inline()
        }
    }

    /// Serialize a **self-contained** chart block: the config, a `---` line, then
    /// the data as CSV. The verbatim `csv_body` is re-emitted when present (an
    /// opened inline block), else the table is serialized via
    /// [`Table::to_csv`](hiker_charts_core::data::Table::to_csv). Renders with no
    /// external file dependency. status: chart-export-mode
    #[must_use]
    pub fn to_block_inline(&self) -> String {
        let yaml = self.spec.to_yaml().unwrap_or_default();
        let csv = self.csv_body.clone().unwrap_or_else(|| self.table.to_csv());
        format!(
            "```chart\n{}---\n{}```",
            ensure_trailing_newline(&yaml),
            ensure_trailing_newline(&csv),
        )
    }

    /// Serialize a chart block that **references external data**: the config with
    /// `data: <data_path>` set (the host supplies the vault-relative path), no
    /// inline CSV. Keeps the note small and re-renders live when the file
    /// changes, at the cost of depending on the file staying put. Does not mutate
    /// the live spec — the `data:` binding is applied to a clone for this emit
    /// only. status: chart-export-mode
    #[must_use]
    pub fn to_block_reference(&self, data_path: &str) -> String {
        let mut spec = self.spec.clone();
        spec.data = Some(data_path.to_string());
        let yaml = spec.to_yaml().unwrap_or_default();
        format!("```chart\n{}```", ensure_trailing_newline(&yaml))
    }

    /// Produce the regenerated block body plus the byte range to splice it into for
    /// an open-in-editor save (SPEC §8.3). Only the config is re-serialized; the
    /// original inline CSV body is re-attached verbatim after a `---` separator.
    /// Returns `None` if the builder has no provenance (it was not opened from a
    /// block, so there is nothing to save back).
    #[must_use]
    pub fn save_block(&self) -> Option<(String, Range<usize>)> {
        let prov = self.provenance.as_ref()?;
        let yaml = self.spec.to_yaml().unwrap_or_default();
        let mut body = ensure_trailing_newline(&yaml);
        if let Some(csv) = self.csv_body.as_deref() {
            body.push_str("---\n");
            body.push_str(csv);
        }
        Some((body, prov.byte_range.clone()))
    }

    /// The y channel's field names as owned strings (empty, one, or many).
    fn y_names(&self) -> Vec<String> {
        self.spec.y_fields().into_iter().map(|(n, _)| n.to_string()).collect()
    }
}

/// Which single-field channel a type declaration targets. `y` is excluded because
/// it may hold many fields; per-y types use the full object form directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    X,
    Color,
}

/// Which cartesian axis a scale edit targets. Radial marks have no axes, so the panel
/// only surfaces these for cartesian marks (`caps.options.scales`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

/// Encode a list of y field names as the terse `One` form for a single field or the
/// `Many` list form for multiple, matching how a human would hand-author it.
fn encode_y(fields: &[String]) -> YEncoding {
    if fields.len() == 1 {
        YEncoding::One(FieldDef::Shorthand(fields[0].clone()))
    } else {
        YEncoding::Many(fields.iter().map(|f| FieldDef::Shorthand(f.clone())).collect())
    }
}

/// Format an opaque color as a `#rrggbb` hex string for `config.palette`.
fn hex(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// Parse a `#rrggbb` hex string back into an opaque `Color`. Returns `None` on any
/// malformed input so padding falls back to the theme color. Mirrors the backend's
/// `#rrggbb` parsing (the only form `set_series_color` ever writes).
fn parse_hex(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(Color::rgb(r, g, b))
}

/// Ensure a string ends with exactly one trailing newline, so a fence's closing
/// backticks sit on their own line regardless of the serializer's output.
fn ensure_trailing_newline(s: &str) -> String {
    if s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::{Axis, BuilderState, Channel, Provenance, SeriesMode};
    use hiker_charts_core::backend::Size;
    use hiker_charts_core::data::Table;
    use hiker_charts_core::dsl::{ChartSpec, DataType, Interpolate, Mark, Orientation, ScaleKind};
    use hiker_charts_core::theme::{Color, Palette, Theme};
    use hiker_charts_plotters::PlottersSvg;

    fn table() -> Table {
        Table::from_csv(b"month,revenue,profit\njan,100,20\nfeb,140,35\n").unwrap()
    }

    fn state() -> BuilderState {
        let spec = ChartSpec::from_yaml("mark: line\nx: month\ny: revenue\n").unwrap();
        BuilderState::new(spec, table(), Theme::default(), Size { width: 320, height: 240 })
    }

    #[test]
    fn transition_changes_spec() {
        let mut s = state();
        assert_eq!(s.spec().mark, Mark::Line);
        s.set_mark(Mark::Bar);
        assert_eq!(s.spec().mark, Mark::Bar);
        s.add_y("profit");
        assert_eq!(s.spec().y_fields().len(), 2);
        s.remove_y("revenue");
        assert_eq!(s.spec().y_fields(), vec![("profit", None)]);
    }

    #[test]
    fn columns_and_inferred_types() {
        let s = state();
        assert_eq!(s.columns(), vec!["month", "revenue", "profit"]);
        assert_eq!(s.inferred_type("revenue"), Some(DataType::Quantitative));
        assert_eq!(s.inferred_type("month"), Some(DataType::Nominal));
        assert_eq!(s.inferred_type("nope"), None);
    }

    #[test]
    fn set_field_type_promotes_to_full_form() {
        let mut s = state();
        s.set_field_type(Channel::X, Some(DataType::Temporal));
        assert_eq!(s.spec().x_field(), Some(("month", Some(DataType::Temporal))));
    }

    #[test]
    fn series_mode_toggles_wide_long() {
        let mut s = state();
        s.add_y("profit");
        s.set_series_mode(SeriesMode::Long, Some("metric"));
        assert_eq!(s.spec().y_fields().len(), 1);
        assert_eq!(s.spec().color_field(), Some(("metric", None)));
        s.set_series_mode(SeriesMode::Wide, None);
        assert_eq!(s.spec().color_field(), None);
    }

    #[test]
    fn render_caches_until_input_changes() {
        let mut s = state();
        let first = s.render(&PlottersSvg).expect("first render").svg.clone();
        // Same inputs: the cached output is byte-identical (no recompute).
        let second = s.render(&PlottersSvg).expect("cached render").svg.clone();
        assert_eq!(first, second);
        // A spec change invalidates the cache and yields different output.
        s.set_mark(Mark::Bar);
        let third = s.render(&PlottersSvg).expect("re-render").svg.clone();
        assert_ne!(first, third);
    }

    #[test]
    fn to_block_is_self_contained_and_round_trips() {
        // status: chart-export-mode — a builder with inline data (no `data:`
        // ref) exports a self-contained block: config + `---` + CSV. It
        // round-trips through `parse_block` to the same spec, with a table.
        use hiker_charts_core::block::parse_block;
        let s = state();
        let block = s.to_block();
        assert!(block.starts_with("```chart\n"));
        assert!(block.ends_with("```"));
        assert!(block.contains("---\n"), "self-contained block carries inline CSV");
        let inner = block.trim_start_matches("```chart\n").trim_end_matches("```");
        let parsed = parse_block(inner).expect("self-contained block parses");
        assert_eq!(&parsed.spec, s.spec());
        let table = parsed.table.expect("inline CSV yields a table");
        assert_eq!(table.columns.len(), 3, "month,revenue,profit survive the round-trip");
    }

    #[test]
    fn to_block_reference_emits_data_path_and_no_csv() {
        // status: chart-export-mode — the reference export injects `data:` and
        // omits the inline CSV, without mutating the live spec.
        use hiker_charts_core::block::parse_block;
        let s = state();
        let block = s.to_block_reference("data/sales.csv");
        assert!(!block.contains("---\n"), "reference block carries no inline CSV");
        let inner = block.trim_start_matches("```chart\n").trim_end_matches("```");
        let parsed = parse_block(inner).expect("reference block parses");
        assert_eq!(parsed.spec.data.as_deref(), Some("data/sales.csv"));
        assert!(parsed.table.is_none());
        // The live spec is untouched (no `data:` leaked in).
        assert_eq!(s.spec().data, None);
    }

    #[test]
    fn to_block_inline_serializes_table_without_csv_body() {
        // status: chart-export-mode — a builder seeded from a raw table (no
        // verbatim `csv_body`, like a `.csv`-tab builder) still emits inline CSV
        // by serializing the table.
        use hiker_charts_core::block::parse_block;
        let s = state(); // BuilderState::new → csv_body is None
        let block = s.to_block_inline();
        let inner = block.trim_start_matches("```chart\n").trim_end_matches("```");
        let parsed = parse_block(inner).expect("parses");
        let table = parsed.table.expect("table serialized into the block");
        assert_eq!(table.row_count(), 2, "both data rows serialized");
    }

    #[test]
    fn from_block_save_preserves_csv_verbatim() {
        let csv = "month,revenue\njan,100\nfeb,140\n";
        let prov = Provenance { note_id: "n1".to_string(), byte_range: 10..42 };
        let s = BuilderState::from_block(
            "mark: bar\nx: month\ny: revenue\n",
            table(),
            Some(csv.to_string()),
            prov.clone(),
            Theme::default(),
            Size { width: 320, height: 240 },
        )
        .unwrap();
        let (body, range) = s.save_block().expect("provenance present");
        assert_eq!(range, prov.byte_range);
        assert!(body.contains("---\n"));
        // The original CSV bytes survive verbatim after the separator.
        assert!(body.ends_with(csv));
    }

    #[test]
    fn save_block_none_without_provenance() {
        assert!(state().save_block().is_none());
    }

    #[test]
    fn from_block_body_parses_inline_csv_and_round_trips() {
        let body = "mark: bar\nx: month\ny: revenue\n---\nmonth,revenue\njan,100\nfeb,140\n";
        let prov = Provenance { note_id: "n1".to_string(), byte_range: 0..body.len() };
        let s = BuilderState::from_block_body(
            body,
            prov,
            Theme::default(),
            Size { width: 320, height: 240 },
        )
        .expect("inline-csv block opens");
        // The inline CSV became the builder's table.
        assert_eq!(s.columns(), vec!["month", "revenue"]);
        assert_eq!(s.spec().mark, Mark::Bar);
        // Save re-emits the `---` + CSV shape the read half just consumed: the data
        // bytes survive verbatim, so open→save is a faithful round-trip.
        let (saved, _) = s.save_block().expect("provenance present");
        assert!(saved.contains("---\n"));
        assert!(saved.ends_with("month,revenue\njan,100\nfeb,140\n"));
    }

    #[test]
    fn from_block_body_errors_without_inline_data() {
        let prov = Provenance { note_id: "n1".to_string(), byte_range: 0..0 };
        let result = BuilderState::from_block_body(
            "mark: bar\nx: a\ny: b\ndata: sales.csv\n",
            prov,
            Theme::default(),
            Size { width: 320, height: 240 },
        );
        match result {
            Err(err) => assert!(err.contains("no inline data"), "got: {err}"),
            Ok(_) => panic!("a block without inline data should not open"),
        }
    }

    #[test]
    fn styling_transitions_change_config() {
        let mut s = state();
        s.set_x_title(Some("Month"));
        s.set_y_title(Some("USD"));
        s.set_point_size(Some(6.0));
        s.set_line_width(Some(2.5));
        s.set_fill_opacity(Some(0.4));
        s.set_stack(true);
        s.set_interpolate(Some(Interpolate::Step));
        s.set_show_grid(false);
        let c = &s.spec().config;
        assert_eq!(c.x_title.as_deref(), Some("Month"));
        assert_eq!(c.y_title.as_deref(), Some("USD"));
        assert_eq!(c.point_size, Some(6.0));
        assert_eq!(c.line_width, Some(2.5));
        assert_eq!(c.fill_opacity, Some(0.4));
        assert!(c.stack);
        assert_eq!(c.interpolate, Some(Interpolate::Step));
        assert!(!c.show_grid);
        // Clearing an Option title removes it.
        s.set_x_title(None);
        assert_eq!(s.spec().config.x_title, None);
    }

    #[test]
    fn set_theme_replaces_theme() {
        let mut s = state();
        assert_eq!(s.theme(), &Theme::light());
        s.set_theme(Theme::dark().with_palette(Palette::Warm));
        assert_eq!(s.theme().background, Theme::dark().background);
        assert_eq!(s.theme().series, Palette::Warm.colors());
    }

    #[test]
    fn set_series_color_pads_and_renders() {
        let mut s = state();
        s.add_y("profit"); // two series: revenue, profit
        // Color the second series red; the first is padded from the theme.
        s.set_series_color(1, Color::rgb(0xff, 0x00, 0x00));
        let palette = s.spec().config.palette.as_ref().expect("palette set");
        assert_eq!(palette.len(), 2);
        assert_eq!(palette[1], "#ff0000");
        // The padded first entry is the theme's first series color.
        assert_eq!(palette[0], "#1f77b4");
        // The override shows up in the rendered SVG.
        let svg = s.render(&PlottersSvg).expect("render").svg.clone();
        assert!(svg.to_uppercase().contains("FF0000"), "override color missing");
        // Reset drops the override.
        s.clear_palette();
        assert!(s.spec().config.palette.is_none());
    }

    #[test]
    fn size_and_theta_channels_bind_and_clear() {
        let mut s = state();
        s.set_size(Some("revenue"));
        s.set_theta(Some("profit"));
        assert_eq!(s.spec().size_field(), Some(("revenue", None)));
        assert_eq!(s.spec().theta_field(), Some(("profit", None)));
        s.set_size(None);
        s.set_theta(None);
        assert_eq!(s.spec().size_field(), None);
        assert_eq!(s.spec().theta_field(), None);
    }

    #[test]
    fn new_option_transitions_change_config() {
        let mut s = state();
        s.set_orientation(Some(Orientation::Horizontal));
        s.set_bins(Some(15));
        s.set_inner_radius(Some(0.3));
        let c = &s.spec().config;
        assert_eq!(c.orientation, Some(Orientation::Horizontal));
        assert_eq!(c.bins, Some(15));
        assert_eq!(c.inner_radius, Some(0.3));
        s.set_orientation(None);
        s.set_bins(None);
        s.set_inner_radius(None);
        let c = &s.spec().config;
        assert_eq!(c.orientation, None);
        assert_eq!(c.bins, None);
        assert_eq!(c.inner_radius, None);
    }

    #[test]
    fn scale_edits_compose_kind_domain_zero() {
        let mut s = state();
        s.set_scale_kind(Axis::X, ScaleKind::Log);
        s.set_scale_domain(Axis::X, Some((1.0, 1000.0)));
        s.set_scale_zero(Axis::X, true);
        let x = s.spec().config.x_scale.expect("x scale set");
        assert_eq!(x.kind, ScaleKind::Log);
        assert_eq!(x.domain, Some((1.0, 1000.0)));
        assert!(x.zero);
        // The y axis is untouched.
        assert_eq!(s.spec().config.y_scale, None);
        // set_y_scale writes the whole value; clearing restores the linear identity.
        s.set_y_scale(None);
        assert_eq!(s.spec().config.y_scale, None);
    }

    #[test]
    fn table_columns_default_to_all_then_become_explicit() {
        let mut s = state();
        // No explicit columns yet: shown set is every data column in order.
        assert_eq!(s.shown_columns(), vec!["month", "revenue", "profit"]);
        assert!(s.is_column_shown("revenue"));
        // Removing one materializes the explicit list of the rest.
        s.toggle_column("revenue");
        assert_eq!(s.shown_columns(), vec!["month", "profit"]);
        assert!(!s.is_column_shown("revenue"));
        assert_eq!(s.spec().columns.as_deref(), Some(["month", "profit"].map(String::from).as_slice()));
        // Re-adding appends at the end.
        s.toggle_column("revenue");
        assert_eq!(s.shown_columns(), vec!["month", "profit", "revenue"]);
    }

    #[test]
    fn move_column_reorders_and_clamps() {
        let mut s = state();
        s.move_column(0, false); // month down past revenue
        assert_eq!(s.shown_columns(), vec!["revenue", "month", "profit"]);
        s.move_column(0, true); // already at front: no-op
        assert_eq!(s.shown_columns(), vec!["revenue", "month", "profit"]);
        s.move_column(2, false); // last item down: no-op
        assert_eq!(s.shown_columns(), vec!["revenue", "month", "profit"]);
    }

    #[test]
    fn table_mark_renders_through_backend() {
        let mut s = state();
        s.set_mark(Mark::Table);
        s.set_transpose(true);
        assert!(s.spec().config.transpose);
        let svg = s.render(&PlottersSvg).expect("table renders").svg.clone();
        assert!(svg.contains("<svg") && svg.contains("revenue"));
    }

    #[test]
    fn series_names_wide_are_y_fields() {
        let mut s = state();
        s.add_y("profit");
        assert_eq!(s.series_names(), vec!["revenue", "profit"]);
    }

    #[test]
    fn series_names_long_are_category_values() {
        let csv = b"day,metric,value\nmon,a,1\nmon,b,2\ntue,a,3\ntue,b,4\n";
        let spec = ChartSpec::from_yaml("mark: line\nx: day\ny: value\ncolor: metric\n").unwrap();
        let s = BuilderState::new(
            spec,
            Table::from_csv(csv).unwrap(),
            Theme::default(),
            Size { width: 320, height: 240 },
        );
        let mut names = s.series_names();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }
}
