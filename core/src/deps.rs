//! Data-dependency extraction so a host can watch the sources a spec needs.
//!
//! The component never watches or polls anything itself (SPEC §3.2); it only
//! reports the set of opaque data identifiers a spec references, so the host can
//! watch those and request a re-render on change. In v1 a spec names at most one
//! source (`spec.data`); inline-bodied blocks reference none.

use crate::dsl::ChartSpec;

/// The data identifiers a spec depends on. In v1 this is `spec.data` lifted into a
/// `Vec` (empty when the data is supplied inline). Identifiers are opaque — never
/// interpreted, resolved, or rewritten here (SPEC §3.3).
pub fn data_dependencies(spec: &ChartSpec) -> Vec<String> {
    match &spec.data {
        Some(id) if !id.is_empty() => vec![id.clone()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::data_dependencies;
    use crate::dsl::ChartSpec;

    #[test]
    fn external_data_is_a_dependency() {
        let spec = ChartSpec::from_yaml("mark: line\nx: a\ny: b\ndata: sales.csv\n").unwrap();
        assert_eq!(data_dependencies(&spec), vec!["sales.csv".to_string()]);
    }

    #[test]
    fn inline_data_has_no_dependencies() {
        let spec = ChartSpec::from_yaml("mark: line\nx: a\ny: b\n").unwrap();
        assert!(data_dependencies(&spec).is_empty());
    }

    #[test]
    fn empty_identifier_is_ignored() {
        let spec = ChartSpec::from_yaml("mark: line\nx: a\ny: b\ndata: ''\n").unwrap();
        assert!(data_dependencies(&spec).is_empty());
    }

    #[test]
    fn identifier_is_passed_through_verbatim() {
        let spec =
            ChartSpec::from_yaml("mark: line\nx: a\ny: b\ndata: ../some/Path To.csv\n").unwrap();
        assert_eq!(data_dependencies(&spec), vec!["../some/Path To.csv".to_string()]);
    }
}
