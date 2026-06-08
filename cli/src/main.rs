//! Headless `hiker-charts` renderer: `spec.yaml + data.csv -> SVG`.
//!
//! The CLI is itself a host, so it may read the filesystem: it loads a YAML `ChartSpec`
//! and a CSV table, resolves them to a `ResolvedChart`, paints via the plotters backend,
//! and writes the SVG. Also the snapshot-test / batch-export driver. See
//! `IMPLEMENTATION.md` §4.

use std::io::Write;

use hiker_charts_core::backend::{Backend, Size};
use hiker_charts_core::block::parse_block;
use hiker_charts_core::diag::{Diagnostic, Severity};
use hiker_charts_core::dsl::ChartSpec;
use hiker_charts_core::data::Table;
use hiker_charts_core::resolve::resolve;
use hiker_charts_core::theme::Theme;
use hiker_charts_plotters::PlottersSvg;

/// The pixel size every chart is painted into (SPEC default canvas).
const CANVAS: Size = Size { width: 800, height: 500 };

/// Parsed command line: the input(s) plus an optional output path. A chart block
/// (config + `---` + inline CSV) is given as a single file; an external-data chart
/// is a spec file plus a separate CSV file.
enum Args {
    /// One self-contained block file carrying both config and inline CSV.
    Block { block_path: String, out_path: Option<String> },
    /// A spec file and a separate CSV data file.
    Pair { spec_path: String, data_path: String, out_path: Option<String> },
}

fn main() {
    if let Err(message) = run(std::env::args().skip(1).collect()) {
        eprintln!("hiker-charts: {message}");
        std::process::exit(1);
    }
}

/// Parse args, read the inputs, render, and write the SVG to `-o` or stdout.
fn run(argv: Vec<String>) -> Result<(), String> {
    match parse_args(argv)? {
        Args::Block { block_path, out_path } => {
            let body = std::fs::read_to_string(&block_path)
                .map_err(|e| format!("reading block `{block_path}`: {e}"))?;
            let svg = render_block(&body)?;
            write_output(out_path.as_deref(), &svg)
        }
        Args::Pair { spec_path, data_path, out_path } => {
            let spec_yaml = std::fs::read_to_string(&spec_path)
                .map_err(|e| format!("reading spec `{spec_path}`: {e}"))?;
            let csv = std::fs::read(&data_path)
                .map_err(|e| format!("reading data `{data_path}`: {e}"))?;
            let svg = render_svg(&spec_yaml, &csv)?;
            write_output(out_path.as_deref(), &svg)
        }
    }
}

/// Render a self-contained chart block (config + `---` + inline CSV) to an SVG. The
/// block must carry its data inline; a block with no `---` section has no data to
/// plot from the CLI (it would reference an external `data:` file a host resolves).
/// Path-free so it can be unit-tested without touching the filesystem.
fn render_block(body: &str) -> Result<String, String> {
    let parsed = parse_block(body).map_err(|diags| format_diagnostics(&diags))?;
    let table = parsed
        .table
        .ok_or("block has no inline data (expected a `---` line followed by CSV)")?;
    let chart = resolve(&parsed.spec, &table).map_err(|diags| format_diagnostics(&diags))?;
    let output = PlottersSvg
        .render(&chart, &Theme::default(), CANVAS)
        .map_err(|e| format!("rendering chart: {e}"))?;
    Ok(output.svg)
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

/// Parse `<block.chart>` or `<spec.yaml> <data.csv>`, plus an optional `-o <out.svg>`,
/// from the argument list (no clap). One positional is a self-contained block with
/// inline CSV; two are a spec and a separate data file.
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
    let mut paths = positional.into_iter();
    match (paths.next(), paths.next(), paths.next()) {
        (Some(block_path), None, _) => Ok(Args::Block { block_path, out_path }),
        (Some(spec_path), Some(data_path), None) => {
            Ok(Args::Pair { spec_path, data_path, out_path })
        }
        _ => Err(format!("expected a block file, or a spec and a data path\n{}", usage())),
    }
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
    "usage: hiker-charts <block.chart> [-o <out.svg>]\n   or: hiker-charts <spec.yaml> <data.csv> [-o <out.svg>]".to_string()
}

#[cfg(test)]
mod tests {
    use super::{format_diagnostics, parse_args, render_block, render_svg, Args};
    use hiker_charts_core::diag::Diagnostic;

    const SPEC: &str = "mark: line\nx: month\ny: revenue\n";
    const CSV: &str = "month,revenue\n2024-01,10\n2024-02,20\n2024-03,15\n";
    const BLOCK: &str = "mark: line\nx: month\ny: revenue\n---\nmonth,revenue\n2024-01,10\n2024-02,20\n";

    #[test]
    fn render_path_produces_non_empty_svg() {
        let svg = render_svg(SPEC, CSV.as_bytes()).expect("render should succeed");
        assert!(svg.contains("<svg"), "output should be an SVG document");
        assert!(svg.len() > 100, "SVG should have real content");
    }

    #[test]
    fn render_block_with_inline_csv_produces_svg() {
        let svg = render_block(BLOCK).expect("inline block should render");
        assert!(svg.contains("<svg"), "output should be an SVG document");
    }

    #[test]
    fn render_block_without_data_is_an_error() {
        let err = render_block("mark: line\nx: month\ny: revenue\n").unwrap_err();
        assert!(err.contains("no inline data"), "got: {err}");
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
    fn parse_args_reads_pair_with_output_flag() {
        let args = parse_args(vec![
            "spec.yaml".into(),
            "data.csv".into(),
            "-o".into(),
            "out.svg".into(),
        ])
        .expect("valid args");
        match args {
            Args::Pair { spec_path, data_path, out_path } => {
                assert_eq!(spec_path, "spec.yaml");
                assert_eq!(data_path, "data.csv");
                assert_eq!(out_path.as_deref(), Some("out.svg"));
            }
            Args::Block { .. } => panic!("two positionals should parse as a pair"),
        }
    }

    #[test]
    fn parse_args_reads_single_block_file() {
        let args = parse_args(vec!["chart.block".into(), "-o".into(), "out.svg".into()])
            .expect("valid args");
        match args {
            Args::Block { block_path, out_path } => {
                assert_eq!(block_path, "chart.block");
                assert_eq!(out_path.as_deref(), Some("out.svg"));
            }
            Args::Pair { .. } => panic!("one positional should parse as a block"),
        }
    }

    #[test]
    fn parse_args_requires_at_least_one_positional() {
        assert!(parse_args(vec![]).is_err());
        assert!(parse_args(vec!["-o".into(), "out.svg".into()]).is_err());
        assert!(parse_args(vec!["a".into(), "b".into(), "c".into()]).is_err());
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
