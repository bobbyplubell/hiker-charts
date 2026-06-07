//! Headless `hiker-charts` renderer: `spec.yaml + data.csv -> SVG`.
//!
//! The CLI is itself a host, so it may read the filesystem: it loads a YAML `ChartSpec`
//! and a CSV table, resolves them to a `ResolvedChart`, paints via the plotters backend,
//! and writes the SVG. Also the snapshot-test / batch-export driver. See
//! `IMPLEMENTATION.md` §4.

use std::io::Write;

use hiker_charts_core::backend::{Backend, Size};
use hiker_charts_core::diag::{Diagnostic, Severity};
use hiker_charts_core::dsl::ChartSpec;
use hiker_charts_core::data::Table;
use hiker_charts_core::resolve::resolve;
use hiker_charts_core::theme::Theme;
use hiker_charts_plotters::PlottersSvg;

/// The pixel size every chart is painted into (SPEC default canvas).
const CANVAS: Size = Size { width: 800, height: 500 };

/// Parsed command line: the two input paths plus an optional output path.
struct Args {
    spec_path: String,
    data_path: String,
    out_path: Option<String>,
}

fn main() {
    if let Err(message) = run(std::env::args().skip(1).collect()) {
        eprintln!("hiker-charts: {message}");
        std::process::exit(1);
    }
}

/// Parse args, read the inputs, render, and write the SVG to `-o` or stdout.
fn run(argv: Vec<String>) -> Result<(), String> {
    let args = parse_args(argv)?;
    let spec_yaml = std::fs::read_to_string(&args.spec_path)
        .map_err(|e| format!("reading spec `{}`: {e}", args.spec_path))?;
    let csv = std::fs::read(&args.data_path)
        .map_err(|e| format!("reading data `{}`: {e}", args.data_path))?;
    let svg = render_svg(&spec_yaml, &csv)?;
    write_output(args.out_path.as_deref(), &svg)
}

/// Turn the YAML spec and CSV bytes into an SVG string, mapping every failure to a
/// readable message. Path-free so it can be unit-tested without touching the filesystem.
fn render_svg(spec_yaml: &str, csv: &[u8]) -> Result<String, String> {
    let spec = ChartSpec::from_yaml(spec_yaml).map_err(|e| format!("parsing spec: {e}"))?;
    let table = Table::from_csv(csv).map_err(|e| format!("parsing data: {e}"))?;
    let chart = resolve(&spec, &table).map_err(|diags| format_diagnostics(&diags))?;
    let output = PlottersSvg
        .render(&chart, &Theme::default(), CANVAS)
        .map_err(|e| format!("rendering chart: {e}"))?;
    Ok(output.svg)
}

/// Parse `<spec.yaml> <data.csv> [-o <out.svg>]` from the argument list (no clap).
fn parse_args(argv: Vec<String>) -> Result<Args, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut out_path: Option<String> = None;
    let mut iter = argv.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                let path = iter.next().ok_or("`-o` needs an output path")?;
                out_path = Some(path);
            }
            "-h" | "--help" => return Err(usage()),
            other if other.starts_with('-') => {
                return Err(format!("unknown flag `{other}`\n{}", usage()));
            }
            _ => positional.push(arg),
        }
    }
    if positional.len() != 2 {
        return Err(format!(
            "expected a spec and a data path, got {}\n{}",
            positional.len(),
            usage()
        ));
    }
    let mut paths = positional.into_iter();
    Ok(Args {
        spec_path: paths.next().unwrap_or_default(),
        data_path: paths.next().unwrap_or_default(),
        out_path,
    })
}

/// Write the SVG to the given path, or to stdout when no path was supplied.
fn write_output(out_path: Option<&str>, svg: &str) -> Result<(), String> {
    match out_path {
        Some(path) => {
            std::fs::write(path, svg).map_err(|e| format!("writing `{path}`: {e}"))
        }
        None => std::io::stdout()
            .write_all(svg.as_bytes())
            .map_err(|e| format!("writing stdout: {e}")),
    }
}

/// Render resolve diagnostics into one readable, multi-line error message.
fn format_diagnostics(diags: &[Diagnostic]) -> String {
    let mut out = String::from("could not resolve chart:");
    for d in diags {
        let label = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        out.push_str(&format!("\n  {label}: {}", d.message));
    }
    out
}

/// The usage line shown on argument errors and `--help`.
fn usage() -> String {
    "usage: hiker-charts <spec.yaml> <data.csv> [-o <out.svg>]".to_string()
}

#[cfg(test)]
mod tests {
    use super::{format_diagnostics, parse_args, render_svg};
    use hiker_charts_core::diag::Diagnostic;

    const SPEC: &str = "mark: line\nx: month\ny: revenue\n";
    const CSV: &str = "month,revenue\n2024-01,10\n2024-02,20\n2024-03,15\n";

    #[test]
    fn render_path_produces_non_empty_svg() {
        let svg = render_svg(SPEC, CSV.as_bytes()).expect("render should succeed");
        assert!(svg.contains("<svg"), "output should be an SVG document");
        assert!(svg.len() > 100, "SVG should have real content");
    }

    #[test]
    fn bad_yaml_is_reported() {
        let err = render_svg("mark: : :\n", CSV.as_bytes()).unwrap_err();
        assert!(err.contains("parsing spec"), "got: {err}");
    }

    #[test]
    fn missing_column_surfaces_diagnostics() {
        let err = render_svg("mark: line\nx: nope\ny: revenue\n", CSV.as_bytes()).unwrap_err();
        assert!(err.contains("resolve"), "got: {err}");
    }

    #[test]
    fn parse_args_reads_output_flag() {
        let args = parse_args(vec![
            "spec.yaml".into(),
            "data.csv".into(),
            "-o".into(),
            "out.svg".into(),
        ])
        .expect("valid args");
        assert_eq!(args.spec_path, "spec.yaml");
        assert_eq!(args.data_path, "data.csv");
        assert_eq!(args.out_path.as_deref(), Some("out.svg"));
    }

    #[test]
    fn parse_args_requires_two_positionals() {
        assert!(parse_args(vec!["only-one.yaml".into()]).is_err());
        assert!(parse_args(vec!["-o".into(), "out.svg".into()]).is_err());
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        match parse_args(vec!["--bogus".into()]) {
            Err(err) => assert!(err.contains("unknown flag"), "got: {err}"),
            Ok(_) => panic!("unknown flag should be rejected"),
        }
    }

    #[test]
    fn diagnostics_format_with_severity_labels() {
        let text = format_diagnostics(&[
            Diagnostic::error("missing column"),
            Diagnostic::warning("dropped a row"),
        ]);
        assert!(text.contains("error: missing column"));
        assert!(text.contains("warning: dropped a row"));
    }
}
