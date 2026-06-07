//! The chart DSL as a serde struct: `ChartSpec` plus its channel types.
//!
//! YAML/JSON are merely constructors for this struct (SPEC §2.1); there is no
//! hand-written parser. Channels are flat top-level fields, each a bare-string
//! shorthand or a full object, and unknown fields are captured in `extra` so they
//! survive a round-trip (SPEC §8.3). The convenience accessors normalize the
//! shorthand/full and single/list distinctions for the resolver.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A full chart specification: a mark, encoding channels, optional data id, and
/// config. Forward-compatible via the flattened `extra` capture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartSpec {
    pub mark: Mark,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<FieldDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<YEncoding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<FieldDef>,
    /// Size channel: a quantitative field scaling per-point radius (bubble) for `Point`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<FieldDef>,
    /// Angular channel: the per-slice magnitude summed per category for the `Arc` mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theta: Option<FieldDef>,
    /// Data identifier (e.g. "sales.csv"). `None` => the data Table is supplied
    /// directly (the inline CSV body of a chart block). Inline YAML `values` is
    /// deferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default)]
    pub config: Config,
    /// Fields the model does not (yet) understand — preserved across round-trips.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yml::Value>,
}

/// The geometric mark a chart draws with. Bounded by the v1 backend (SPEC §2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark {
    Bar,
    Line,
    Point,
    Area,
    /// Bins a single quantitative `x` column into equal-width buckets and draws their
    /// frequency as bars; the `y` axis is the computed count (IMPLEMENTATION §17).
    Histogram,
    /// A radial pie/donut: each `color` category contributes a wedge sized by its summed
    /// `theta` value. Drawn without cartesian axes; `inner_radius` cuts a donut hole.
    Arc,
}

/// Bar/area drawing orientation. `Vertical` (the default) runs bars up from the x axis;
/// `Horizontal` lays categories down the y axis with bars extending rightward.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    Vertical,
    Horizontal,
}

/// The family of an axis scale transform. `Linear` is identity; `Log` is base-10; `Sqrt`
/// is the square-root transform (compresses large values). See [`Scale`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScaleKind {
    Linear,
    Log,
    Sqrt,
}

/// A per-axis scale: the transform `kind`, an optional explicit `domain` (data-space
/// `(min, max)`), and whether the auto domain should include zero. The backend applies
/// [`Scale::forward`] to coordinates and [`Scale::inverse`] in tick labels, so coordinates
/// stay `f64` and the single-coordinate-type design holds (IMPLEMENTATION §17.3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    #[serde(default = "linear_kind")]
    pub kind: ScaleKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<(f64, f64)>,
    #[serde(default)]
    pub zero: bool,
}

const fn linear_kind() -> ScaleKind {
    ScaleKind::Linear
}

impl Default for Scale {
    /// The identity scale: linear, no explicit domain, zero not forced.
    fn default() -> Self {
        Self { kind: ScaleKind::Linear, domain: None, zero: false }
    }
}

impl Scale {
    /// Map a data-space value into scaled space. `None` when the transform is undefined for
    /// the input (log of a value `<= 0`, sqrt of a negative) so the caller can drop the
    /// point and warn. Linear is the identity.
    pub fn forward(self, v: f64) -> Option<f64> {
        match self.kind {
            ScaleKind::Linear => Some(v),
            ScaleKind::Log => {
                if v > 0.0 {
                    Some(v.log10())
                } else {
                    None
                }
            }
            ScaleKind::Sqrt => {
                if v >= 0.0 {
                    Some(v.sqrt())
                } else {
                    None
                }
            }
        }
    }

    /// Map a scaled-space value back to data space — the inverse of [`Scale::forward`],
    /// used to format tick labels. Total (no `Option`): negative sqrt inputs cannot occur
    /// because they were never produced by `forward`.
    pub fn inverse(self, v: f64) -> f64 {
        match self.kind {
            ScaleKind::Linear => v,
            ScaleKind::Log => 10.0_f64.powf(v),
            ScaleKind::Sqrt => v * v,
        }
    }
}

/// A channel binding: `x: month` (shorthand) or `x: {field: month, type: temporal}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldDef {
    Shorthand(String),
    Full(FieldSpec),
}

/// The expanded object form of a channel binding: a field name and optional type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    pub field: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<DataType>,
}

/// `y: revenue` or `y: [revenue, profit]` (wide multi-series, SPEC §2.3.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum YEncoding {
    One(FieldDef),
    Many(Vec<FieldDef>),
}

/// The declared data type of a channel; authoritative over inference (SPEC §3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    Quantitative,
    Temporal,
    Ordinal,
    Nominal,
}

/// Line interpolation between consecutive points (SPEC §2.3 follow-up). `Linear`
/// joins points with straight segments; `Step` holds each value then steps to the
/// next (horizontal-then-vertical), giving a staircase line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interpolate {
    Linear,
    Step,
}

/// Presentation and styling options: titles, legend, palette, plus the mark-styling
/// knobs the host exposes (point/line sizes, fill opacity, stacking, interpolation,
/// gridlines). Every field is defaulted so an old spec without them round-trips and a
/// default `Config` serializes to (almost) nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_title: Option<String>,
    #[serde(default = "yes")]
    pub legend: bool,
    /// Hex palette override; defaults to the injected Theme when absent (SPEC §6.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<Vec<String>>,
    /// Point/scatter radius in pixels; the backend default applies when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point_size: Option<f32>,
    /// Line and area-border stroke width in pixels; backend default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_width: Option<f32>,
    /// Fill alpha (`0.0..=1.0`) for area and bar fills; backend default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_opacity: Option<f32>,
    /// Stack bar and area series on a running per-x cumulative baseline rather than
    /// drawing them grouped/overlaid (SPEC §2.3 follow-up).
    #[serde(default)]
    pub stack: bool,
    /// Line interpolation; `Linear` (the absent default) joins points straight,
    /// `Step` draws a staircase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpolate: Option<Interpolate>,
    /// Draw mesh gridlines behind the marks. Axis lines and labels are kept either
    /// way; only the interior grid is suppressed when false.
    #[serde(default = "yes")]
    pub show_grid: bool,
    /// Bar/area orientation; `Vertical` (the absent default) draws bars upward, `Horizontal`
    /// lays categories down the y axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<Orientation>,
    /// Histogram bucket count; the resolver applies a default of 20 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bins: Option<usize>,
    /// Donut hole radius as a `0.0..=1.0` fraction of the outer radius (`Arc`); 0 = full pie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_radius: Option<f32>,
    /// X-axis scale (log/sqrt/domain/zero); the linear identity applies when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_scale: Option<Scale>,
    /// Y-axis scale (log/sqrt/domain/zero); the linear identity applies when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_scale: Option<Scale>,
}

const fn yes() -> bool {
    true
}

impl Default for Config {
    /// Config defaults match the serde field defaults: no titles, legend on, no
    /// palette override, all styling knobs unset (backend defaults), no stacking,
    /// linear interpolation, gridlines shown. Hand-written because `legend` and
    /// `show_grid` default to `true`.
    fn default() -> Self {
        Self {
            title: None,
            x_title: None,
            y_title: None,
            legend: true,
            palette: None,
            point_size: None,
            line_width: None,
            fill_opacity: None,
            stack: false,
            interpolate: None,
            show_grid: true,
            orientation: None,
            bins: None,
            inner_radius: None,
            x_scale: None,
            y_scale: None,
        }
    }
}

impl FieldDef {
    /// Normalize this binding to a field name plus an optional declared type.
    pub const fn parts(&self) -> (&str, Option<DataType>) {
        match self {
            Self::Shorthand(name) => (name.as_str(), None),
            Self::Full(spec) => (spec.field.as_str(), spec.ty),
        }
    }
}

impl ChartSpec {
    /// Deserialize a `ChartSpec` from a YAML (or JSON, a YAML subset) string.
    pub fn from_yaml(s: &str) -> Result<Self, serde_yml::Error> {
        serde_yml::from_str(s)
    }

    /// Serialize this `ChartSpec` back to YAML, preserving captured `extra` fields.
    pub fn to_yaml(&self) -> Result<String, serde_yml::Error> {
        serde_yml::to_string(self)
    }

    /// The x channel's field name and optional declared type, if an x is bound.
    pub fn x_field(&self) -> Option<(&str, Option<DataType>)> {
        self.x.as_ref().map(FieldDef::parts)
    }

    /// The y channel's fields: empty, one, or many depending on the encoding.
    pub fn y_fields(&self) -> Vec<(&str, Option<DataType>)> {
        match &self.y {
            None => Vec::new(),
            Some(YEncoding::One(def)) => vec![def.parts()],
            Some(YEncoding::Many(defs)) => defs.iter().map(FieldDef::parts).collect(),
        }
    }

    /// The color channel's field name and optional declared type, if bound.
    pub fn color_field(&self) -> Option<(&str, Option<DataType>)> {
        self.color.as_ref().map(FieldDef::parts)
    }

    /// The size channel's field name and optional declared type, if bound (bubble radius).
    pub fn size_field(&self) -> Option<(&str, Option<DataType>)> {
        self.size.as_ref().map(FieldDef::parts)
    }

    /// The theta channel's field name and optional declared type, if bound (`Arc` magnitude).
    pub fn theta_field(&self) -> Option<(&str, Option<DataType>)> {
        self.theta.as_ref().map(FieldDef::parts)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChartSpec, Config, DataType, FieldDef, Interpolate, Mark, Orientation, Scale, ScaleKind,
        YEncoding,
    };

    #[test]
    fn parses_shorthand_and_list_y() {
        let yaml = "mark: line\nx: month\ny: [revenue, profit]\n";
        let spec = ChartSpec::from_yaml(yaml).unwrap();
        assert_eq!(spec.mark, Mark::Line);
        assert_eq!(spec.x_field(), Some(("month", None)));
        assert_eq!(
            spec.y_fields(),
            vec![("revenue", None), ("profit", None)]
        );
    }

    #[test]
    fn parses_full_field_with_type() {
        let yaml = "mark: point\nx: { field: t, type: temporal }\ny: v\n";
        let spec = ChartSpec::from_yaml(yaml).unwrap();
        assert_eq!(spec.x_field(), Some(("t", Some(DataType::Temporal))));
        assert!(matches!(spec.y, Some(YEncoding::One(FieldDef::Shorthand(_)))));
    }

    #[test]
    fn round_trip_preserves_extra_fields() {
        let yaml = "mark: bar\nx: month\ny: revenue\nfuture_field: 42\nnested:\n  a: b\n";
        let spec = ChartSpec::from_yaml(yaml).unwrap();
        assert!(spec.extra.contains_key("future_field"));
        assert!(spec.extra.contains_key("nested"));
        let out = spec.to_yaml().unwrap();
        let reparsed = ChartSpec::from_yaml(&out).unwrap();
        assert_eq!(spec, reparsed);
        assert!(out.contains("future_field"));
        assert!(out.contains("nested"));
    }

    #[test]
    fn legend_defaults_true_color_optional() {
        let spec = ChartSpec::from_yaml("mark: area\nx: a\ny: b\n").unwrap();
        assert!(spec.config.legend);
        assert_eq!(spec.color_field(), None);
        assert_eq!(spec.data, None);
    }

    #[test]
    fn config_round_trips() {
        let yaml = "mark: line\nx: a\ny: b\nconfig:\n  title: Sales\n  legend: false\n  palette: ['#ff0000']\n";
        let spec = ChartSpec::from_yaml(yaml).unwrap();
        assert_eq!(spec.config.title.as_deref(), Some("Sales"));
        assert!(!spec.config.legend);
        let reparsed = ChartSpec::from_yaml(&spec.to_yaml().unwrap()).unwrap();
        assert_eq!(spec, reparsed);
    }

    /// A default Config carries no optional styling knobs and serializes to nothing
    /// but the two always-on bool defaults (which serde still emits).
    #[test]
    fn default_config_is_almost_empty() {
        let c = Config::default();
        let out = serde_yml::to_string(&c).unwrap();
        assert!(!out.contains("title"));
        assert!(!out.contains("palette"));
        assert!(!out.contains("point_size"));
        assert!(!out.contains("line_width"));
        assert!(!out.contains("fill_opacity"));
        assert!(!out.contains("interpolate"));
        // Only the bool fields (legend, stack, show_grid) serialize, at defaults.
        assert!(out.contains("stack: false"));
        assert!(out.contains("legend: true"));
        assert!(out.contains("show_grid: true"));
    }

    /// Each new styling field deserializes and survives a round-trip.
    #[test]
    fn new_styling_fields_round_trip() {
        let yaml = "mark: area\nx: a\ny: b\nconfig:\n  point_size: 5.0\n  line_width: 3.5\n  fill_opacity: 0.25\n  stack: true\n  interpolate: step\n  show_grid: false\n";
        let spec = ChartSpec::from_yaml(yaml).unwrap();
        assert_eq!(spec.config.point_size, Some(5.0));
        assert_eq!(spec.config.line_width, Some(3.5));
        assert_eq!(spec.config.fill_opacity, Some(0.25));
        assert!(spec.config.stack);
        assert_eq!(spec.config.interpolate, Some(Interpolate::Step));
        assert!(!spec.config.show_grid);
        let reparsed = ChartSpec::from_yaml(&spec.to_yaml().unwrap()).unwrap();
        assert_eq!(spec, reparsed);
    }

    /// An OLD spec authored before the new fields existed still deserializes, with
    /// the new fields taking their defaults.
    #[test]
    fn old_spec_without_new_fields_deserializes() {
        let yaml = "mark: bar\nx: month\ny: [revenue, profit]\nconfig:\n  title: Old\n";
        let spec = ChartSpec::from_yaml(yaml).unwrap();
        assert_eq!(spec.config.point_size, None);
        assert_eq!(spec.config.line_width, None);
        assert_eq!(spec.config.fill_opacity, None);
        assert!(!spec.config.stack);
        assert_eq!(spec.config.interpolate, None);
        assert!(spec.config.show_grid);
        assert!(spec.config.legend);
    }

    /// The new marks deserialize from their lowercase tags.
    #[test]
    fn histogram_and_arc_marks_parse() {
        assert_eq!(
            ChartSpec::from_yaml("mark: histogram\nx: v\n").unwrap().mark,
            Mark::Histogram
        );
        assert_eq!(
            ChartSpec::from_yaml("mark: arc\ntheta: v\ncolor: c\n").unwrap().mark,
            Mark::Arc
        );
    }

    /// The size/theta channels and the new config fields deserialize and round-trip.
    #[test]
    fn size_theta_and_new_config_round_trip() {
        let yaml = "mark: point\nx: a\ny: b\nsize: pop\ntheta: amt\nconfig:\n  orientation: horizontal\n  bins: 12\n  inner_radius: 0.4\n  x_scale:\n    kind: log\n  y_scale:\n    kind: sqrt\n    domain: [0.0, 100.0]\n    zero: true\n";
        let spec = ChartSpec::from_yaml(yaml).unwrap();
        assert_eq!(spec.size_field(), Some(("pop", None)));
        assert_eq!(spec.theta_field(), Some(("amt", None)));
        assert_eq!(spec.config.orientation, Some(Orientation::Horizontal));
        assert_eq!(spec.config.bins, Some(12));
        assert_eq!(spec.config.inner_radius, Some(0.4));
        assert_eq!(spec.config.x_scale.map(|s| s.kind), Some(ScaleKind::Log));
        let y = spec.config.y_scale.unwrap();
        assert_eq!(y.kind, ScaleKind::Sqrt);
        assert_eq!(y.domain, Some((0.0, 100.0)));
        assert!(y.zero);
        let reparsed = ChartSpec::from_yaml(&spec.to_yaml().unwrap()).unwrap();
        assert_eq!(spec, reparsed);
    }

    /// Linear scale is the identity in both directions.
    #[test]
    fn linear_scale_is_identity() {
        let s = Scale::default();
        assert_eq!(s.forward(7.5), Some(7.5));
        assert_eq!(s.inverse(7.5), 7.5);
    }

    /// Log scale: forward is base-10, inverse is the power; log of `<= 0` is `None`.
    #[test]
    fn log_scale_forward_inverse_and_nonpositive() {
        let s = Scale { kind: ScaleKind::Log, domain: None, zero: false };
        assert_eq!(s.forward(100.0), Some(2.0));
        assert_eq!(s.forward(0.0), None);
        assert_eq!(s.forward(-5.0), None);
        assert!((s.inverse(2.0) - 100.0).abs() < 1e-9);
    }

    /// Sqrt scale: forward is the root, inverse is the square; sqrt of a negative is `None`.
    #[test]
    fn sqrt_scale_forward_inverse_and_negative() {
        let s = Scale { kind: ScaleKind::Sqrt, domain: None, zero: false };
        assert_eq!(s.forward(9.0), Some(3.0));
        assert_eq!(s.forward(0.0), Some(0.0));
        assert_eq!(s.forward(-1.0), None);
        assert!((s.inverse(3.0) - 9.0).abs() < 1e-9);
    }
}
