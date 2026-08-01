use std::collections::HashMap;
use std::fmt;

use toml_edit::{Document, Item, Table};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldProvenance {
    UserToml { line: usize },
    LocalToml { line: usize },
    ProcessorDefault,
    ScanDefault,
    OutputDirDefault,
    SerdeDefault,
    CliOverride,
}

impl fmt::Display for FieldProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserToml { line } => write!(f, "from rsconstruct.toml:{line}"),
            Self::LocalToml { line } => write!(f, "from rsconstruct.local.toml:{line}"),
            Self::ProcessorDefault => write!(f, "processor default"),
            Self::ScanDefault => write!(f, "scan default"),
            Self::OutputDirDefault => write!(f, "output dir default"),
            Self::SerdeDefault => write!(f, "serde default"),
            Self::CliOverride => write!(f, "CLI override"),
        }
    }
}

pub type ProvenanceMap = HashMap<String, FieldProvenance>;

pub fn record_if_absent(
    map: &mut ProvenanceMap,
    field: &str,
    source: FieldProvenance,
) {
    if !map.contains_key(field) {
        map.insert(field.to_string(), source);
    }
}

/// Which config section a user-set field lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Processor,
    Analyzer,
}

/// A map from (section, `instance_name`, field) to the line number in the user's
/// rsconstruct.toml where the field was set. Populated by walking a
/// [`DocumentMut`] once at config load time.
///
/// `instance_name` is what ProcessorInstance/AnalyzerInstance use: "ruff" for
/// single-instance, "pylint.core" for multi-instance.
pub type SpanMap = HashMap<(Section, String, String), usize>;

/// Per-section field line numbers for top-level sections like `[build]`,
/// `[cache]`. Outer key = section name; inner key = field name; value = 1-based
/// line number.
pub type GlobalSpanMap = HashMap<String, HashMap<String, usize>>;

/// Parse the user TOML source and build a map of field-level line numbers for
/// every key under `[processor.*]` and `[analyzer.*]`. Returns an empty map if
/// parsing fails (the caller already ran the `toml` crate's parser and
/// validated the config, so we treat span capture as best-effort enrichment).
pub fn build_span_map(source: &str) -> SpanMap {
    let mut map = SpanMap::new();
    // Document preserves byte spans on every Key and Item. DocumentMut
    // strips them on construction (`despan`), so we can't use it here.
    let doc: Document<&str> = match Document::parse(source) {
        Ok(d) => d,
        Err(_) => return map,
    };
    let root = doc.as_table();
    if let Some(item) = root.get("processor")
        && let Some(table) = item.as_table()
    {
        walk_instance_section(table, Section::Processor, source, &mut map);
    }
    if let Some(item) = root.get("analyzer")
        && let Some(table) = item.as_table()
    {
        walk_instance_section(table, Section::Analyzer, source, &mut map);
    }
    map
}

/// Walk `[processor]` or `[analyzer]` — each child is either a single instance
/// (direct config fields) or a multi-instance container (each grand-child is an
/// instance).
fn walk_instance_section(
    table: &Table,
    section: Section,
    source: &str,
    map: &mut SpanMap,
) {
    for (type_name, item) in table {
        let Some(sub) = item.as_table() else { continue };
        // Heuristic matches ProcessorConfig::is_multi_instance's shape: if
        // every child is itself a table, this section holds named
        // sub-instances; otherwise it's a single instance with direct fields.
        let all_children_are_tables = !sub.is_empty()
            && sub.iter().all(|(_, v)| v.is_table());

        if all_children_are_tables {
            // Multi-instance: [processor.pylint.core], [processor.pylint.tests]
            for (inst_suffix, inst_item) in sub {
                if let Some(inst_table) = inst_item.as_table() {
                    let instance_name = format!("{type_name}.{inst_suffix}");
                    record_field_lines(inst_table, section, &instance_name, source, map);
                }
            }
        } else {
            // Single instance
            record_field_lines(sub, section, type_name, source, map);
        }
    }
}

fn record_field_lines(
    table: &Table,
    section: Section,
    instance_name: &str,
    source: &str,
    map: &mut SpanMap,
) {
    for (key, item) in table {
        let span = key_span(table, key).or_else(|| item_span(item));
        if let Some(range) = span {
            let line = byte_offset_to_line(source, range.start);
            map.insert((section, instance_name.to_string(), key.to_string()), line);
        }
    }
}

fn key_span(table: &Table, key: &str) -> Option<std::ops::Range<usize>> {
    table.key(key).and_then(toml_edit::Key::span)
}

fn item_span(item: &Item) -> Option<std::ops::Range<usize>> {
    item.span()
}

/// Collect line numbers for every field in top-level `[section]` tables
/// other than `processor` and `analyzer` (handled separately by
/// [`build_span_map`]).
pub fn build_global_span_map(source: &str) -> GlobalSpanMap {
    let mut map = GlobalSpanMap::new();
    let doc: Document<&str> = match Document::parse(source) {
        Ok(d) => d,
        Err(_) => return map,
    };
    for (section_name, item) in doc.as_table() {
        if section_name == "processor" || section_name == "analyzer" {
            continue;
        }
        let Some(table) = item.as_table() else { continue };
        let mut fields = HashMap::new();
        for (key, field_item) in table {
            let span = key_span(table, key).or_else(|| item_span(field_item));
            if let Some(range) = span {
                fields.insert(key.to_string(), byte_offset_to_line(source, range.start));
            }
        }
        if !fields.is_empty() {
            map.insert(section_name.to_string(), fields);
        }
    }
    map
}

/// Convert a byte offset into a 1-based line number, counting `\n` bytes.
fn byte_offset_to_line(source: &str, offset: usize) -> usize {
    let clamped = offset.min(source.len());
    let mut line = 1usize;
    for b in &source.as_bytes()[..clamped] {
        if *b == b'\n' {
            line += 1;
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_capture_instance_fields() {
        let src = r#"[build]
output_dir = "build"
parallel = 4

[processor.ruff]
src_dirs = ["src"]
args = ["--fix"]

[processor.pylint.core]
src_dirs = ["src/core"]
"#;
        let spans = build_span_map(src);
        let key = (Section::Processor, "ruff".to_string(), "args".to_string());
        let line = spans.get(&key).copied().unwrap_or(0);
        assert!(line > 0, "expected a real line for ruff.args, got {line} (span map: {spans:?})");
    }

    #[test]
    fn global_spans_capture_build_fields() {
        let src = r#"[build]
output_dir = "build"
parallel = 4
"#;
        let spans = build_global_span_map(src);
        let build = spans.get("build").expect("expected [build] section in span map");
        assert_eq!(build.get("parallel"), Some(&3), "expected parallel on line 3");
    }
}
