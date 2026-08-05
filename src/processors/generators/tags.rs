use anyhow::{Context, Result, bail};
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tables;
use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor};

fn default_tags_output() -> String {
    "out/tags/tags.db".into()
}

fn default_tags_dir() -> String {
    "tags".into()
}

const fn default_tags_similar_files_limit() -> usize { 10 }
const fn default_tags_suggested_tags_limit() -> usize { 15 }
const fn default_tags_common_tags_limit() -> usize { 20 }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TagsConfig {
    #[serde(default = "default_tags_output")]
    pub output: String,
    /// Directory containing tag list files.
    /// Each `<name>.txt` file defines allowed tags as `<name>:<line>` pairs.
    #[serde(default = "default_tags_dir")]
    pub tags_dir: String,
    /// Frontmatter fields that every markdown file must have.
    #[serde(default)]
    pub required_fields: Vec<String>,
    /// Scalar fields whose values must exist in the corresponding `tag_lists` file.
    #[serde(default)]
    pub required_values: Vec<String>,
    /// Fields whose values must be unique across all files.
    #[serde(default)]
    pub unique_fields: Vec<String>,
    /// Expected types for fields: "scalar", "list", or "number".
    #[serde(default)]
    pub field_types: HashMap<String, String>,
    /// Groups of fields where at least one group must be fully present.
    /// Each inner Vec is a group; a file passes if all fields in any one group are present.
    #[serde(default)]
    pub required_field_groups: Vec<Vec<String>>,
    /// Require list-type fields to have items in sorted order.
    #[serde(default)]
    pub sorted_tags: bool,
    /// Fail the build when tags in the allowlist are not used by any file.
    #[serde(default)]
    pub check_unused: bool,
    /// How many most-similar files to inspect when generating tag suggestions.
    #[serde(default = "default_tags_similar_files_limit")]
    pub similar_files_limit: usize,
    /// Maximum tag suggestions emitted by `tags suggest`.
    #[serde(default = "default_tags_suggested_tags_limit")]
    pub suggested_tags_limit: usize,
    /// Maximum "most common tags" reported when a file has no tags of its own.
    #[serde(default = "default_tags_common_tags_limit")]
    pub common_tags_limit: usize,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for TagsConfig {
    fn default() -> Self {
        Self {
            output: "out/tags/tags.db".into(),
            tags_dir: "tags".into(),
            required_fields: Vec::new(),
            required_values: Vec::new(),
            unique_fields: Vec::new(),
            field_types: HashMap::new(),
            required_field_groups: Vec::new(),
            sorted_tags: false,
            check_unused: false,
            similar_files_limit: default_tags_similar_files_limit(),
            suggested_tags_limit: default_tags_suggested_tags_limit(),
            common_tags_limit: default_tags_common_tags_limit(),
            standard: StandardConfig::default(),
        }
    }
}

const FRONTMATTER: TableDefinition<&str, &str> = TableDefinition::new("frontmatter");
const TAG_INDEX: TableDefinition<&str, &str> = TableDefinition::new("tag_index");

pub struct TagsProcessor {
    config: TagsConfig,
}

impl TagsProcessor {
    pub const fn new(config: TagsConfig) -> Self {
        Self {
            config,
        }
    }
}

impl Processor for TagsProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        if file_index.scan(&self.config.standard, true).is_empty() {
            return false;
        }
        // Require a tags_dir with at least one .txt file
        let dir = Path::new(&self.config.tags_dir);
        dir.is_dir() && fs::read_dir(dir).is_ok_and(|entries| {
            entries.filter_map(std::result::Result::ok)
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("txt"))
        })
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let files = file_index.scan(&self.config.standard, true);
        if files.is_empty() {
            return Ok(());
        }

        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;
        let mut inputs = Vec::with_capacity(files.len() + extra.len() + 1);
        inputs.extend(files);
        inputs.extend_from_slice(&extra);

        // Add tag list files as inputs so edits trigger rebuild
        let dir = Path::new(&self.config.tags_dir);
        if dir.is_dir() {
            for entry in fs::read_dir(dir)
                .with_context(|| format!("Failed to read tags_dir: {}", self.config.tags_dir))?
            {
                let entry = entry
                    .with_context(|| format!("Failed to read entry in tags_dir: {}", self.config.tags_dir))?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("txt") {
                    inputs.push(path);
                }
            }
        }

        let output = PathBuf::from(&self.config.output);
        graph.add_product(
            inputs,
            vec![output],
            instance_name,
            Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name))),
        )?;

        Ok(())
    }

    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let output_path = product.primary_output();

        // Ensure output directory exists
        crate::processors::ensure_output_dir(output_path)?;

        // Collect frontmatter from all input .md files
        let mut all_frontmatter: HashMap<String, serde_json::Value> = HashMap::new();
        let mut tag_to_files: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut duplicate_tags: Vec<(String, String)> = Vec::new(); // (file, tag)
        let mut unsorted_tags: Vec<(String, String, String, String)> = Vec::new(); // (file, field, a, b)

        for input in &product.inputs {
            let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "md" {
                continue;
            }
            let content = fs::read_to_string(input)
                .with_context(|| format!("Failed to read {}", input.display()))?;

            if let Some(fm) = parse_frontmatter(&content) {
                let file_key = input.display().to_string();
                let mut file_tags: HashSet<String> = HashSet::new();

                // Index all frontmatter fields:
                // - list fields: each item becomes a tag (e.g. "docker" from tags: [docker])
                // - scalar fields: indexed as "key:value" (e.g. "level:intermediate")
                if let Some(obj) = fm.as_object() {
                    for (key, value) in obj {
                        match value {
                            serde_json::Value::Array(items) => {
                                // Check sorted order if enabled
                                if self.config.sorted_tags {
                                    let strs: Vec<&str> = items.iter()
                                        .filter_map(|i| i.as_str())
                                        .collect();
                                    for pair in strs.windows(2) {
                                        if pair[0] > pair[1] {
                                            unsorted_tags.push((
                                                file_key.clone(), key.clone(),
                                                pair[0].to_string(), pair[1].to_string(),
                                            ));
                                            break;
                                        }
                                    }
                                }
                                for item in items {
                                    if let Some(s) = item.as_str() {
                                        if !file_tags.insert(s.to_string()) {
                                            duplicate_tags.push((file_key.clone(), s.to_string()));
                                        }
                                        tag_to_files.entry(s.to_string())
                                            .or_default()
                                            .insert(file_key.clone());
                                    }
                                }
                            }
                            serde_json::Value::String(s) => {
                                let tag = format!("{key}:{s}");
                                if !file_tags.insert(tag.clone()) {
                                    duplicate_tags.push((file_key.clone(), tag.clone()));
                                }
                                tag_to_files.entry(tag)
                                    .or_default()
                                    .insert(file_key.clone());
                            }
                            // parse_simple_yaml produces only String values for scalars,
                            // so Number/Bool/Null/Object cannot occur here.
                            _ => {}
                        }
                    }
                }

                all_frontmatter.insert(file_key, fm);
            }
        }

        // Check required frontmatter fields
        if !self.config.required_fields.is_empty() {
            let mut missing: Vec<(String, Vec<String>)> = Vec::new(); // (file, missing_fields)
            for input in &product.inputs {
                let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext != "md" {
                    continue;
                }
                let file_key = input.display().to_string();
                let fm = all_frontmatter.get(&file_key);
                let obj = fm.and_then(|v| v.as_object());
                let mut file_missing: Vec<String> = Vec::new();
                for field in &self.config.required_fields {
                    let has_field = obj.is_some_and(|o| {
                        o.get(field).is_some_and(|v| match v {
                            serde_json::Value::Array(a) => !a.is_empty(),
                            serde_json::Value::String(s) => !s.is_empty(),
                            serde_json::Value::Null => false,
                            _ => true,
                        })
                    });
                    if !has_field {
                        file_missing.push(field.clone());
                    }
                }
                if !file_missing.is_empty() {
                    missing.push((file_key, file_missing));
                }
            }
            if !missing.is_empty() {
                missing.sort_by(|a, b| a.0.cmp(&b.0));
                let mut msg = String::from("Missing required frontmatter fields:\n");
                for (file, fields) in &missing {
                    let _ = writeln!(msg, "  {}: {}", file, fields.join(", "));
                }
                bail!("{}", msg.trim_end());
            }
        }

        // Check required field groups
        if !self.config.required_field_groups.is_empty() {
            let mut failing: Vec<(String, Vec<String>)> = Vec::new();
            for input in &product.inputs {
                let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext != "md" {
                    continue;
                }
                let file_key = input.display().to_string();
                let fm = all_frontmatter.get(&file_key);
                let obj = fm.and_then(|v| v.as_object());
                let satisfies_any = self.config.required_field_groups.iter().any(|group| {
                    group.iter().all(|field| {
                        obj.is_some_and(|o| {
                            o.get(field).is_some_and(|v| match v {
                                serde_json::Value::Array(a) => !a.is_empty(),
                                serde_json::Value::String(s) => !s.is_empty(),
                                serde_json::Value::Null => false,
                                _ => true,
                            })
                        })
                    })
                });
                if !satisfies_any {
                    let group_strs: Vec<String> = self.config.required_field_groups.iter()
                        .map(|g| format!("[{}]", g.join(", ")))
                        .collect();
                    failing.push((file_key, group_strs));
                }
            }
            if !failing.is_empty() {
                failing.sort_by(|a, b| a.0.cmp(&b.0));
                let mut msg = String::from("Files missing required field groups (must satisfy at least one):\n");
                for (file, groups) in &failing {
                    let _ = writeln!(msg, "  {}: none of {}", file, groups.join(" or "));
                }
                bail!("{}", msg.trim_end());
            }
        }

        // Check for duplicate tags within files
        if !duplicate_tags.is_empty() {
            duplicate_tags.sort();
            let mut msg = String::from("Duplicate tags found within files:\n");
            for (file, tag) in &duplicate_tags {
                let _ = writeln!(msg, "  {tag} in {file}");
            }
            bail!("{}", msg.trim_end());
        }

        // Validate tags against allowed set from tags_dir
        let dir = Path::new(&self.config.tags_dir);
        if dir.is_dir() {
            let allowed = load_tags_dir(dir)?;
            let mut unknown: Vec<(String, Vec<String>)> = Vec::new();
            for (tag, files) in &tag_to_files {
                if !tag_matches_allowed(tag, &allowed) {
                    unknown.push((tag.clone(), files.iter().cloned().collect()));
                }
            }
            if !unknown.is_empty() {
                unknown.sort_by(|a, b| a.0.cmp(&b.0));
                let mut msg = format!("Unknown tags found (not in {}):\n", self.config.tags_dir);
                for (tag, files) in &unknown {
                    let _ = write!(msg, "  {tag}");
                    if let Some(suggestion) = find_similar_tag(tag, &allowed) {
                        let _ = write!(msg, " (did you mean '{suggestion}'?)");
                    }
                    msg.push('\n');
                    for file in files {
                        let _ = writeln!(msg, "    - {file}");
                    }
                }
                bail!("{}", msg.trim_end());
            }

            // Check for unused tags (in allowlist but not used by any file)
            if self.config.check_unused {
                let used_tags: HashSet<&String> = tag_to_files.keys().collect();
                let mut unused: Vec<&String> = allowed.iter()
                    .filter(|t| !used_tags.contains(t))
                    .collect();
                if !unused.is_empty() {
                    unused.sort();
                    let mut msg = format!("Unused tags in {} (not used by any file):\n", self.config.tags_dir);
                    for tag in &unused {
                        let _ = writeln!(msg, "  {tag}");
                    }
                    bail!("{}", msg.trim_end());
                }
            }
        }

        // Check sorted tags
        if self.config.sorted_tags && !unsorted_tags.is_empty() {
            unsorted_tags.sort();
            let mut msg = String::from("List tags are not in sorted order:\n");
            for (file, field, a, b) in &unsorted_tags {
                let _ = writeln!(msg, "  {file} field '{field}': '{b}' should come after '{a}'");
            }
            bail!("{}", msg.trim_end());
        }

        // Check required_values (scalar fields must have values in tags dir)
        if !self.config.required_values.is_empty() {
            let dir = Path::new(&self.config.tags_dir);
            let allowed = if dir.is_dir() { load_tags_dir(dir)? } else { HashSet::new() };
            let mut invalid: Vec<(String, String, String)> = Vec::new(); // (file, field, value)
            for (file_key, fm) in &all_frontmatter {
                if let Some(obj) = fm.as_object() {
                    for field in &self.config.required_values {
                        if let Some(serde_json::Value::String(val)) = obj.get(field) {
                            let tag = format!("{field}:{val}");
                            if !allowed.contains(&tag) {
                                invalid.push((file_key.clone(), field.clone(), val.clone()));
                            }
                        }
                    }
                }
            }
            if !invalid.is_empty() {
                invalid.sort();
                let mut msg = String::from("Invalid values for validated fields:\n");
                for (file, field, val) in &invalid {
                    let _ = writeln!(msg, "  {}: {}={} (not in {}/{}.txt)", file, field, val, self.config.tags_dir, field);
                }
                bail!("{}", msg.trim_end());
            }
        }

        // Check unique_fields
        if !self.config.unique_fields.is_empty() {
            let mut field_values: HashMap<(&str, String), Vec<String>> = HashMap::new();
            for (file_key, fm) in &all_frontmatter {
                if let Some(obj) = fm.as_object() {
                    for field in &self.config.unique_fields {
                        if let Some(val) = obj.get(field) {
                            let val_str = match val {
                                serde_json::Value::String(s) => s.clone(),
                                serde_json::Value::Array(items) => {
                                    let strs: Vec<&str> = items.iter()
                                        .filter_map(|i| i.as_str())
                                        .collect();
                                    strs.join(",")
                                }
                                _ => continue,
                            };
                            field_values.entry((field.as_str(), val_str))
                                .or_default()
                                .push(file_key.clone());
                        }
                    }
                }
            }
            let mut dupes: Vec<(String, String, Vec<String>)> = Vec::new();
            for ((field, val), files) in &field_values {
                if files.len() > 1 {
                    let mut sorted_files = files.clone();
                    sorted_files.sort();
                    dupes.push((field.to_string(), val.clone(), sorted_files));
                }
            }
            if !dupes.is_empty() {
                dupes.sort();
                let mut msg = String::from("Duplicate values for unique fields:\n");
                for (field, val, files) in &dupes {
                    let _ = writeln!(msg, "  {field}='{val}' in:");
                    for file in files {
                        let _ = writeln!(msg, "    - {file}");
                    }
                }
                bail!("{}", msg.trim_end());
            }
        }

        // Check field_types
        if !self.config.field_types.is_empty() {
            let mut type_errors: Vec<(String, String, String, String)> = Vec::new(); // (file, field, expected, actual)
            for (file_key, fm) in &all_frontmatter {
                if let Some(obj) = fm.as_object() {
                    for (field, expected_type) in &self.config.field_types {
                        if let Some(val) = obj.get(field) {
                            let actual_ok = match expected_type.as_str() {
                                "list" => matches!(val, serde_json::Value::Array(_)),
                                "scalar" => matches!(val, serde_json::Value::String(_)),
                                "number" => {
                                    if let serde_json::Value::String(s) = val {
                                        s.parse::<f64>().is_ok()
                                    } else {
                                        false
                                    }
                                }
                                _ => true,
                            };
                            if !actual_ok {
                                let actual = match val {
                                    serde_json::Value::Array(_) => "list",
                                    serde_json::Value::String(s) => {
                                        if s.parse::<f64>().is_ok() { "number" } else { "scalar" }
                                    }
                                    _ => "unknown",
                                };
                                type_errors.push((
                                    file_key.clone(), field.clone(),
                                    expected_type.clone(), actual.to_string(),
                                ));
                            }
                        }
                    }
                }
            }
            if !type_errors.is_empty() {
                type_errors.sort();
                let mut msg = String::from("Field type mismatches:\n");
                for (file, field, expected, actual) in &type_errors {
                    let _ = writeln!(msg, "  {file}: '{field}' expected {expected}, got {actual}");
                }
                bail!("{}", msg.trim_end());
            }
        }

        // Delete old database to avoid stale entries from previous builds
        if output_path.exists() {
            fs::remove_file(output_path)
                .with_context(|| format!("Failed to remove old tags database: {}", output_path.display()))?;
        }
        let db = crate::db::open_or_recreate(output_path, "tags database")?;

        let write_txn = db.begin_write()
            .context("Failed to begin write transaction")?;
        {
            let mut fm_table = write_txn.open_table(FRONTMATTER)
                .context("Failed to open frontmatter table")?;
            for (file, value) in &all_frontmatter {
                let json = serde_json::to_string(value).expect(crate::errors::JSON_SERIALIZE);
                fm_table.insert(file.as_str(), json.as_str())
                    .context("Failed to insert frontmatter")?;
            }
        }
        {
            let mut tag_table = write_txn.open_table(TAG_INDEX)
                .context("Failed to open tag_index table")?;
            for (tag, files) in &tag_to_files {
                let files_vec: Vec<&String> = files.iter().collect();
                let json = serde_json::to_string(&files_vec).expect(crate::errors::JSON_SERIALIZE);
                tag_table.insert(tag.as_str(), json.as_str())
                    .context("Failed to insert tag index")?;
            }
        }
        crate::errors::ctx(write_txn.commit(), "Failed to commit tags database")?;

        Ok(())
    }

}

/// Parse YAML frontmatter from a markdown file.
/// Looks for content between `---` markers at the start of the file.
fn parse_frontmatter(content: &str) -> Option<serde_json::Value> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let rest = after_first.trim_start_matches(['\r', '\n']);
    let end_pos = rest.find("\n---")?;
    let yaml_block = &rest[..end_pos];

    Some(parse_simple_yaml(yaml_block))
}

/// Strip surrounding quotes (single or double) from a YAML value.
fn strip_yaml_quotes(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"'))
            || (s.starts_with('\'') && s.ends_with('\'')))
    {
        return &s[1..s.len() - 1];
    }
    s
}

/// Parse simple YAML key-value pairs and lists.
/// Supports:
///   key: value           (scalar, including values with colons like URLs)
///   key: "quoted value"
///   key: [a, b, c]       (inline list)
///   key:
///     - item1
///     - item2
///
/// Limitations: no multi-line strings (| or >), no nested objects, no anchors.
fn parse_simple_yaml(block: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut current_key: Option<String> = None;
    let mut current_list: Vec<serde_json::Value> = Vec::new();
    let mut in_list = false;

    for line in block.lines() {
        let stripped = line.trim_end();

        // Check for list item: optional leading whitespace followed by "- "
        let trimmed = stripped.trim_start();
        if let Some(item) = trimmed.strip_prefix("- ")
            && in_list
        {
            let val = strip_yaml_quotes(item.trim());
            current_list.push(serde_json::Value::String(val.to_string()));
            continue;
        }

        // If we were building a list, save it
        if in_list {
            if let Some(key) = current_key.take() {
                map.insert(key, serde_json::Value::Array(std::mem::take(&mut current_list)));
            }
            in_list = false;
        }

        // Parse key: value — split on first ": " (colon+space) to preserve colons in values
        if let Some((key, value)) = stripped.split_once(": ") {
            let key = key.trim().to_string();
            let value = value.trim();

            if value.is_empty() {
                // Start of a multi-line list
                current_key = Some(key);
                in_list = true;
                current_list.clear();
            } else if value.starts_with('[') && value.ends_with(']') {
                // Inline list: [item1, item2, item3]
                let inner = &value[1..value.len() - 1];
                if inner.trim().is_empty() {
                    map.insert(key, serde_json::Value::Array(Vec::new()));
                } else {
                    let items: Vec<serde_json::Value> = inner.split(',')
                        .map(|s| {
                            let val = strip_yaml_quotes(s.trim());
                            serde_json::Value::String(val.to_string())
                        })
                        .collect();
                    map.insert(key, serde_json::Value::Array(items));
                }
            } else {
                let val = strip_yaml_quotes(value);
                map.insert(key, serde_json::Value::String(val.to_string()));
            }
        } else if let Some((key, _)) = stripped.split_once(':') {
            // Key with no value after colon (no space) — start of a multi-line list
            let key = key.trim().to_string();
            current_key = Some(key);
            in_list = true;
            current_list.clear();
        }
    }

    // Flush any trailing list
    if in_list
        && let Some(key) = current_key.take()
    {
        map.insert(key, serde_json::Value::Array(current_list));
    }

    serde_json::Value::Object(map)
}

/// Open the tags database for reading. Used by the `rsconstruct tags` CLI subcommand.
pub fn open_tags_db(db_path: &str) -> Result<redb::Database> {
    let path = std::path::Path::new(db_path);
    if !path.exists() {
        anyhow::bail!("Tags database not found: {db_path}. Run 'rsconstruct build' first.");
    }
    redb::Database::open(path)
        .with_context(|| format!("Failed to open tags database: {db_path}"))
}

/// List all unique tags from the database.
pub fn list_tags(db_path: &str) -> Result<()> {
    let tags = load_all_tags_sorted(db_path)?;
    if crate::json_output::is_json_mode() {
        println!("{}", serde_json::to_string(&tags).expect(crate::errors::JSON_SERIALIZE));
    } else {
        for tag in &tags {
            println!("{tag}");
        }
    }
    Ok(())
}

/// Search for tags containing a substring.
pub fn grep_tags(db_path: &str, text: &str, ignore_case: bool) -> Result<()> {
    let all_tags = load_all_tags_sorted(db_path)?;
    let needle = if ignore_case { text.to_lowercase() } else { text.to_string() };
    let matches: Vec<&String> = all_tags.iter()
        .filter(|t| {
            if ignore_case {
                t.to_lowercase().contains(&needle)
            } else {
                t.contains(text)
            }
        })
        .collect();
    if crate::json_output::is_json_mode() {
        println!("{}", serde_json::to_string(&matches).expect(crate::errors::JSON_SERIALIZE));
    } else {
        for tag in &matches {
            println!("{tag}");
        }
    }
    Ok(())
}

/// List files matching given tags. AND by default, OR if `use_or` is true.
pub fn files_for_tags(db_path: &str, tags: &[String], use_or: bool) -> Result<()> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;
    let table = crate::errors::ctx(read_txn.open_table(TAG_INDEX), "Failed to open tag_index table")?;

    let mut result: Option<BTreeSet<String>> = None;

    for tag in tags {
        let files: BTreeSet<String> = match table.get(tag.as_str()).context("Failed to query tag")? {
            Some(value) => {
                let v: Vec<String> = serde_json::from_str(value.value())
                    .context("Failed to parse tag file list")?;
                v.into_iter().collect()
            }
            None => BTreeSet::new(),
        };
        result = Some(match result {
            Some(acc) => {
                if use_or {
                    acc.union(&files).cloned().collect()
                } else {
                    acc.intersection(&files).cloned().collect()
                }
            }
            None => files,
        });
    }

    let files: Vec<String> = result.unwrap_or_default().into_iter().collect();
    if crate::json_output::is_json_mode() {
        println!("{}", serde_json::to_string(&files).expect(crate::errors::JSON_SERIALIZE));
    } else if files.is_empty() {
        let mode = if use_or { "any" } else { "all" };
        println!("No files found matching {} tags: {}", mode, tags.join(", "));
    } else {
        for file in &files {
            println!("{file}");
        }
    }

    Ok(())
}

/// Show each tag with its file count, sorted by frequency (descending).
pub fn count_tags(db_path: &str) -> Result<()> {
    let tag_counts = load_tag_counts(db_path)?;
    let mut entries: Vec<(String, usize)> = tag_counts.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    if crate::json_output::is_json_mode() {
        let json_entries: Vec<serde_json::Value> = entries.iter()
            .map(|(tag, count)| serde_json::json!({"tag": tag, "count": count}))
            .collect();
        println!("{}", serde_json::to_string(&json_entries).expect(crate::errors::JSON_SERIALIZE));
    } else {
        let rows: Vec<Vec<String>> = entries.iter()
            .map(|(tag, count)| vec![count.to_string(), tag.clone()])
            .collect();
        tables::print_table(&["Count", "Tag"], &rows);
    }

    Ok(())
}

/// Show tags grouped by prefix/category.
pub fn tree_tags(db_path: &str) -> Result<()> {
    let tag_counts = load_tag_counts(db_path)?;

    // Split into key:value groups and bare tags
    let mut groups: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    let mut bare: Vec<(String, usize)> = Vec::new();

    for (tag, count) in &tag_counts {
        if let Some((key, value)) = tag.split_once(':') {
            groups.entry(key.to_string())
                .or_default()
                .push((value.to_string(), *count));
        } else {
            bare.push((tag.clone(), *count));
        }
    }

    if crate::json_output::is_json_mode() {
        let mut json_groups: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for (key, mut values) in groups {
            values.sort_by(|a, b| a.0.cmp(&b.0));
            let entries: Vec<serde_json::Value> = values.iter()
                .map(|(v, c)| serde_json::json!({"value": v, "count": c}))
                .collect();
            json_groups.insert(key, serde_json::Value::Array(entries));
        }
        bare.sort_by(|a, b| a.0.cmp(&b.0));
        let bare_entries: Vec<serde_json::Value> = bare.iter()
            .map(|(t, c)| serde_json::json!({"tag": t, "count": c}))
            .collect();
        json_groups.insert("_bare".to_string(), serde_json::Value::Array(bare_entries));
        println!("{}", serde_json::to_string(&json_groups).expect(crate::errors::JSON_SERIALIZE));
    } else {
        // Print key=value groups
        for (key, mut values) in groups {
            values.sort_by(|a, b| a.0.cmp(&b.0));
            println!("{key}=");
            for (value, count) in &values {
                println!("  {count:>4}  {value}");
            }
        }

        // Print bare tags
        if !bare.is_empty() {
            bare.sort_by(|a, b| a.0.cmp(&b.0));
            println!("(bare tags)");
            for (tag, count) in &bare {
                println!("  {count:>4}  {tag}");
            }
        }
    }

    Ok(())
}

/// Show statistics about the tags database.
pub fn stats_tags(db_path: &str) -> Result<()> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;

    // Count indexed files from the frontmatter table
    let fm_table = crate::errors::ctx(read_txn.open_table(FRONTMATTER), "Failed to open frontmatter table")?;
    let file_count = crate::errors::ctx(fm_table.len(), "Failed to count frontmatter entries")?;

    // Count and classify tags, and sum total associations
    let tag_table = crate::errors::ctx(read_txn.open_table(TAG_INDEX), "Failed to open tag_index table")?;
    let mut bare_count: u64 = 0;
    let mut kv_count: u64 = 0;
    let mut total_associations: u64 = 0;
    let iter = crate::errors::ctx(tag_table.iter(), "Failed to iterate tag_index")?;
    for entry in iter {
        let (key, value) = crate::errors::ctx(entry, "Failed to read tag entry")?;
        if key.value().contains(':') {
            kv_count += 1;
        } else {
            bare_count += 1;
        }
        let files: Vec<String> = serde_json::from_str(value.value())
            .context("Failed to parse tag file list")?;
        total_associations += files.len() as u64;
    }

    let unique_tags = bare_count + kv_count;

    if crate::json_output::is_json_mode() {
        let stats = serde_json::json!({
            "files_indexed": file_count,
            "tag_assignments": total_associations,
            "unique_tags": unique_tags,
            "bare_tags": bare_count,
            "kv_tags": kv_count,
        });
        println!("{}", serde_json::to_string(&stats).expect(crate::errors::JSON_SERIALIZE));
    } else {
        println!("Files indexed:    {file_count}");
        println!("Tag assignments:  {total_associations}");
        println!("Unique tags:      {unique_tags}");
        println!("  bare tags:      {bare_count}");
        println!("  key=value:      {kv_count}");
    }

    Ok(())
}

/// List all tags for a specific file.
pub fn tags_for_file(db_path: &str, path: &str) -> Result<()> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;
    let table = crate::errors::ctx(read_txn.open_table(TAG_INDEX), "Failed to open tag_index table")?;

    let mut file_tags: Vec<String> = Vec::new();
    let iter = crate::errors::ctx(table.iter(), "Failed to iterate tag_index")?;
    for entry in iter {
        let (key, value) = crate::errors::ctx(entry, "Failed to read tag entry")?;
        let files: Vec<String> = serde_json::from_str(value.value())
            .context("Failed to parse tag file list")?;
        if files.iter().any(|f| path_matches(f, path)) {
            file_tags.push(key.value().to_string());
        }
    }
    file_tags.sort();

    if crate::json_output::is_json_mode() {
        println!("{}", serde_json::to_string(&file_tags).expect(crate::errors::JSON_SERIALIZE));
    } else if file_tags.is_empty() {
        eprintln!("No tags found for: {path}");
    } else {
        for tag in &file_tags {
            println!("{tag}");
        }
    }

    Ok(())
}

/// Show the raw frontmatter for a specific file.
pub fn frontmatter_for_file(db_path: &str, path: &str) -> Result<()> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;
    let table = crate::errors::ctx(read_txn.open_table(FRONTMATTER), "Failed to open frontmatter table")?;

    // Try exact match first, then suffix match
    let (found_key, found_value): (Option<String>, Option<String>) =
        if let Some(value) = table.get(path).context("Failed to query frontmatter")? {
            (Some(path.to_string()), Some(value.value().to_string()))
        } else {
            // Suffix match with path boundary — collect all matches to detect ambiguity
            let mut all_matches: Vec<(String, String)> = Vec::new();
            let iter = crate::errors::ctx(table.iter(), "Failed to iterate frontmatter")?;
            for entry in iter {
                let (key, value) = crate::errors::ctx(entry, "Failed to read frontmatter entry")?;
                if path_matches(key.value(), path) {
                    all_matches.push((key.value().to_string(), value.value().to_string()));
                }
            }
            if all_matches.len() > 1 {
                eprintln!("Warning: '{}' matches {} files, showing first:", path, all_matches.len());
                for (k, _) in &all_matches {
                    eprintln!("  {k}");
                }
            }
            if let Some((k, v)) = all_matches.into_iter().next() {
                (Some(k), Some(v))
            } else {
                (None, None)
            }
        };

    match (found_key, found_value) {
        (Some(key), Some(json)) => {
            if crate::json_output::is_json_mode() {
                let fm_value: serde_json::Value = serde_json::from_str(&json)
                    .context("Failed to parse stored frontmatter")?;
                let output = serde_json::json!({"file": key, "frontmatter": fm_value});
                println!("{}", serde_json::to_string(&output).expect(crate::errors::JSON_SERIALIZE));
            } else {
                println!("{key}:");
                let value: serde_json::Value = serde_json::from_str(&json)
                    .context("Failed to parse stored frontmatter")?;
                println!("{}", serde_json::to_string_pretty(&value).expect(crate::errors::JSON_SERIALIZE));
            }
        }
        _ => {
            if crate::json_output::is_json_mode() {
                println!("{}", serde_json::json!({"file": null, "frontmatter": null}));
            } else {
                println!("No frontmatter found for: {path}");
            }
        }
    }

    Ok(())
}

/// List tags in the allowlist that are not used by any file.
/// If `strict` is true, returns an error when unused tags are found (for CI).
pub fn unused_tags(db_path: &str, tags_dir: &str, strict: bool) -> Result<()> {
    let dir = Path::new(tags_dir);
    if !dir.is_dir() {
        bail!("tags_dir not found: {tags_dir}");
    }
    let allowed = load_tags_dir(dir)?;
    let db_tags = load_all_tags(db_path)?;

    let mut unused: Vec<&String> = allowed.iter()
        .filter(|t| !db_tags.contains(*t))
        .collect();
    unused.sort();

    if strict && !unused.is_empty() {
        let mut msg = format!("{} unused tag(s) in {}:\n", unused.len(), tags_dir);
        for tag in &unused {
            let _ = writeln!(msg, "  {tag}");
        }
        bail!("{}", msg.trim_end());
    }

    if crate::json_output::is_json_mode() {
        println!("{}", serde_json::to_string(&unused).expect(crate::errors::JSON_SERIALIZE));
    } else if unused.is_empty() {
        println!("All tags in {tags_dir} are in use.");
    } else {
        println!("{} unused tag(s) in {}:", unused.len(), tags_dir);
        for tag in &unused {
            println!("  {tag}");
        }
    }

    Ok(())
}

/// Scan the tags database and add any missing tags back to the tag collection files.
/// For key:value tags (e.g. "level:advanced"), adds the value to `{tags_dir}/{key}.txt`.
/// For bare tags (e.g. "docker"), adds to `{tags_dir}/tags.txt`.
pub fn collect_tags(db_path: &str, tags_dir: &str) -> Result<()> {
    let dir = Path::new(tags_dir);
    if !dir.is_dir() {
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create tags_dir: {tags_dir}"))?;
    }
    let allowed = load_tags_dir(dir)?;
    let db_tags = load_all_tags(db_path)?;

    // Find tags in the database that are not in the allowlist
    let mut missing: Vec<&String> = db_tags.iter()
        .filter(|t| !tag_matches_allowed(t, &allowed))
        .collect();
    missing.sort();

    if missing.is_empty() {
        println!("All tags are already in the collection.");
        return Ok(());
    }

    // Group by category: key:value → file={key}.txt, bare → file=tags.txt
    let mut by_file: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for tag in &missing {
        if let Some((key, value)) = tag.split_once(':') {
            by_file.entry(format!("{key}.txt"))
                .or_default()
                .insert(value.to_string());
        } else {
            by_file.entry("tags.txt".to_string())
                .or_default()
                .insert((*tag).clone());
        }
    }

    // Append to each file and keep sorted
    for (filename, new_values) in &by_file {
        let path = dir.join(filename);
        let mut existing: BTreeSet<String> = if path.exists() {
            fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect()
        } else {
            BTreeSet::new()
        };

        let before = existing.len();
        existing.extend(new_values.iter().cloned());
        let added = existing.len() - before;

        if added > 0 {
            let mut sorted: Vec<&String> = existing.iter().collect();
            sorted.sort();
            let content = sorted.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(&path, format!("{content}\n"))
                .with_context(|| format!("Failed to write {}", path.display()))?;
            println!("Added {} tag(s) to {}", added, path.display());
        }
    }

    println!("Collected {} missing tag(s) into {}.", missing.len(), tags_dir);
    Ok(())
}

/// Validate tags against allowed set without building.
pub fn validate_tags(db_path: &str, tags_dir: &str) -> Result<()> {
    let dir = Path::new(tags_dir);
    if !dir.is_dir() {
        bail!("tags_dir not found: {tags_dir}");
    }
    let allowed = load_tags_dir(dir)?;
    // Single db open: get both tags and counts in one pass
    let tag_counts = load_tag_counts(db_path)?;

    let mut unknown: Vec<String> = tag_counts.keys()
        .filter(|t| !tag_matches_allowed(t, &allowed))
        .cloned()
        .collect();
    unknown.sort();

    if unknown.is_empty() {
        if crate::json_output::is_json_mode() {
            println!("{}", serde_json::json!({"valid": true, "unknown_count": 0}));
        } else {
            println!("All tags are valid.");
        }
    } else {
        let mut msg = format!("{} unknown tag(s) found:\n", unknown.len());
        for tag in &unknown {
            let count = tag_counts.get(tag).copied().unwrap_or(0);
            let _ = write!(msg, "  {tag} ({count} file(s))");
            // Suggest similar tags
            if let Some(suggestion) = find_similar_tag(tag, &allowed) {
                let _ = write!(msg, " - did you mean '{suggestion}'?");
            }
            msg.push('\n');
        }
        bail!("{}", msg.trim_end());
    }

    Ok(())
}

/// Show a coverage matrix of tag categories per file.
pub fn matrix_tags(db_path: &str) -> Result<()> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;
    let fm_table = crate::errors::ctx(read_txn.open_table(FRONTMATTER), "Failed to open frontmatter table")?;
    let tag_table = crate::errors::ctx(read_txn.open_table(TAG_INDEX), "Failed to open tag_index table")?;

    // Collect all categories and per-file category presence
    let mut categories: BTreeSet<String> = BTreeSet::new();
    let mut file_categories: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    // Initialize all files from frontmatter table
    let fm_iter = crate::errors::ctx(fm_table.iter(), "Failed to iterate frontmatter")?;
    for entry in fm_iter {
        let (key, _) = crate::errors::ctx(entry, "Failed to read frontmatter entry")?;
        file_categories.entry(key.value().to_string()).or_default();
    }

    // Build category sets from tag index
    let tag_iter = crate::errors::ctx(tag_table.iter(), "Failed to iterate tag_index")?;
    for entry in tag_iter {
        let (key, value) = crate::errors::ctx(entry, "Failed to read tag entry")?;
        let tag = key.value();
        let category = tag.split(':').next().unwrap_or(tag).to_string();
        categories.insert(category.clone());
        let files: Vec<String> = serde_json::from_str(value.value())
            .context("Failed to parse tag file list")?;
        for file in files {
            file_categories.entry(file).or_default().insert(category.clone());
        }
    }

    if crate::json_output::is_json_mode() {
        println!("{}", serde_json::to_string_pretty(&file_categories).expect(crate::errors::JSON_SERIALIZE));
    } else {
        let cats: Vec<&String> = categories.iter().collect();
        let mut headers: Vec<&str> = vec!["File"];
        headers.extend(cats.iter().map(|c| c.as_str()));
        let rows: Vec<Vec<String>> = file_categories.iter().map(|(file, file_cats)| {
            let short = file.rsplit('/').next().unwrap_or(file);
            let mut row: Vec<String> = vec![short.to_string()];
            for cat in &cats {
                row.push(if file_cats.contains(*cat) { "Y".to_string() } else { "-".to_string() });
            }
            row
        }).collect();
        tables::print_table(&headers, &rows);
    }
    Ok(())
}

/// Show percentage of files that have each tag category.
pub fn coverage_tags(db_path: &str) -> Result<()> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;
    let fm_table = crate::errors::ctx(read_txn.open_table(FRONTMATTER), "Failed to open frontmatter table")?;
    let tag_table = crate::errors::ctx(read_txn.open_table(TAG_INDEX), "Failed to open tag_index table")?;

    let total_files = crate::errors::ctx(fm_table.len(), "Failed to count frontmatter entries")? as usize;
    if total_files == 0 {
        println!("No files indexed.");
        return Ok(());
    }

    // Count files per category
    let mut category_files: HashMap<String, HashSet<String>> = HashMap::new();
    let tag_iter = crate::errors::ctx(tag_table.iter(), "Failed to iterate tag_index")?;
    for entry in tag_iter {
        let (key, value) = crate::errors::ctx(entry, "Failed to read tag entry")?;
        let tag = key.value();
        let category = tag.split(':').next().unwrap_or(tag).to_string();
        let files: Vec<String> = serde_json::from_str(value.value())
            .context("Failed to parse tag file list")?;
        let cat_set = category_files.entry(category).or_default();
        for file in files {
            cat_set.insert(file);
        }
    }

    let mut coverage: Vec<(String, usize, f64)> = category_files.iter()
        .map(|(cat, files)| {
            let pct = (files.len() as f64 / total_files as f64) * 100.0;
            (cat.clone(), files.len(), pct)
        })
        .collect();
    coverage.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    if crate::json_output::is_json_mode() {
        let json: Vec<serde_json::Value> = coverage.iter()
            .map(|(cat, count, pct)| serde_json::json!({"category": cat, "files": count, "total": total_files, "percent": *pct as u32}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&json).expect(crate::errors::JSON_SERIALIZE));
    } else {
        let rows: Vec<Vec<String>> = coverage.iter()
            .map(|(cat, count, pct)| vec![cat.clone(), count.to_string(), format!("{:.0}%", pct)])
            .collect();
        tables::print_table(&["Category", "Files", "Coverage"], &rows);
        println!("Total files: {total_files}");
    }
    Ok(())
}

/// Find markdown files with no tags at all.
pub fn orphan_files(db_path: &str) -> Result<()> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;
    let fm_table = crate::errors::ctx(read_txn.open_table(FRONTMATTER), "Failed to open frontmatter table")?;
    let tag_table = crate::errors::ctx(read_txn.open_table(TAG_INDEX), "Failed to open tag_index table")?;

    // Collect all files that have at least one tag
    let mut tagged_files: HashSet<String> = HashSet::new();
    let tag_iter = crate::errors::ctx(tag_table.iter(), "Failed to iterate tag_index")?;
    for entry in tag_iter {
        let (_, value) = crate::errors::ctx(entry, "Failed to read tag entry")?;
        let files: Vec<String> = serde_json::from_str(value.value())
            .context("Failed to parse tag file list")?;
        for file in files {
            tagged_files.insert(file);
        }
    }

    // Find files in frontmatter that have no tags
    let mut orphans: Vec<String> = Vec::new();
    let fm_iter = crate::errors::ctx(fm_table.iter(), "Failed to iterate frontmatter")?;
    for entry in fm_iter {
        let (key, _) = crate::errors::ctx(entry, "Failed to read frontmatter entry")?;
        let file = key.value().to_string();
        if !tagged_files.contains(&file) {
            orphans.push(file);
        }
    }
    orphans.sort();

    if crate::json_output::is_json_mode() {
        println!("{}", serde_json::to_string(&orphans).expect(crate::errors::JSON_SERIALIZE));
    } else if orphans.is_empty() {
        println!("All files have tags.");
    } else {
        println!("{} file(s) with no tags:", orphans.len());
        for file in &orphans {
            println!("  {file}");
        }
    }
    Ok(())
}

/// Run all tag validations without building.
pub fn check_tags(config: &TagsConfig) -> Result<()> {
    let file_index = crate::file_index::FileIndex::build()?;
    let files = file_index.scan(&config.standard, true);
    if files.is_empty() {
        println!("No files to check.");
        return Ok(());
    }

    // Parse all frontmatter
    let mut all_frontmatter: HashMap<String, serde_json::Value> = HashMap::new();
    let mut tag_to_files: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut errors: Vec<String> = Vec::new();

    for input in &files {
        let content = fs::read_to_string(input)
            .with_context(|| format!("Failed to read {}", input.display()))?;
        if let Some(fm) = parse_frontmatter(&content) {
            let file_key = input.display().to_string();
            let mut file_tags: HashSet<String> = HashSet::new();

            if let Some(obj) = fm.as_object() {
                for (key, value) in obj {
                    match value {
                        serde_json::Value::Array(items) => {
                            // Check sorted order
                            if config.sorted_tags {
                                let strs: Vec<&str> = items.iter()
                                    .filter_map(|i| i.as_str())
                                    .collect();
                                for pair in strs.windows(2) {
                                    if pair[0] > pair[1] {
                                        errors.push(format!("Unsorted: {} field '{}': '{}' before '{}'", file_key, key, pair[0], pair[1]));
                                        break;
                                    }
                                }
                            }
                            for item in items {
                                if let Some(s) = item.as_str() {
                                    if !file_tags.insert(s.to_string()) {
                                        errors.push(format!("Duplicate tag: {s} in {file_key}"));
                                    }
                                    tag_to_files.entry(s.to_string()).or_default().insert(file_key.clone());
                                }
                            }
                        }
                        serde_json::Value::String(s) => {
                            let tag = format!("{key}:{s}");
                            if !file_tags.insert(tag.clone()) {
                                errors.push(format!("Duplicate tag: {tag} in {file_key}"));
                            }
                            tag_to_files.entry(tag).or_default().insert(file_key.clone());
                        }
                        _ => {}
                    }
                }
            }
            all_frontmatter.insert(file_key, fm);
        }
    }

    // Required fields
    for input in &files {
        let file_key = input.display().to_string();
        let fm = all_frontmatter.get(&file_key);
        let obj = fm.and_then(|v| v.as_object());
        for field in &config.required_fields {
            let has_field = obj.is_some_and(|o| {
                o.get(field).is_some_and(|v| match v {
                    serde_json::Value::Array(a) => !a.is_empty(),
                    serde_json::Value::String(s) => !s.is_empty(),
                    serde_json::Value::Null => false,
                    _ => true,
                })
            });
            if !has_field {
                errors.push(format!("Missing required field '{field}' in {file_key}"));
            }
        }
    }

    // Required field groups
    if !config.required_field_groups.is_empty() {
        for input in &files {
            let file_key = input.display().to_string();
            let fm = all_frontmatter.get(&file_key);
            let obj = fm.and_then(|v| v.as_object());
            let satisfies_any = config.required_field_groups.iter().any(|group| {
                group.iter().all(|field| {
                    obj.is_some_and(|o| {
                        o.get(field).is_some_and(|v| match v {
                            serde_json::Value::Array(a) => !a.is_empty(),
                            serde_json::Value::String(s) => !s.is_empty(),
                            serde_json::Value::Null => false,
                            _ => true,
                        })
                    })
                })
            });
            if !satisfies_any {
                let group_strs: Vec<String> = config.required_field_groups.iter()
                    .map(|g| format!("[{}]", g.join(", ")))
                    .collect();
                errors.push(format!("Missing required field group in {}: none of {}", file_key, group_strs.join(" or ")));
            }
        }
    }

    // Unknown tags
    let dir = Path::new(&config.tags_dir);
    if dir.is_dir() {
        let allowed = load_tags_dir(dir)?;

        for (tag, tag_files) in &tag_to_files {
            if !allowed.contains(tag) {
                let files_str: Vec<&str> = tag_files.iter().map(std::string::String::as_str).collect();
                errors.push(format!("Unknown tag '{}' in {}", tag, files_str.join(", ")));
            }
        }

        // Unused tags
        let used_tags: HashSet<&String> = tag_to_files.keys().collect();
        for tag in &allowed {
            if !used_tags.contains(tag) {
                errors.push(format!("Unused tag '{}' in {}", tag, config.tags_dir));
            }
        }

        // Required values
        for (file_key, fm) in &all_frontmatter {
            if let Some(obj) = fm.as_object() {
                for field in &config.required_values {
                    if let Some(serde_json::Value::String(val)) = obj.get(field) {
                        let tag = format!("{field}:{val}");
                        if !allowed.contains(&tag) {
                            errors.push(format!("Invalid value {}={} in {} (not in {}/{}.txt)", field, val, file_key, config.tags_dir, field));
                        }
                    }
                }
            }
        }
    }

    // Unique fields
    if !config.unique_fields.is_empty() {
        let mut field_values: HashMap<(String, String), Vec<String>> = HashMap::new();
        for (file_key, fm) in &all_frontmatter {
            if let Some(obj) = fm.as_object() {
                for field in &config.unique_fields {
                    if let Some(serde_json::Value::String(val)) = obj.get(field) {
                        field_values.entry((field.clone(), val.clone())).or_default().push(file_key.clone());
                    }
                }
            }
        }
        for ((field, val), dup_files) in &field_values {
            if dup_files.len() > 1 {
                errors.push(format!("Duplicate {}='{}' in {}", field, val, dup_files.join(", ")));
            }
        }
    }

    // Field types
    for (file_key, fm) in &all_frontmatter {
        if let Some(obj) = fm.as_object() {
            for (field, expected_type) in &config.field_types {
                if let Some(val) = obj.get(field) {
                    let ok = match expected_type.as_str() {
                        "list" => matches!(val, serde_json::Value::Array(_)),
                        "scalar" => matches!(val, serde_json::Value::String(_)),
                        "number" => matches!(val, serde_json::Value::String(s) if s.parse::<f64>().is_ok()),
                        _ => true,
                    };
                    if !ok {
                        errors.push(format!("Type mismatch: '{field}' in {file_key} expected {expected_type}"));
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        if crate::json_output::is_json_mode() {
            let out = serde_json::json!({
                "ok": true,
                "files_checked": all_frontmatter.len(),
                "issues": serde_json::Value::Array(Vec::new()),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            println!("All checks passed ({} files).", all_frontmatter.len());
        }
    } else {
        errors.sort();
        let mut msg = format!("{} issue(s) found:\n", errors.len());
        for err in &errors {
            let _ = writeln!(msg, "  {err}");
        }
        bail!("{}", msg.trim_end());
    }

    Ok(())
}

/// Suggest tags for a file based on similarity to other tagged files.
pub fn suggest_tags(db_path: &str, path: &str, config: &TagsConfig) -> Result<()> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;
    let tag_table = crate::errors::ctx(read_txn.open_table(TAG_INDEX), "Failed to open tag_index table")?;

    // Build file -> tags and tag -> files maps
    let mut file_tags: HashMap<String, HashSet<String>> = HashMap::new();
    let tag_iter = crate::errors::ctx(tag_table.iter(), "Failed to iterate tag_index")?;
    for entry in tag_iter {
        let (key, value) = crate::errors::ctx(entry, "Failed to read tag entry")?;
        let tag = key.value().to_string();
        let files: Vec<String> = serde_json::from_str(value.value())
            .context("Failed to parse tag file list")?;
        for file in files {
            file_tags.entry(file).or_default().insert(tag.clone());
        }
    }

    // Find the target file
    let target_tags = file_tags.iter()
        .find(|(f, _)| path_matches(f, path))
        .map(|(_, tags)| tags.clone())
        .unwrap_or_default();

    if target_tags.is_empty() {
        // No tags — suggest most common tags
        let mut tag_counts: HashMap<&String, usize> = HashMap::new();
        for tags in file_tags.values() {
            for tag in tags {
                *tag_counts.entry(tag).or_default() += 1;
            }
        }
        let mut sorted: Vec<(&&String, &usize)> = tag_counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        println!("File has no tags. Most common tags across all files:");
        for (tag, count) in sorted.iter().take(config.common_tags_limit) {
            println!("  {tag} ({count} files)");
        }
        return Ok(());
    }

    // Compute Jaccard similarity to every other file
    let mut similarities: Vec<(String, f64, HashSet<String>)> = Vec::new();
    for (file, tags) in &file_tags {
        if path_matches(file, path) {
            continue;
        }
        let intersection = target_tags.intersection(tags).count();
        if intersection == 0 {
            continue;
        }
        let union = target_tags.union(tags).count();
        let jaccard = intersection as f64 / union as f64;
        // Tags this file has that the target doesn't
        let new_tags: HashSet<String> = tags.difference(&target_tags).cloned().collect();
        if !new_tags.is_empty() {
            similarities.push((file.clone(), jaccard, new_tags));
        }
    }
    similarities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Count suggested tags weighted by similarity
    let mut suggestions: HashMap<String, f64> = HashMap::new();
    for (_, sim, new_tags) in similarities.iter().take(config.similar_files_limit) {
        for tag in new_tags {
            *suggestions.entry(tag.clone()).or_default() += sim;
        }
    }

    let mut sorted_suggestions: Vec<(String, f64)> = suggestions.into_iter().collect();
    sorted_suggestions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    if crate::json_output::is_json_mode() {
        let json: Vec<serde_json::Value> = sorted_suggestions.iter().take(config.suggested_tags_limit)
            .map(|(tag, score)| serde_json::json!({"tag": tag, "score": format!("{:.2}", score)}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&json).expect(crate::errors::JSON_SERIALIZE));
    } else if sorted_suggestions.is_empty() {
        println!("No suggestions — file already has all tags of similar files.");
    } else {
        println!("Suggested tags for {path}:");
        let rows: Vec<Vec<String>> = sorted_suggestions.iter().take(config.suggested_tags_limit)
            .map(|(tag, score)| vec![tag.clone(), format!("{:.2}", score)])
            .collect();
        tables::print_table(&["Tag", "Score"], &rows);
    }
    Ok(())
}

// --- Helper functions ---

/// Check if a stored file path matches a user-provided path.
/// Matches exactly, or as a suffix after a `/` path separator.
fn path_matches(stored: &str, query: &str) -> bool {
    if stored == query {
        return true;
    }
    // Suffix match: stored must end with /query.
    // Subtraction is safe: the guard ensures stored.len() > query.len(),
    // so stored.len() - query.len() - 1 cannot underflow.
    if stored.len() > query.len() {
        let boundary = stored.len() - query.len() - 1;
        if stored.as_bytes().get(boundary) == Some(&b'/')
            && stored.ends_with(query)
        {
            return true;
        }
    }
    false
}

/// Load all tags from the database as a `HashSet`.
fn load_all_tags(db_path: &str) -> Result<HashSet<String>> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;
    let table = crate::errors::ctx(read_txn.open_table(TAG_INDEX), "Failed to open tag_index table")?;

    let mut tags = HashSet::new();
    let iter = crate::errors::ctx(table.iter(), "Failed to iterate tag_index")?;
    for entry in iter {
        let (key, _) = crate::errors::ctx(entry, "Failed to read tag entry")?;
        tags.insert(key.value().to_string());
    }

    Ok(tags)
}

/// Load all tags from the database as a sorted Vec.
fn load_all_tags_sorted(db_path: &str) -> Result<Vec<String>> {
    let mut tags: Vec<String> = load_all_tags(db_path)?.into_iter().collect();
    tags.sort();
    Ok(tags)
}

/// Load tag -> file count mapping from the database.
fn load_tag_counts(db_path: &str) -> Result<HashMap<String, usize>> {
    let db = open_tags_db(db_path)?;
    let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction")?;
    let table = crate::errors::ctx(read_txn.open_table(TAG_INDEX), "Failed to open tag_index table")?;

    let mut counts = HashMap::new();
    let iter = crate::errors::ctx(table.iter(), "Failed to iterate tag_index")?;
    for entry in iter {
        let (key, value) = crate::errors::ctx(entry, "Failed to read tag entry")?;
        let files: Vec<String> = serde_json::from_str(value.value())
            .context("Failed to parse tag file list")?;
        counts.insert(key.value().to_string(), files.len());
    }

    Ok(counts)
}

/// Load allowed tags from a directory of `.txt` files.
/// Each file `<name>.txt` contributes tags as `<name>:<line>`.
/// Fails if the same tag appears in multiple files.
pub fn load_tags_dir(dir: &Path) -> Result<HashSet<String>> {
    let mut tags = HashSet::new();
    let mut tag_source: HashMap<String, String> = HashMap::new();
    let mut duplicates: Vec<(String, String, String)> = Vec::new(); // (tag, file1, file2)
    let mut entries: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("Failed to read tags directory: {}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("Failed to read entry in tags directory: {}", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        let category = path.file_stem()
            .and_then(|s| s.to_str())
            .context("Invalid filename in tags_dir")?;
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        for line in content.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                // tags.txt holds bare (uncategorized) tags — the same file
                // collect writes them to; every other file prefixes its stem
                // as the category.
                let tag = if category == "tags" {
                    line.to_string()
                } else {
                    format!("{category}:{line}")
                };
                if let Some(prev_file) = tag_source.get(&tag) {
                    duplicates.push((tag.clone(), prev_file.clone(), filename.clone()));
                } else {
                    tag_source.insert(tag.clone(), filename.clone());
                }
                tags.insert(tag);
            }
        }
    }

    if !duplicates.is_empty() {
        duplicates.sort();
        let mut msg = String::from("Duplicate tags found across tags files:\n");
        for (tag, file1, file2) in &duplicates {
            let _ = writeln!(msg, "  {tag} in {file1} and {file2}");
        }
        bail!("{}", msg.trim_end());
    }

    Ok(tags)
}

/// Merge tags from another project's tags directory into the current one.
/// For each .txt file in `source_dir`:
///   - If a file with the same name exists in `tags_dir`, merge (union) and sort the entries.
///   - Otherwise, copy the file as-is.
///
/// Also copies files that exist in the destination but not the source back to the source.
pub fn merge_tags(tags_dir: &str, source_dir: &str) -> Result<()> {
    let src = Path::new(source_dir);
    if !src.is_dir() {
        bail!("Source directory `{source_dir}` does not exist or is not a directory");
    }
    let dest = Path::new(tags_dir);
    if !dest.is_dir() {
        bail!("Tags directory `{tags_dir}` does not exist or is not a directory");
    }

    let mut merged_count = 0;
    let mut copied_count = 0;

    for entry in crate::errors::ctx(fs::read_dir(src), &format!("Failed to read source directory {}", src.display()))? {
        let entry = crate::errors::ctx(entry, &format!("Failed to read entry in source directory {}", src.display()))?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "txt") {
            continue;
        }
        let filename = path.file_name().unwrap();
        let dest_path = dest.join(filename);

        let source_content = crate::errors::ctx(fs::read_to_string(&path), &format!("Failed to read tags source: {}", path.display()))?;
        let source_entries: HashSet<String> = source_content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();

        if dest_path.exists() {
            let dest_content = crate::errors::ctx(fs::read_to_string(&dest_path), &format!("Failed to read tags dest: {}", dest_path.display()))?;
            let mut all_entries: HashSet<String> = dest_content
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            let before = all_entries.len();
            all_entries.extend(source_entries);
            let added = all_entries.len() - before;
            if added > 0 {
                let mut sorted: Vec<String> = all_entries.into_iter().collect();
                sorted.sort();
                crate::errors::ctx(fs::write(&dest_path, sorted.join("\n") + "\n"), &format!("Failed to write {}", dest_path.display()))?;
                merged_count += 1;
                println!("  Merged: {} ({} new entries)", filename.to_string_lossy(), added);
            }
        } else {
            let mut sorted: Vec<String> = source_entries.into_iter().collect();
            sorted.sort();
            crate::errors::ctx(fs::write(&dest_path, sorted.join("\n") + "\n"), &format!("Failed to write {}", dest_path.display()))?;
            copied_count += 1;
            println!("  Copied: {}", filename.to_string_lossy());
        }
    }

    // Copy files that exist in destination but not in source back to source
    for entry in crate::errors::ctx(fs::read_dir(dest), &format!("Failed to read destination directory {}", dest.display()))? {
        let entry = crate::errors::ctx(entry, &format!("Failed to read entry in destination directory {}", dest.display()))?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "txt") {
            continue;
        }
        let filename = path.file_name().unwrap();
        let src_path = src.join(filename);
        if !src_path.exists() {
            crate::errors::ctx(fs::copy(&path, &src_path), &format!("Failed to copy {} to {}", path.display(), src_path.display()))?;
            copied_count += 1;
            println!("  Copied to source: {}", filename.to_string_lossy());
        }
    }

    println!("Done. Merged {merged_count} file(s), copied {copied_count} new file(s).");
    Ok(())
}

/// Check if a tag is in the allowed set.
fn tag_matches_allowed(tag: &str, allowed: &HashSet<String>) -> bool {
    allowed.contains(tag)
}

/// Find the most similar tag in the allowed set using Levenshtein distance.
/// The threshold scales with tag length to avoid spurious matches on short tags.
fn find_similar_tag(tag: &str, allowed: &HashSet<String>) -> Option<String> {
    // Scale threshold: at least 1, at most 3, roughly tag_char_count/3
    let max_dist = (tag.chars().count() / 3).clamp(1, 3);
    let mut best: Option<(String, usize)> = None;
    for candidate in allowed {
        // Skip wildcard patterns
        if candidate.ends_with('*') {
            continue;
        }
        let dist = levenshtein(tag, candidate);
        if dist > 0 && dist <= max_dist
            && (best.is_none() || dist < best.as_ref().unwrap().1)
        {
            best = Some((candidate.clone(), dist));
        }
    }
    best.map(|(s, _)| s)
}

/// Compute Levenshtein edit distance between two strings.
/// Uses char-level comparison for correct handling of non-ASCII input.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 { return b_len; }
    if b_len == 0 { return a_len; }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, ca) in a_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j] + cost)
                .min(curr[j] + 1)
                .min(prev[j + 1] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_frontmatter ---

    #[test]
    fn frontmatter_basic() {
        let content = "---\ntitle: Hello\ntags:\n  - foo\n---\n# Body\n";
        let fm = parse_frontmatter(content).unwrap();
        assert_eq!(fm["title"], "Hello");
        assert_eq!(fm["tags"][0], "foo");
    }

    #[test]
    fn frontmatter_no_markers() {
        assert!(parse_frontmatter("# Just a heading\n").is_none());
    }

    #[test]
    fn frontmatter_only_opening() {
        assert!(parse_frontmatter("---\ntitle: Hello\n").is_none());
    }

    #[test]
    fn frontmatter_empty_block() {
        // Empty frontmatter (no content between --- markers) returns None
        // because there is no "\n---" after the opening marker's newline
        let content = "---\n---\n# Body\n";
        assert!(parse_frontmatter(content).is_none());
    }

    // --- parse_simple_yaml ---

    #[test]
    fn yaml_scalar_values() {
        let block = "title: Hello World\nlevel: beginner";
        let v = parse_simple_yaml(block);
        assert_eq!(v["title"], "Hello World");
        assert_eq!(v["level"], "beginner");
    }

    #[test]
    fn yaml_colon_in_value() {
        let block = "url: https://example.com/path\ntime: 10:30";
        let v = parse_simple_yaml(block);
        assert_eq!(v["url"], "https://example.com/path");
        assert_eq!(v["time"], "10:30");
    }

    #[test]
    fn yaml_multiline_list() {
        let block = "tags:\n  - alpha\n  - beta\n  - gamma";
        let v = parse_simple_yaml(block);
        let tags = v["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0], "alpha");
        assert_eq!(tags[1], "beta");
        assert_eq!(tags[2], "gamma");
    }

    #[test]
    fn yaml_inline_list() {
        let block = "tags: [alpha, beta, gamma]";
        let v = parse_simple_yaml(block);
        let tags = v["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0], "alpha");
        assert_eq!(tags[2], "gamma");
    }

    #[test]
    fn yaml_empty_inline_list() {
        let block = "tags: []";
        let v = parse_simple_yaml(block);
        let tags = v["tags"].as_array().unwrap();
        assert!(tags.is_empty());
    }

    #[test]
    fn yaml_quoted_values() {
        let block = "name: \"quoted value\"\nother: 'single quoted'";
        let v = parse_simple_yaml(block);
        assert_eq!(v["name"], "quoted value");
        assert_eq!(v["other"], "single quoted");
    }

    #[test]
    fn yaml_list_no_leading_whitespace() {
        let block = "tags:\n- alpha\n- beta";
        let v = parse_simple_yaml(block);
        let tags = v["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], "alpha");
    }

    #[test]
    fn yaml_empty_block() {
        let v = parse_simple_yaml("");
        assert!(v.as_object().unwrap().is_empty());
    }

    #[test]
    fn yaml_key_no_space_after_colon() {
        // "tags:" with no space starts a list
        let block = "tags:\n  - item";
        let v = parse_simple_yaml(block);
        let tags = v["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 1);
    }

    // --- strip_yaml_quotes ---

    #[test]
    fn strip_double_quotes() {
        assert_eq!(strip_yaml_quotes("\"hello\""), "hello");
    }

    #[test]
    fn strip_single_quotes() {
        assert_eq!(strip_yaml_quotes("'hello'"), "hello");
    }

    #[test]
    fn strip_no_quotes() {
        assert_eq!(strip_yaml_quotes("hello"), "hello");
    }

    #[test]
    fn strip_mismatched_quotes() {
        assert_eq!(strip_yaml_quotes("\"hello'"), "\"hello'");
    }

    #[test]
    fn strip_single_char() {
        assert_eq!(strip_yaml_quotes("x"), "x");
    }

    #[test]
    fn strip_empty() {
        assert_eq!(strip_yaml_quotes(""), "");
    }

    // --- path_matches ---

    #[test]
    fn path_exact_match() {
        assert!(path_matches("foo.md", "foo.md"));
    }

    #[test]
    fn path_suffix_match() {
        assert!(path_matches("sub/foo.md", "foo.md"));
        assert!(path_matches("a/b/foo.md", "b/foo.md"));
    }

    #[test]
    fn path_no_false_suffix() {
        // "barfoo.md" should NOT match query "foo.md"
        assert!(!path_matches("barfoo.md", "foo.md"));
        assert!(!path_matches("sub/barfoo.md", "foo.md"));
    }

    #[test]
    fn path_query_longer_than_stored() {
        assert!(!path_matches("foo.md", "sub/foo.md"));
    }

    #[test]
    fn path_empty_strings() {
        assert!(path_matches("", ""));
        assert!(!path_matches("foo", ""));
        // empty query with non-empty stored: stored.len() > 0, boundary would need -1
        // but stored.len() > query.len() is true, boundary = stored.len() - 0 - 1
        // stored.as_bytes().get(boundary) checks for '/'
    }

    // --- levenshtein ---

    #[test]
    fn levenshtein_identical() {
        assert_eq!(levenshtein("docker", "docker"), 0);
    }

    #[test]
    fn levenshtein_empty() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn levenshtein_one_edit() {
        assert_eq!(levenshtein("docker", "dockker"), 1);
        assert_eq!(levenshtein("python", "pyhton"), 2); // transposition = 2 edits
    }

    #[test]
    fn levenshtein_single_char() {
        assert_eq!(levenshtein("a", "b"), 1);
        assert_eq!(levenshtein("a", "a"), 0);
    }

    // --- tag_matches_allowed ---

    #[test]
    fn tag_allowed_exact() {
        let allowed: HashSet<String> = ["docker", "python"].iter().map(ToString::to_string).collect();
        assert!(tag_matches_allowed("docker", &allowed));
        assert!(!tag_matches_allowed("rust", &allowed));
    }

}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(TagsProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "tags",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "output", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Output tags database file path" },
            crate::config::FieldSpec { name: "tags_dir", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Directory containing tag list files" },
            crate::config::FieldSpec { name: "required_fields", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Frontmatter fields that every markdown file must have" },
            crate::config::FieldSpec { name: "required_values", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Scalar fields whose values must exist in the tag lists file" },
            crate::config::FieldSpec { name: "unique_fields", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Fields whose values must be unique across all files" },
            crate::config::FieldSpec { name: "field_types", ty: crate::config::FieldType::Table,
                affects_output: true, required: false,
                doc: "Expected types for fields: scalar, list, or number" },
            crate::config::FieldSpec { name: "required_field_groups", ty: crate::config::FieldType::Array,
                affects_output: true, required: false,
                doc: "Groups of fields where at least one group must be fully present" },
            crate::config::FieldSpec { name: "sorted_tags", ty: crate::config::FieldType::Bool,
                affects_output: true, required: false,
                doc: "Require list-type fields to have items in sorted order" },
            crate::config::FieldSpec { name: "check_unused", ty: crate::config::FieldType::Bool,
                affects_output: true, required: false,
                doc: "Fail the build when tags in the allowlist are not used by any file" },
            crate::config::FieldSpec { name: "similar_files_limit", ty: crate::config::FieldType::Integer,
                affects_output: false, required: false,
                doc: "How many most-similar files to inspect when generating tag suggestions" },
            crate::config::FieldSpec { name: "suggested_tags_limit", ty: crate::config::FieldType::Integer,
                affects_output: false, required: false,
                doc: "Maximum tag suggestions emitted by `tags suggest`" },
            crate::config::FieldSpec { name: "common_tags_limit", ty: crate::config::FieldType::Integer,
                affects_output: false, required: false,
                doc: "Maximum 'most common tags' reported when a file has no tags of its own" },
        ],
        omit_standard_fields: &["command", "formats", "args", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] }),
        defaults: None,
        defconfig_json: crate::registries::default_config_json::<TagsConfig>,
        keywords: &["ctags", "tags", "generator", "code-navigation"],
        description: "Extract YAML frontmatter tags from markdown files into a searchable database",
        is_native: true,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
