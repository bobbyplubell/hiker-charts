//! Diagnostics surfaced to the host: parse errors, unknown fields, dropped rows.
//!
//! A `Diagnostic` is a severity plus a human-readable message. The resolver and
//! typing layers emit these instead of failing silently (SPEC §2.2, §8.1), so a
//! host can show authors exactly what was malformed or dropped.

/// How serious a diagnostic is. `Error` means the chart could not be built;
/// `Warning` means it was built but something was skipped (e.g. dropped rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A single reported issue: a severity and a message describing what happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    /// Construct an error-severity diagnostic from any displayable message.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
        }
    }

    /// Construct a warning-severity diagnostic from any displayable message.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, Severity};

    #[test]
    fn error_constructor_sets_severity() {
        let d = Diagnostic::error("missing column `month`");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.message, "missing column `month`");
    }

    #[test]
    fn warning_constructor_sets_severity() {
        let d = Diagnostic::warning("dropped 2 rows");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.message, "dropped 2 rows");
    }

    #[test]
    fn diagnostics_compare_by_value() {
        assert_eq!(Diagnostic::error("x"), Diagnostic::error("x"));
        assert_ne!(Diagnostic::error("x"), Diagnostic::warning("x"));
    }
}
