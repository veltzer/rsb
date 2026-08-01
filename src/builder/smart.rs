use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::fs;

use crate::config::all_type_names;

const CONFIG_FILE: &str = "rsconstruct.toml";

/// Load rsconstruct.toml as a `toml_edit` document.
fn load_doc() -> Result<toml_edit::DocumentMut> {
    let content = fs::read_to_string(CONFIG_FILE)
        .with_context(|| format!("Failed to read {CONFIG_FILE}"))?;
    content.parse()
        .with_context(|| format!("Failed to parse {CONFIG_FILE}"))
}

/// Write a `toml_edit` document back to rsconstruct.toml.
fn save_doc(doc: &toml_edit::DocumentMut) -> Result<()> {
    fs::write(CONFIG_FILE, doc.to_string())
        .with_context(|| format!("Failed to write {CONFIG_FILE}"))
}

/// Get or create the [processor] table in the document.
fn processor_table(doc: &mut toml_edit::DocumentMut) -> Result<&mut toml_edit::Table> {
    doc.entry("processor")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .context("[processor] must be a table")
}

/// Validate that a processor name is a known builtin type.
fn validate_name(name: &str) -> Result<()> {
    let all = all_type_names();
    if !all.contains(&name) {
        bail!(
            "Unknown processor '{name}'. Run 'rsconstruct processors list --all' to see available processors."
        );
    }
    Ok(())
}

/// Disable all processors by removing all [processor.*] sections.
pub fn disable_all() -> Result<()> {
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
    let mut count = 0;

    for key in &keys {
        if table.remove(key).is_some() {
            count += 1;
        }
    }

    save_doc(&doc)?;
    println!("Removed {count} processor sections from {CONFIG_FILE}.");
    Ok(())
}

/// Enable all processors by adding [processor.NAME] sections for all builtin types.
pub fn enable_all() -> Result<()> {
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;
    let all_names = all_type_names();
    let mut count = 0;

    for name in &all_names {
        if table.get(name).is_none() {
            table.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
            count += 1;
        }
    }

    save_doc(&doc)?;
    println!("Added {count} processor sections to {CONFIG_FILE}.");
    Ok(())
}

/// Disable a single processor by removing its [processor.NAME] section.
pub fn disable(name: &str) -> Result<()> {
    validate_name(name)?;
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;

    if table.remove(name).is_some() {
        save_doc(&doc)?;
        println!("Removed processor '{name}'.");
    } else {
        println!("Processor '{name}' is not declared.");
    }
    Ok(())
}

/// Enable a single processor by adding an empty [processor.NAME] section.
pub fn enable(name: &str) -> Result<()> {
    validate_name(name)?;
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;

    if table.get(name).is_some() {
        println!("Processor '{name}' is already declared.");
    } else {
        table.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
        save_doc(&doc)?;
        println!("Added processor '{name}'.");
    }
    Ok(())
}

/// Enable only processors whose files are detected in the project.
pub fn enable_detected(detected: &HashSet<String>) -> Result<()> {
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;
    let mut count = 0;

    for name in detected {
        if table.get(name.as_str()).is_none() {
            table.insert(name.as_str(), toml_edit::Item::Table(toml_edit::Table::new()));
            count += 1;
        }
    }

    save_doc(&doc)?;
    println!("Added {count} detected processor sections to {CONFIG_FILE}.");
    Ok(())
}

/// Remove all processor sections, returning to empty config.
pub fn reset() -> Result<()> {
    disable_all()
}

/// Remove all processor sections, then add only the listed ones.
pub fn only(names: &[String]) -> Result<()> {
    for name in names {
        validate_name(name)?;
    }

    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;

    // Remove all existing processor sections
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
    for key in &keys {
        table.remove(key);
    }

    // Add only the requested ones
    for name in names {
        table.insert(name.as_str(), toml_edit::Item::Table(toml_edit::Table::new()));
    }

    save_doc(&doc)?;
    println!("Active processors: {}", names.join(", "));
    Ok(())
}

/// Remove all processor sections, then add only detected ones.
pub fn minimal(detected: &HashSet<String>) -> Result<()> {
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;

    // Remove all
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
    for key in &keys {
        table.remove(key);
    }

    // Add detected
    for name in detected {
        table.insert(name.as_str(), toml_edit::Item::Table(toml_edit::Table::new()));
    }

    save_doc(&doc)?;
    if !detected.is_empty() {
        let mut names: Vec<&String> = detected.iter().collect();
        names.sort();
        println!("Minimal config: {}", names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
    } else {
        println!("No processors detected.");
    }
    Ok(())
}

/// Add sections for processors whose files are detected AND tools are installed.
pub fn enable_if_available(available: &HashSet<String>) -> Result<()> {
    enable_detected(available)
}

/// Auto-detect relevant processors and add them to rsconstruct.toml.
/// Only adds processors whose files are detected AND whose tools are installed.
/// Does not remove existing processor sections.
pub fn auto(available: &HashSet<String>) -> Result<()> {
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;
    let mut added = Vec::new();

    for name in available {
        if table.get(name.as_str()).is_none() {
            table.insert(name.as_str(), toml_edit::Item::Table(toml_edit::Table::new()));
            added.push(name.as_str());
        }
    }

    if added.is_empty() {
        println!("No new processors to add (all detected processors are already declared).");
    } else {
        save_doc(&doc)?;
        let mut added = added;
        added.sort_unstable();
        println!("Added {} processor(s): {}", added.len(), added.join(", "));
    }
    Ok(())
}

/// Get or create the [analyzer] table in the document.
fn analyzer_table(doc: &mut toml_edit::DocumentMut) -> Result<&mut toml_edit::Table> {
    doc.entry("analyzer")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .context("[analyzer] must be a table")
}

/// Delete a single entry (processor or analyzer) by iname from the given section table.
/// Supports both simple inames ("ruff") and dotted named instances ("pylint.core").
fn delete_iname(section_table: &mut toml_edit::Table, iname: &str) -> bool {
    if let Some(dot) = iname.find('.') {
        let type_name = &iname[..dot];
        let sub_name = &iname[dot + 1..];
        if let Some(type_table) = section_table.get_mut(type_name).and_then(|t| t.as_table_mut())
            && type_table.remove(sub_name).is_some() {
                if type_table.is_empty() {
                    section_table.remove(type_name);
                }
                return true;
            }
        false
    } else {
        section_table.remove(iname).is_some()
    }
}

/// Set `enabled = VALUE` on a single entry by iname from the given section table.
/// Supports both simple inames ("ruff") and dotted named instances ("pylint.core").
/// Returns an error if the iname is not found.
fn set_enabled_iname(section_table: &mut toml_edit::Table, iname: &str, value: bool, section: &str) -> Result<()> {
    let entry = if let Some(dot) = iname.find('.') {
        let type_name = &iname[..dot];
        let sub_name = &iname[dot + 1..];
        section_table
            .get_mut(type_name)
            .and_then(|t| t.as_table_mut())
            .and_then(|t| t.get_mut(sub_name))
            .and_then(|v| v.as_table_mut())
    } else {
        section_table
            .get_mut(iname)
            .and_then(|v| v.as_table_mut())
    };

    match entry {
        Some(t) => {
            t.insert("enabled", toml_edit::value(value));
            Ok(())
        }
        None => bail!("{section} '{iname}' is not declared in rsconstruct.toml."),
    }
}

/// Delete a processor by iname from rsconstruct.toml.
pub fn delete_processor(iname: &str) -> Result<()> {
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;
    if delete_iname(table, iname) {
        save_doc(&doc)?;
        println!("Deleted processor '{iname}'.");
    } else {
        println!("Processor '{iname}' is not declared.");
    }
    Ok(())
}

/// Set enabled = false on a processor by iname.
pub fn disable_processor(iname: &str) -> Result<()> {
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;
    set_enabled_iname(table, iname, false, "Processor")?;
    save_doc(&doc)?;
    println!("Disabled processor '{iname}'.");
    Ok(())
}

/// Set enabled = true on a processor by iname.
pub fn enable_processor(iname: &str) -> Result<()> {
    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;
    set_enabled_iname(table, iname, true, "Processor")?;
    save_doc(&doc)?;
    println!("Enabled processor '{iname}'.");
    Ok(())
}

/// Delete an analyzer by iname from rsconstruct.toml.
pub fn delete_analyzer(iname: &str) -> Result<()> {
    let mut doc = load_doc()?;
    let table = analyzer_table(&mut doc)?;
    if delete_iname(table, iname) {
        save_doc(&doc)?;
        println!("Deleted analyzer '{iname}'.");
    } else {
        println!("Analyzer '{iname}' is not declared.");
    }
    Ok(())
}

/// Set enabled = false on an analyzer by iname.
pub fn disable_analyzer(iname: &str) -> Result<()> {
    let mut doc = load_doc()?;
    let table = analyzer_table(&mut doc)?;
    set_enabled_iname(table, iname, false, "Analyzer")?;
    save_doc(&doc)?;
    println!("Disabled analyzer '{iname}'.");
    Ok(())
}

/// Set enabled = true on an analyzer by iname.
pub fn enable_analyzer(iname: &str) -> Result<()> {
    let mut doc = load_doc()?;
    let table = analyzer_table(&mut doc)?;
    set_enabled_iname(table, iname, true, "Analyzer")?;
    save_doc(&doc)?;
    println!("Enabled analyzer '{iname}'.");
    Ok(())
}

/// Remove processors from rsconstruct.toml that don't match any files.
pub fn remove_no_file_processors(empty_processors: &[String]) -> Result<()> {
    if empty_processors.is_empty() {
        println!("All processors match at least one file.");
        return Ok(());
    }

    let mut doc = load_doc()?;
    let table = processor_table(&mut doc)?;
    let mut removed = Vec::new();

    for name in empty_processors {
        // Handle both single-instance (pylint) and the type part of named instances (pylint.core)
        let type_name = name.split('.').next().unwrap_or(name);
        if name.contains('.') {
            // Named instance: remove the sub-key from [processor.TYPE]
            if let Some(type_table) = table.get_mut(type_name).and_then(|t| t.as_table_mut()) {
                let sub_name = &name[type_name.len() + 1..];
                if type_table.remove(sub_name).is_some() {
                    removed.push(name.as_str());
                    // If the type table is now empty, remove it entirely
                    if type_table.is_empty() {
                        table.remove(type_name);
                    }
                }
            }
        } else if table.remove(name.as_str()).is_some() {
            removed.push(name.as_str());
        }
    }

    if removed.is_empty() {
        println!("No processors to remove.");
    } else {
        save_doc(&doc)?;
        println!("Removed {} processor(s) with no files: {}", removed.len(), removed.join(", "));
    }
    Ok(())
}
