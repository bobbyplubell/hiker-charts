//! The raw tabular data a host's resolver yields: a header plus string cells.
//!
//! Typing happens later in `typing`; this layer stores columns of unparsed text
//! exactly as read. The crate never reads files itself (SPEC §3.1) — `from_csv`
//! parses bytes a host already loaded, e.g. an inline chart-block body.

/// A table is an ordered list of named columns of raw string cells.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Table {
    pub columns: Vec<Column>,
}

/// A single named column: its header name and one raw cell per row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub cells: Vec<String>,
}

impl Table {
    /// Parse a CSV byte slice into a `Table`. The first record is the header;
    /// each subsequent record contributes one cell per column. Missing trailing
    /// fields in a short row are stored as empty strings.
    pub fn from_csv(bytes: &[u8]) -> Result<Self, csv::Error> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_reader(bytes);
        let headers = reader.headers()?.clone();
        let mut columns: Vec<Column> = headers
            .iter()
            .map(|name| Column {
                name: name.to_string(),
                cells: Vec::new(),
            })
            .collect();
        for record in reader.records() {
            let record = record?;
            for (i, col) in columns.iter_mut().enumerate() {
                col.cells.push(record.get(i).unwrap_or("").to_string());
            }
        }
        Ok(Self { columns })
    }

    /// Look up a column by exact name, if present.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The number of data rows (the length of the longest column).
    pub fn row_count(&self) -> usize {
        self.columns.iter().map(|c| c.cells.len()).max().unwrap_or(0)
    }

    /// Serialize the table back to a CSV string: the header row, then one record
    /// per row (a short column contributes an empty cell). The inverse of
    /// [`from_csv`](Self::from_csv), used to emit a self-contained ```` ```chart
    /// ```` block from a builder seeded with a table rather than verbatim CSV
    /// bytes. Round-trips through `csv`'s quoting, so commas / quotes / newlines
    /// in a cell survive. Returns an empty string on the (practically
    /// unreachable) writer error.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut wtr = csv::Writer::from_writer(Vec::new());
        let headers: Vec<&str> = self.columns.iter().map(|c| c.name.as_str()).collect();
        if wtr.write_record(&headers).is_err() {
            return String::new();
        }
        for row in 0..self.row_count() {
            let record: Vec<&str> = self
                .columns
                .iter()
                .map(|c| c.cells.get(row).map_or("", String::as_str))
                .collect();
            if wtr.write_record(&record).is_err() {
                return String::new();
            }
        }
        wtr.into_inner()
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::Table;

    #[test]
    fn parses_header_and_rows() {
        let csv = b"month,revenue\njan,100\nfeb,140\n";
        let table = Table::from_csv(csv).unwrap();
        assert_eq!(table.columns.len(), 2);
        assert_eq!(table.column("month").unwrap().cells, vec!["jan", "feb"]);
        assert_eq!(table.column("revenue").unwrap().cells, vec!["100", "140"]);
        assert_eq!(table.row_count(), 2);
    }

    #[test]
    fn missing_column_is_none() {
        let table = Table::from_csv(b"a,b\n1,2\n").unwrap();
        assert!(table.column("c").is_none());
    }

    #[test]
    fn to_csv_round_trips() {
        let csv = "month,revenue\njan,100\nfeb,140\n";
        let table = Table::from_csv(csv.as_bytes()).unwrap();
        let out = table.to_csv();
        assert_eq!(Table::from_csv(out.as_bytes()).unwrap(), table);
        assert_eq!(out, csv);
    }

    #[test]
    fn to_csv_quotes_cells_with_commas() {
        let table = Table::from_csv(b"name,note\nx,\"a,b\"\n").unwrap();
        // The comma-bearing cell is re-quoted, so the round-trip is exact.
        assert_eq!(Table::from_csv(table.to_csv().as_bytes()).unwrap(), table);
    }

    #[test]
    fn short_rows_padded_with_empty() {
        let table = Table::from_csv(b"a,b,c\n1,2\n").unwrap();
        assert_eq!(table.column("c").unwrap().cells, vec![""]);
    }

    #[test]
    fn empty_table_has_zero_rows() {
        let table = Table::default();
        assert_eq!(table.row_count(), 0);
    }
}
