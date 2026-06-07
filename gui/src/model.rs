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
use hiker_charts_core::data::Table;
use hiker_charts_core::dsl::{
    ChartSpec, DataType, FieldDef, FieldSpec, Mark, YEncoding,
};
use hiker_charts_core::identity::content_hash;
use hiker_charts_core::resolve::resolve;
use hiker_charts_core::theme::Theme;
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

    /// Serialize the spec as a chart block: `to_yaml` wrapped in a ```chart fence
    /// (SPEC §8.2). "Copy" is the host pushing this string to the clipboard.
    #[must_use]
    pub fn to_block(&self) -> String {
        let yaml = self.spec.to_yaml().unwrap_or_default();
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

/// Encode a list of y field names as the terse `One` form for a single field or the
/// `Many` list form for multiple, matching how a human would hand-author it.
fn encode_y(fields: &[String]) -> YEncoding {
    if fields.len() == 1 {
        YEncoding::One(FieldDef::Shorthand(fields[0].clone()))
    } else {
        YEncoding::Many(fields.iter().map(|f| FieldDef::Shorthand(f.clone())).collect())
    }
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
    use super::{BuilderState, Channel, Provenance, SeriesMode};
    use hiker_charts_core::backend::Size;
    use hiker_charts_core::data::Table;
    use hiker_charts_core::dsl::{ChartSpec, DataType, Mark};
    use hiker_charts_core::theme::Theme;
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
    fn to_block_round_trips_through_from_yaml() {
        let s = state();
        let block = s.to_block();
        assert!(block.starts_with("```chart\n"));
        assert!(block.ends_with("```"));
        let inner = block
            .trim_start_matches("```chart\n")
            .trim_end_matches("```");
        let reparsed = ChartSpec::from_yaml(inner).unwrap();
        assert_eq!(&reparsed, s.spec());
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
}
