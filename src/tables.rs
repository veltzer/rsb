//! Centralized table rendering for CLI output.
//!
//! All tables emitted by `rsconstruct` go through this module. Changing
//! the visual style here changes every table at once. The two public
//! entry points are `print_table` (header + rows) and
//! `print_table_with_total` (header + rows + a separated total row at
//! the bottom).
//!
//! Both renderers use `tabled::Style::rounded()` and always draw a
//! horizontal separator below the header row.

use tabled::Table;
use tabled::builder::Builder as TableBuilder;
use tabled::settings::Style;
use tabled::settings::style::HorizontalLine;

fn build_table(headers: &[&str], rows: &[Vec<String>]) -> Table {
    let mut builder = TableBuilder::new();
    builder.push_record(headers.iter().copied());
    for row in rows {
        builder.push_record(row.iter().map(std::string::String::as_str));
    }
    builder.build()
}

/// Print a table with an explicit header row. A horizontal separator is drawn
/// between the header and the data rows.
///
/// Routed through `output::info`: tables are human prose, so `--quiet`
/// silences them and `--json` keeps them out of the machine stream (a bare
/// `println!` here used to leak box-drawing into both, across all ~39 table
/// call sites at once).
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut table = build_table(headers, rows);
    crate::output::info(&format!("{}", table.with(Style::rounded())));
}

/// Print a table with an explicit header row and a summary ("Total") row.
/// Horizontal separators are drawn after the header and before the total row.
pub fn print_table_with_total(headers: &[&str], rows: &[Vec<String>], total: &[String]) {
    let mut all_rows: Vec<Vec<String>> = rows.to_vec();
    all_rows.push(total.to_vec());
    let mut table = build_table(headers, &all_rows);
    let n_rows = table.count_rows();
    let header_line = HorizontalLine::inherit(Style::modern());
    let total_line = HorizontalLine::inherit(Style::modern());
    let style = Style::rounded().horizontals([
        (1, header_line),
        (n_rows - 1, total_line),
    ]);
    crate::output::info(&format!("{}", table.with(style)));
}

/// Render a boolean as "Yes" or "No" — the canonical formatting used in tables.
/// Change this in one place to rename/translate the representation everywhere.
pub const fn yes_no(b: bool) -> &'static str {
    if b { "Yes" } else { "No" }
}

/// Canonical rendering for "no value set" in tables and config dumps.
/// Use everywhere we'd otherwise emit ad-hoc strings like "(none)" or "(global)".
pub const NONE_LABEL: &str = "None";

/// Render an optional JSON value for display. `None` becomes `NONE_LABEL`;
/// `Some(v)` is JSON-serialized (strings stay quoted, arrays/objects stay compact).
pub fn opt_json(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
        None    => NONE_LABEL.to_string(),
    }
}
