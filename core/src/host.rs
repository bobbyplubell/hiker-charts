//! The host integration seam for data: the `DataResolver` trait.
//!
//! A spec references its data by an opaque identifier; the host implements this
//! trait to map that identifier to a `Table`, keeping file access, sandboxing, and
//! network policy host-side (SPEC §3.1, §10). The core never reads files itself.

use crate::data::Table;

/// An error a resolver returns when an identifier cannot be mapped to a table.
/// Wraps a host-supplied message (e.g. "file not found", "permission denied").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveError(pub String);

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ResolveError {}

/// Implemented by a host to turn a data identifier into a raw `Table`. The core
/// calls this; it never interprets, rewrites, or persists the identifier itself.
pub trait DataResolver {
    /// Resolve a data identifier to a table, or fail with a host-supplied message.
    fn resolve(&self, id: &str) -> Result<Table, ResolveError>;
}

#[cfg(test)]
mod tests {
    use super::{DataResolver, ResolveError};
    use crate::data::Table;

    struct Fixed;

    impl DataResolver for Fixed {
        fn resolve(&self, id: &str) -> Result<Table, ResolveError> {
            if id == "ok.csv" {
                Table::from_csv(b"a,b\n1,2\n").map_err(|e| ResolveError(e.to_string()))
            } else {
                Err(ResolveError(format!("unknown id: {id}")))
            }
        }
    }

    #[test]
    fn resolver_returns_table_for_known_id() {
        let table = Fixed.resolve("ok.csv").unwrap();
        assert_eq!(table.columns.len(), 2);
    }

    #[test]
    fn resolver_errors_for_unknown_id() {
        let err = Fixed.resolve("nope.csv").unwrap_err();
        assert_eq!(err.to_string(), "unknown id: nope.csv");
    }
}
