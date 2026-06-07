//! UI- and renderer-agnostic core for `hiker-charts`.
//!
//! Houses the `ChartSpec` grammar-of-graphics model (the DSL, deserialized from YAML),
//! raw tabular data, type inference and self-contained coercion, the renderer-neutral
//! `ResolvedChart` plus the `Backend` trait it is painted through, content hashing,
//! data-dependency extraction, and the host interfaces (data resolver, theme). This crate
//! never depends on a charting library or a GUI toolkit — see `IMPLEMENTATION.md` §2.

pub mod backend;
pub mod data;
pub mod deps;
pub mod diag;
pub mod dsl;
pub mod host;
pub mod identity;
pub mod resolve;
pub mod theme;
pub mod typing;
