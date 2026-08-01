use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs;

use crate::registries::{all_analyzer_plugins, all_plugins};

const CONFIG_FILE: &str = "rsconstruct.toml";

/// Add a `[processor.NAME]` section to rsconstruct.toml, pre-populated with
/// must-fill fields and one-line `#` comments for every known field.
pub fn add_processor(pname: &str, dry_run: bool) -> Result<()> {
    let plugin = all_plugins().find(|p| p.name == pname)
        .ok_or_else(|| anyhow::anyhow!("Unknown processor '{pname}'"))?;

    let known: Vec<&str> = (plugin.known_fields)().to_vec();
    let must: Vec<&str> = (plugin.must_fields)().to_vec();
    let checksum_fields: Vec<&str> = (plugin.checksum_fields)().to_vec();
    let mut descs: HashMap<&str, &str> = (plugin.field_descriptions)()
        .iter().copied().collect();
    for (f, d) in crate::config::SHARED_FIELD_DESCRIPTIONS { descs.entry(f).or_insert(d); }
    for (f, d) in crate::config::SCAN_FIELD_DESCRIPTIONS  { descs.entry(f).or_insert(d); }

    let defaults: serde_json::Value = (plugin.defconfig_json)(pname)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::default()));

    let description = processor_description(pname);

    let snippet = render_section(
        "processor",
        pname,
        description.as_deref(),
        &known,
        &must,
        &checksum_fields,
        &defaults,
        &descs,
    );

    apply_snippet("processor", pname, &snippet, dry_run)
}

/// Add a `[analyzer.NAME]` section to rsconstruct.toml.
pub fn add_analyzer(name: &str, dry_run: bool) -> Result<()> {
    let plugin = all_analyzer_plugins().find(|p| p.name == name)
        .ok_or_else(|| anyhow::anyhow!("Unknown analyzer '{name}'"))?;

    let defaults: serde_json::Value = match (plugin.defconfig_toml)() {
        Some(t) => toml::from_str::<serde_json::Value>(&t)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::default())),
        None => serde_json::Value::Object(serde_json::Map::default()),
    };
    let keys: Vec<&str> = match &defaults {
        serde_json::Value::Object(map) => map.keys().map(std::string::String::as_str).collect(),
        _ => Vec::new(),
    };

    let snippet = render_section(
        "analyzer",
        name,
        Some(plugin.description),
        &keys,
        &[],
        &[],
        &defaults,
        &HashMap::new(),
    );

    apply_snippet("analyzer", name, &snippet, dry_run)
}

/// Look up a processor's description from static plugin metadata.
fn processor_description(name: &str) -> Option<String> {
    crate::registries::processor::find_plugin(name).map(|p| p.description.to_string())
}

/// Render a single `[section.name]` block as a TOML snippet string with comments.
#[allow(clippy::too_many_arguments)]
fn render_section(
    section: &str,
    name: &str,
    description: Option<&str>,
    fields: &[&str],
    must: &[&str],
    checksum_fields: &[&str],
    defaults: &serde_json::Value,
    descs: &HashMap<&str, &str>,
) -> String {
    let mut out = String::new();
    if let Some(d) = description {
        out.push_str(&format!("# {d}\n"));
    }
    out.push_str(&format!("[{section}.{name}]\n"));

    let def_obj = defaults.as_object();
    let must_set: std::collections::HashSet<&str> = must.iter().copied().collect();
    let checksum_set: std::collections::HashSet<&str> = checksum_fields.iter().copied().collect();

    // must-fill fields first, uncommented, with TODO placeholders where the default is empty.
    for field in must {
        let desc = descs.get(field).copied().unwrap_or("");
        if !desc.is_empty() {
            out.push_str(&format!("# {desc}\n"));
        }
        let value_str = default_value_for_must(field, def_obj.and_then(|m| m.get(*field)));
        out.push_str(&format!("{field} = {value_str}\n"));
    }
    if !must.is_empty() { out.push('\n'); }

    // Remaining fields, all commented out.
    for field in fields {
        if must_set.contains(field) { continue; }
        let desc = descs.get(field).copied().unwrap_or("");
        let value_str = default_value_for_optional(def_obj.and_then(|m| m.get(*field)));
        let tag = if checksum_set.contains(field) { " (affects checksum)" } else { "" };
        if !desc.is_empty() {
            out.push_str(&format!("# {desc}{tag}\n"));
        }
        out.push_str(&format!("# {field} = {value_str}\n"));
    }

    out
}

fn default_value_for_must(field: &str, val: Option<&serde_json::Value>) -> String {
    match val {
        Some(v) if !is_empty_default(v) => toml_value_string(v),
        _ => format!("\"TODO: set {field}\""),
    }
}

fn default_value_for_optional(val: Option<&serde_json::Value>) -> String {
    match val {
        Some(v) => toml_value_string(v),
        None => "\"\"".to_string(),
    }
}

const fn is_empty_default(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::String(s) => s.is_empty(),
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Null => true,
        _ => false,
    }
}

fn toml_value_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "\"\"".to_string(),
        serde_json::Value::Array(a) => {
            let items: Vec<String> = a.iter().map(toml_value_string).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(map) => {
            let items: Vec<String> = map.iter()
                .map(|(k, v)| format!("{} = {}", k, toml_value_string(v)))
                .collect();
            format!("{{ {} }}", items.join(", "))
        }
    }
}

fn apply_snippet(section: &str, name: &str, snippet: &str, dry_run: bool) -> Result<()> {
    if dry_run {
        print!("{snippet}");
        return Ok(());
    }

    let path = std::path::Path::new(CONFIG_FILE);
    if !path.exists() {
        bail!("{CONFIG_FILE} not found. Run 'rsconstruct init' first, or use --dry-run to preview.");
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {CONFIG_FILE}"))?;

    let header = format!("[{section}.{name}]");
    if content.lines().any(|l| l.trim_start() == header) {
        bail!("Section [{section}.{name}] already exists in {CONFIG_FILE}. Edit it manually or remove it first.");
    }

    let mut new_content = content;
    if !new_content.ends_with('\n') { new_content.push('\n'); }
    if !new_content.ends_with("\n\n") { new_content.push('\n'); }
    new_content.push_str(snippet);

    fs::write(path, &new_content)
        .with_context(|| format!("Failed to write {CONFIG_FILE}"))?;

    println!("Added [{section}.{name}] to {CONFIG_FILE}.");
    Ok(())
}
