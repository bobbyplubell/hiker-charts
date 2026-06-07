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

/// Presentation options: titles, legend toggle, and an optional palette override.
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
}

const fn yes() -> bool {
    true
}

impl Default for Config {
    /// Config defaults match the serde field defaults: no titles, legend on, no
    /// palette override. Hand-written because `legend` defaults to `true`.
    fn default() -> Self {
        Self {
            title: None,
            x_title: None,
            y_title: None,
            legend: true,
            palette: None,
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
}

#[cfg(test)]
mod tests {
    use super::{ChartSpec, DataType, FieldDef, Mark, YEncoding};

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
}
