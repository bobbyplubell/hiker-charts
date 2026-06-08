//! Splitting a chart block into its YAML config and optional inline CSV body.
//!
//! A ```` ```chart ```` block may carry its data *inline*: the YAML config, then a
//! line that is exactly `---`, then a raw CSV body (SPEC §8.3). This keeps a chart
//! fully self-contained in one fenced block — the common case when an author types
//! or pastes a small table rather than referencing an external `data:` file.
//!
//! The host hands the fence's inner text to [`parse_block`] and gets back the
//! `ChartSpec` plus, when present, the `Table` parsed from the inline CSV — without
//! needing to know the `---` convention itself. This is the read half of the
//! round-trip whose write half is `BuilderState::save_block`, which emits this exact
//! shape (config, a `---` line, then the CSV body re-attached verbatim).

use crate::data::Table;
use crate::diag::Diagnostic;
use crate::dsl::ChartSpec;

/// A chart block parsed into its spec and, when the block carried a `---` data
/// section, the table read from the inline CSV.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedBlock {
    /// The chart spec deserialized from the YAML above the separator.
    pub spec: ChartSpec,
    /// The table parsed from the inline CSV body, or `None` when the block has no
    /// `---` section (the host resolves `spec.data` through its own resolver instead).
    pub table: Option<Table>,
    /// The verbatim CSV body, retained so an edit-and-save can re-attach the bytes
    /// unchanged (the input to `BuilderState::from_block`); `None` with no `---` section.
    pub csv_body: Option<String>,
}

/// Split a block body on the first line that is exactly `---` (ignoring surrounding
/// whitespace) into the YAML config above and the raw CSV body below. Returns
/// `(config, None)` when there is no separator line, so a block that references
/// external data is unaffected. The CSV slice begins immediately after the
/// separator line's newline, so it is the body byte-for-byte.
#[must_use]
pub fn split_block(body: &str) -> (&str, Option<&str>) {
    let mut offset = 0usize;
    for line in body.split_inclusive('\n') {
        if line.trim() == "---" {
            return (&body[..offset], Some(&body[offset + line.len()..]));
        }
        offset += line.len();
    }
    (body, None)
}

/// Parse a chart block body into its spec and inline table (SPEC §8.3): the YAML
/// above the first `---` line becomes the [`ChartSpec`]; the CSV below it becomes
/// the [`Table`]. A block with no `---` parses to a spec with `table: None` (its
/// `data:` identifier is the host's to resolve). Errors are returned as
/// [`Diagnostic`]s — the same currency as [`crate::resolve::resolve`] — so a host can
/// surface a malformed config or unparseable CSV the same way it surfaces resolve errors.
pub fn parse_block(body: &str) -> Result<ParsedBlock, Vec<Diagnostic>> {
    let (config, csv) = split_block(body);
    let spec = ChartSpec::from_yaml(config)
        .map_err(|e| vec![Diagnostic::error(format!("chart config: {e}"))])?;
    let (table, csv_body) = match csv {
        Some(csv) => {
            let table = Table::from_csv(csv.as_bytes())
                .map_err(|e| vec![Diagnostic::error(format!("inline csv: {e}"))])?;
            (Some(table), Some(csv.to_string()))
        }
        None => (None, None),
    };
    Ok(ParsedBlock { spec, table, csv_body })
}

#[cfg(test)]
mod tests {
    use super::{parse_block, split_block};
    use crate::dsl::Mark;

    const BLOCK: &str = "mark: line\nx: month\ny: revenue\n---\nmonth,revenue\n2024-01,100\n2024-02,140\n";

    #[test]
    fn splits_config_from_csv_on_separator() {
        let (config, csv) = split_block(BLOCK);
        assert_eq!(config, "mark: line\nx: month\ny: revenue\n");
        assert_eq!(csv, Some("month,revenue\n2024-01,100\n2024-02,140\n"));
    }

    #[test]
    fn no_separator_means_no_inline_csv() {
        let (config, csv) = split_block("mark: bar\nx: a\ny: b\n");
        assert_eq!(config, "mark: bar\nx: a\ny: b\n");
        assert_eq!(csv, None);
    }

    #[test]
    fn separator_tolerates_trailing_whitespace_and_crlf() {
        let body = "mark: line\r\nx: a\r\ny: b\r\n---  \r\na,b\r\n1,2\r\n";
        let (config, csv) = split_block(body);
        assert_eq!(config, "mark: line\r\nx: a\r\ny: b\r\n");
        assert_eq!(csv, Some("a,b\r\n1,2\r\n"));
    }

    #[test]
    fn only_the_first_separator_splits() {
        // A `---` inside the CSV body (unlikely, but possible) stays in the data.
        let body = "mark: line\nx: a\ny: b\n---\na,b\n1,2\n---\n3,4\n";
        let (_, csv) = split_block(body);
        assert_eq!(csv, Some("a,b\n1,2\n---\n3,4\n"));
    }

    #[test]
    fn parse_block_yields_spec_and_table() {
        let parsed = parse_block(BLOCK).expect("valid block parses");
        assert_eq!(parsed.spec.mark, Mark::Line);
        let table = parsed.table.expect("inline csv yields a table");
        assert_eq!(table.columns.len(), 2);
        assert_eq!(table.row_count(), 2);
        assert_eq!(parsed.csv_body.as_deref(), Some("month,revenue\n2024-01,100\n2024-02,140\n"));
    }

    #[test]
    fn parse_block_without_csv_has_no_table() {
        let parsed = parse_block("mark: bar\nx: a\ny: b\ndata: sales.csv\n").expect("parses");
        assert!(parsed.table.is_none());
        assert!(parsed.csv_body.is_none());
        assert_eq!(parsed.spec.data.as_deref(), Some("sales.csv"));
    }

    #[test]
    fn bad_config_is_a_diagnostic() {
        let diags = parse_block("mark: : :\n---\na,b\n1,2\n").unwrap_err();
        assert!(diags[0].message.contains("chart config"), "got: {:?}", diags);
    }
}
