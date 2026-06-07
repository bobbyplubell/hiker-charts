//! Input-keyed content hashing for cache invalidation (SPEC §7).
//!
//! The hash covers the chart's *inputs* — spec, table, theme, and size — not the
//! rendered bytes (plotters' SVG is not byte-stable, SPEC §4.4). A host keys its
//! rendered-output cache on this value: any input change yields a new hash and a
//! re-render. NOTE: this uses std's `DefaultHasher`, which is session-scoped, not
//! cross-process stable — fine for an in-process per-frame cache key, not for
//! persistence across runs.

use crate::backend::Size;
use crate::data::Table;
use crate::dsl::ChartSpec;
use crate::theme::Theme;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Compute a content hash over the four render inputs. Equal inputs (within a
/// process) yield equal hashes; any change to any input changes the hash.
pub fn content_hash(spec: &ChartSpec, table: &Table, theme: &Theme, size: Size) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_spec(spec, &mut hasher);
    hash_table(table, &mut hasher);
    hash_theme(theme, &mut hasher);
    size.hash(&mut hasher);
    hasher.finish()
}

/// Fold the spec into the hasher via its canonical YAML serialization, so every
/// channel, config option, and captured `extra` field contributes.
fn hash_spec(spec: &ChartSpec, hasher: &mut DefaultHasher) {
    match spec.to_yaml() {
        Ok(yaml) => yaml.hash(hasher),
        Err(_) => {
            // Unserializable spec: fall back to the debug form so distinct specs
            // still differ. Should not occur for well-formed specs.
            format!("{spec:?}").hash(hasher);
        }
    }
}

/// Fold every column name and cell into the hasher in row-major order.
fn hash_table(table: &Table, hasher: &mut DefaultHasher) {
    for column in &table.columns {
        column.name.hash(hasher);
        for cell in &column.cells {
            cell.hash(hasher);
        }
    }
}

/// Fold the theme's structural colors and series palette into the hasher.
fn hash_theme(theme: &Theme, hasher: &mut DefaultHasher) {
    theme.background.hash(hasher);
    theme.foreground.hash(hasher);
    theme.gridline.hash(hasher);
    theme.series.hash(hasher);
}

#[cfg(test)]
mod tests {
    use super::content_hash;
    use crate::backend::Size;
    use crate::data::Table;
    use crate::dsl::ChartSpec;
    use crate::theme::{Color, Theme};

    fn inputs() -> (ChartSpec, Table, Theme, Size) {
        let spec = ChartSpec::from_yaml("mark: line\nx: month\ny: v\n").unwrap();
        let table = Table::from_csv(b"month,v\njan,1\n").unwrap();
        let size = Size { width: 640, height: 480 };
        (spec, table, Theme::default(), size)
    }

    #[test]
    fn identical_inputs_hash_equal() {
        let (s, t, th, sz) = inputs();
        assert_eq!(content_hash(&s, &t, &th, sz), content_hash(&s, &t, &th, sz));
    }

    #[test]
    fn spec_change_changes_hash() {
        let (s, t, th, sz) = inputs();
        let base = content_hash(&s, &t, &th, sz);
        let s2 = ChartSpec::from_yaml("mark: bar\nx: month\ny: v\n").unwrap();
        assert_ne!(base, content_hash(&s2, &t, &th, sz));
    }

    #[test]
    fn data_change_changes_hash() {
        let (s, _t, th, sz) = inputs();
        let t2 = Table::from_csv(b"month,v\njan,2\n").unwrap();
        let t1 = Table::from_csv(b"month,v\njan,1\n").unwrap();
        assert_ne!(content_hash(&s, &t1, &th, sz), content_hash(&s, &t2, &th, sz));
    }

    #[test]
    fn theme_and_size_change_the_hash() {
        let (s, t, th, sz) = inputs();
        let base = content_hash(&s, &t, &th, sz);
        let mut th2 = th.clone();
        th2.background = Color::rgb(0, 0, 0);
        assert_ne!(base, content_hash(&s, &t, &th2, sz));
        let sz2 = Size { width: 800, height: 480 };
        assert_ne!(base, content_hash(&s, &t, &th, sz2));
    }
}
