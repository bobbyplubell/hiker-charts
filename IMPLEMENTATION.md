# hiker-charts — Implementation

Companion to `SPEC.md`. This is the **how**: crate layout, the concrete `ChartSpec` /
`ResolvedChart` / `Backend` shapes, the render pipeline, and the test/lint gates. Type
signatures here are the **contract** between crates — implement to these names so the crates
compose.

> Conventions are Hiker's (this is a `notes` submodule). All code must pass
> `scripts/check.sh` (copied/adapted from `../notes/scripts`): `cargo test --workspace`, the
> strict clippy lint set, the 1500-line file cap, the anti-split detector, and the emoji ban.
> See §8. No `#[allow(...)]` escape hatches — fix the code or it doesn't land.

---

## 1. Crate layout (workspace)

```
hiker-charts/
  Cargo.toml            # workspace
  core/                 # pkg hiker-charts-core   (lib hiker_charts_core)
  backend-plotters/     # pkg hiker-charts-plotters
  cli/                  # pkg hiker-charts-cli     (bin hiker-charts)
  gui/                  # pkg hiker-charts-gui     (DEFERRED — design in §6)
  scripts/              # check.sh + python checks (adapted from ../notes)
  references/           # vega-lite, observable-plot, plotters (read-only)
```

Dependency graph (the §4.1 invariant: **plotters appears only in `backend-plotters`**):

```
core  ──────────────▶ (no plotters, no egui)
backend-plotters ───▶ core + plotters
cli  ───────────────▶ core + backend-plotters
gui  ───────────────▶ core + backend-plotters + egui + resvg   (deferred)
```

---

## 2. `core` — the model, data, and contracts

No plotters, no egui. May depend on `serde`, `serde_yml`, `csv`. Module layout (each file a
real module with a `//!` doc — the split detector requires it):

```
core/src/
  lib.rs        # crate doc + `pub mod` of the modules below (no glob re-exports)
  dsl.rs        # ChartSpec + serde (the DSL)   [renamed from spec.rs: module_name_repetitions]
  data.rs       # Table / Column / Cell (raw, resolver-supplied)
  typing.rs     # type inference + self-contained coercion (numbers, ISO dates)
  resolve.rs    # ChartSpec + Table -> ResolvedChart (wide/long normalization)
  backend.rs    # Backend trait, ResolvedChart, Series, Axis, Size, RenderOutput
  theme.rs      # Theme, Color
  host.rs       # DataResolver trait
  identity.rs   # content hash over (spec + table + theme + size)  [renamed from hash.rs]
  deps.rs       # data-dependency extraction
  diag.rs       # Diagnostic, Severity
```
(`spec`→`dsl` and `hash`→`identity` were renamed during implementation so the contract type
names `ChartSpec`/`content_hash` don't trip `clippy::module_name_repetitions`. Type/trait/fn
signatures are unchanged.)
(If a file approaches the 1500-line cap or a function the 200-line cap, split along real
seams — never `*_helper.rs` shards; the detector rejects those.)

### 2.1 `spec.rs` — the DSL as a struct

The canonical surface is **flat top-level channels** (matches `SPEC.md` §11.1 and the
prototype). Each channel is a `FieldDef`: a bare string shorthand *or* a full object. `y` is
single-or-list. Unknown fields are captured (forward-compat, `SPEC.md` §8.3).

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartSpec {
    pub mark: Mark,
    pub x: Option<FieldDef>,
    pub y: Option<YEncoding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<FieldDef>,
    /// Data identifier (e.g. "sales.csv"). `None` => the data Table is supplied
    /// directly (the inline CSV body of a ```chart block). Inline YAML `values`
    /// is deferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default)]
    pub config: Config,
    /// Fields the model does not (yet) understand — preserved across round-trips.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yml::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark { Bar, Line, Point, Area }

/// A channel binding: `x: month` (shorthand) or `x: {field: month, type: temporal}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldDef { Shorthand(String), Full(FieldSpec) }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    pub field: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<DataType>,
}

/// `y: revenue` or `y: [revenue, profit]` (wide multi-series, SPEC §2.3.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum YEncoding { One(FieldDef), Many(Vec<FieldDef>) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataType { Quantitative, Temporal, Ordinal, Nominal }

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub x_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub y_title: Option<String>,
    #[serde(default = "yes")] pub legend: bool,
    /// Hex palette override; defaults to the injected Theme when absent (SPEC §6.3).
    #[serde(default, skip_serializing_if = "Option::is_none")] pub palette: Option<Vec<String>>,
}
fn yes() -> bool { true }
```

Helpers on `ChartSpec` (the open-in-editor save, §6, needs `to_yaml`):
```rust
impl ChartSpec {
    pub fn from_yaml(s: &str) -> Result<Self, serde_yml::Error>;
    pub fn to_yaml(&self) -> Result<String, serde_yml::Error>;
    /// Convenience accessors that normalize FieldDef -> field name + optional type.
    pub fn x_field(&self) -> Option<(&str, Option<DataType>)>;
    pub fn y_fields(&self) -> Vec<(&str, Option<DataType>)>;  // 0, 1, or N
    pub fn color_field(&self) -> Option<(&str, Option<DataType>)>;
}
```

### 2.2 `data.rs` — raw table (what the resolver yields)

```rust
/// Resolver output: a header + rows of raw string cells. Typing happens in `typing`.
pub struct Table { pub columns: Vec<Column> }
pub struct Column { pub name: String, pub cells: Vec<String> }

impl Table {
    /// Parse a CSV byte slice (BurntSushi `csv`). Used for inline block bodies and by
    /// a host's resolver. The crate never reads files itself (SPEC §3.1).
    pub fn from_csv(bytes: &[u8]) -> Result<Self, csv::Error>;
    pub fn column(&self, name: &str) -> Option<&Column>;
}
```

### 2.3 `typing.rs` — inference + self-contained coercion

```rust
use crate::spec::DataType;

/// Typed scalar after coercion. Temporal is epoch **seconds** as i64 (kept as f64
/// downstream). Self-contained: number parse via std, date parse via our own ISO parser
/// (no chrono, no system locale — SPEC §3.4, §4.6).
pub enum Value { Number(f64), Time(i64), Category(String), Missing }

/// Infer a column's type by sniffing all non-empty cells: all parse as f64 -> Quantitative;
/// all parse as a supported date -> Temporal; else Nominal. (Ordinal is only ever declared,
/// never inferred.)
pub fn infer_type(cells: &[String]) -> DataType;

/// Coerce one cell to the declared/inferred type. Unparseable -> Value::Missing (+ a
/// diagnostic at the call site).
pub fn coerce(cell: &str, ty: DataType) -> Value;

/// Supported date formats (extend deliberately): `YYYY-MM-DD`, `YYYY-MM`, `YYYY/MM/DD`,
/// `YYYY-MM-DDTHH:MM:SS`. Returns epoch seconds (UTC, no tz handling in v1).
pub fn parse_date(s: &str) -> Option<i64>;
```

### 2.4 `resolve.rs` — wide/long → one neutral chart

This is where wide and long collapse to the same `Vec<Series>` (SPEC §2.3.1). The result is
**renderer-neutral** — pure `f64` coordinates plus axis label maps, so no backend ever sees
the original shape or a plotters type.

```rust
use crate::{spec::{ChartSpec, Mark}, data::Table, diag::Diagnostic};

pub fn resolve(spec: &ChartSpec, table: &Table) -> Result<ResolvedChart, Vec<Diagnostic>>;
```
Algorithm:
1. Determine x column + type (declared or inferred). Build the x->f64 mapping: quantitative =
   value; temporal = epoch seconds; **categorical = stable index 0..n with a label map**.
2. Series selection:
   - `y: [a, b, ...]` (wide) -> one `Series` per y column; series name = column name.
   - `y: v` + `color: c` (long) -> group rows by distinct value of `c`; one `Series` per
     group; series name = the category value.
   - `y: v`, no color -> single `Series`.
3. Each series = `Vec<(x_f64, y_f64)>`, rows with a `Missing` x or y dropped (+ diagnostic).
4. Axis kinds + label maps recorded for the backend's tick formatter.

`ResolvedChart` is defined in `backend.rs` (so the trait and its argument live together).

### 2.5 `backend.rs` — the swappable seam

```rust
pub struct Size { pub width: u32, pub height: u32 }

pub struct ResolvedChart {
    pub mark: Mark,
    pub series: Vec<Series>,
    pub x_axis: Axis,
    pub y_axis: Axis,
    pub config: Config,
}
pub struct Series { pub name: String, pub points: Vec<(f64, f64)> }
pub struct Axis { pub title: String, pub kind: AxisKind }
pub enum AxisKind {
    Quantitative,
    Temporal,                      // f64 is epoch seconds; format as date
    Categorical(Vec<String>),      // f64 is the index into these labels
}

pub struct RenderOutput { pub svg: String }   // RGBA is the host's job (resvg), SPEC §4.2

#[derive(Debug)]
pub enum RenderError { Empty, Backend(String) }

/// The one seam plotters lives behind. A first-party engine is a second impl later.
pub trait Backend {
    fn render(&self, chart: &ResolvedChart, theme: &Theme, size: Size)
        -> Result<RenderOutput, RenderError>;
}
```

### 2.6 `theme.rs`, `host.rs`, `hash.rs`, `deps.rs`, `diag.rs`

```rust
// theme.rs
#[derive(Clone, Copy, PartialEq, Eq, Hash)] pub struct Color { pub r:u8, pub g:u8, pub b:u8, pub a:u8 }
#[derive(Clone)] pub struct Theme {
    pub background: Color, pub foreground: Color, pub gridline: Color,
    pub series: Vec<Color>,    // categorical palette; index % len per series
}
impl Default for Theme { /* a neutral light default */ }

// host.rs
pub trait DataResolver { fn resolve(&self, id: &str) -> Result<crate::data::Table, ResolveError>; }
pub struct ResolveError(pub String);

// hash.rs  — input-keyed (SPEC §7), NOT over rendered bytes. std hasher is fine for v1
//           (session-scoped cache key); document that it is not cross-process-stable.
pub fn content_hash(spec: &ChartSpec, table: &Table, theme: &Theme, size: Size) -> u64;

// deps.rs
pub fn data_dependencies(spec: &ChartSpec) -> Vec<String>;   // v1: spec.data into a Vec

// diag.rs
pub enum Severity { Error, Warning }
pub struct Diagnostic { pub severity: Severity, pub message: String }
```

---

## 3. `backend-plotters` — the only crate that imports plotters

Depends on `core` + `plotters` (exact features from the prototype's `Cargo.toml`:
`default-features = false`, `svg_backend`, `line_series`, `point_series`, `area_series`,
`histogram`; **no `ttf`** — resvg draws glyphs, SPEC §4.2). One public type:

```rust
pub struct PlottersSvg;
impl hiker_charts_core::backend::Backend for PlottersSvg {
    fn render(&self, chart, theme, size) -> Result<RenderOutput, RenderError> { ... }
}
```

Mapping rules:
- **One coordinate type throughout: `f64 × f64`.** Categorical and temporal axes are already
  `f64` in `ResolvedChart`; build a `build_cartesian_2d(x_range, y_range)` over f64. This
  dodges plotters' generic-`ChartContext` awkwardness (the prototype's noted friction).
- **Tick label formatter** from `AxisKind`: Quantitative = the number; Temporal = format the
  epoch back to a date string (our own formatter, mirrors `typing::parse_date`); Categorical =
  `labels[index]`.
- **Marks:** `Line -> LineSeries`, `Point -> Circle` per point, `Area -> AreaSeries`,
  `Bar -> Rectangle` bars (grouped when multi-series: offset bars within each x slot; no
  stacking in v1, SPEC §2.3).
- **Theme:** background, axis/text = `foreground`, grid = `gridline`, series color =
  `config.palette` (if set) else `theme.series[i % len]`. Legend when `config.legend`.
- Empty chart (no series / no points) -> `Err(RenderError::Empty)`.

Keep `render` under the 200-line clippy cap by extracting genuine sub-steps (axis setup, a
per-mark draw fn keyed by `Mark`) into private fns or an `impl PlottersSvg` continuation file
— real seams, not shards.

---

## 4. `cli` — headless render

`hiker-charts <spec.yaml> <data.csv> -o out.svg` (and `--png` via the dev resvg path, like the
prototype's example). Reads files (the CLI *is* a host, so it may touch the fs), builds
`ChartSpec::from_yaml` + `Table::from_csv`, `resolve`, `PlottersSvg::render`, writes output.
Small; this is also the snapshot-test driver.

---

## 5. Testing (SPEC §13)

- `core`: round-trip `from_yaml -> to_yaml` stable incl. `extra` preservation; `infer_type` /
  `coerce` / `parse_date` edge cases; `resolve` wide vs long produce identical `Series`;
  missing-column and type-mismatch diagnostics.
- `backend-plotters`: render of each `mark × {single, wide, long}` returns non-empty SVG
  containing expected structure (e.g. a `<path>`/`<rect>` count, the title text). **No exact
  golden bytes** (plotters isn't byte-stable, SPEC §4.4) — assert on structure / use a
  tolerance image diff if rasterizing.
- `cli`: a smoke test over a tiny fixture spec+csv.

---

## 6. `gui` — comfy builder (DEFERRED, design captured)

Not built in the first implementation pass; documented so it slots in cleanly. Depends on
`core` + `backend-plotters` + `egui` + `resvg`.

- **`BuilderState`** holds a `ChartSpec`, the resolved `Table` (for column lists), and the
  last `RenderOutput` (cached by `content_hash`). Pure state + transitions could live in
  `core` later (SPEC §9) if a non-egui host wants them; v1 keeps them in `gui`.
- **Panel**: egui `ComboBox` for mark; `x`/`color` field pickers populated from
  `table.columns` (can't mistype a column); a `y` multiselect; per-field type pickers
  defaulting to the inferred type; title field; legend checkbox; a "series from: columns /
  values in a column" toggle that writes the wide vs long encoding (SPEC §2.3.1).
- **Preview**: rasterize `PlottersSvg` output via resvg -> egui texture, re-rendered only when
  `content_hash` changes (SPEC §7).
- **Export (v1 of gui)**: "Copy as ```chart block" = `spec.to_yaml()` (SPEC §8.2).
- **Open-in-editor / save-back (SPEC §8.3)**: the builder carries provenance
  `{ note id, byte_range }` (the byte range the host's fence detector reports). Save =
  `to_yaml()` -> splice into that byte range (re-attaching an inline CSV body verbatim) ->
  host writes the note. The splice + note write is **host-side** (Hiker's `app`/`editor`),
  not this crate; `gui` only emits the new block text + the target range.

Hiker-side wiring (the ```chart fence detector in `editor-md`, the CSV tab type) lives in the
`../notes` repo, not here — kept separate so this component stays host-agnostic and there is no
integration-time cleanup.

---

## 7. Cross-cutting rules

- **§4.1 invariant:** no `plotters` (or `egui`) type in any `pub` signature of `core`. A
  reviewer/CI grep for `plotters::` outside `backend-plotters/` must be empty.
- **Self-contained math/dates** in `core` (no chrono, no system numeric/locale libs;
  std parsing is fine) — SPEC §3.4, §4.6.
- **Determinism honesty:** hash inputs, not output; no byte-golden tests in v1 (SPEC §4.4/§7).

---

## 8. Required gates — `scripts/check.sh`

Every crate's code must pass, from the repo root:

```
scripts/check.sh
```

which runs: `cargo test --workspace`; `cargo clippy --workspace --all-targets` with the strict
deny-list (200-line fn cap, `wildcard_imports`, `module_inception`, `module_name_repetitions`,
`pub_use`, `cognitive_complexity`, `unnecessary_wraps`, `needless_pass_by_value`,
`too_many_arguments`, `trivially_copy_pass_by_ref`, `missing_const_for_fn`, …); the 1500-line
file cap (`check-lengths.py`); the anti-split detector (`check-splits.py` — every `mod.rs`/
`lib.rs` needs a real `//!` doc, no `*_helper.rs`/`*_part2.rs`, no sibling-only shards); and the
emoji ban (`check-emojis.py`). `#[allow(...)]` to dodge a lint is not permitted — fix the code.
