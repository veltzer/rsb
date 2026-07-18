use std::collections::HashMap;
use anyhow::{Result, bail};
use crate::cli::ProcessorAction;
use crate::color;
use crate::tables;
use crate::config::ProcessorConfig;
use super::{Builder, sorted_keys};

/// Search processors by name, description, and keywords. Case-insensitive.
/// Uses static plugin metadata only — no processor instantiation.
pub fn search_processors(query: &str) -> Result<()> {
    let query_lower = query.to_lowercase();
    let mut matches: Vec<(&str, &[&str])> = Vec::new();

    for plugin in crate::registries::processor::all_plugins() {
        let name_match = plugin.name.to_lowercase().contains(&query_lower);
        let keyword_match = plugin.keywords.iter().any(|k| k.to_lowercase().contains(&query_lower));
        let desc_match = plugin.description.to_lowercase().contains(&query_lower);

        if name_match || keyword_match || desc_match {
            matches.push((plugin.name, plugin.keywords));
        }
    }

    matches.sort_by_key(|(name, _)| *name);

    if matches.is_empty() {
        println!("No processors matching '{query}'.");
        return Ok(());
    }

    if crate::json_output::is_json_mode() {
        let entries: Vec<serde_json::Value> = matches.iter().map(|(name, keywords)| {
            serde_json::json!({
                "name": name,
                "type": crate::registries::processor::processor_type_of(name).as_str(),
                "description": crate::registries::processor::description_of(name),
                "keywords": keywords,
            })
        }).collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    let rows: Vec<Vec<String>> = matches.iter().map(|(name, keywords)| {
        vec![
            name.to_string(),
            crate::registries::processor::processor_type_of(name).as_str().to_string(),
            crate::registries::processor::description_of(name).to_string(),
            keywords.join(", "),
        ]
    }).collect();
    tables::print_table(&["Name", "Type", "Description", "Keywords"], &rows);

    Ok(())
}

/// List all processor types (generator, checker, creator, explicit).
pub fn list_processor_types(verbose: bool) -> Result<()> {
    use strum::IntoEnumIterator;
    use crate::processors::ProcessorType;

    if crate::json_output::is_json_mode() {
        #[derive(serde::Serialize)]
        struct TypeEntry {
            name: &'static str,
            description: &'static str,
        }
        let entries: Vec<TypeEntry> = ProcessorType::iter()
            .map(|pt| TypeEntry { name: pt.as_str(), description: pt.description() })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if verbose {
        let rows: Vec<Vec<String>> = ProcessorType::iter()
            .map(|pt| vec![pt.as_str().to_string(), pt.description().to_string()])
            .collect();
        tables::print_table(&["Type", "Description"], &rows);
    } else {
        let rows: Vec<Vec<String>> = ProcessorType::iter()
            .map(|pt| vec![pt.as_str().to_string()])
            .collect();
        tables::print_table(&["Type"], &rows);
    }
    Ok(())
}

/// List all built-in processors (works without rsconstruct.toml).
/// Uses static plugin metadata — no processor instantiation needed.
/// Optionally filters by processor type (e.g., "checker", "generator").
pub fn list_processors_no_config(verbose: bool, type_filter: Option<&str>) -> Result<()> {
    let mut plugins: Vec<&crate::registries::ProcessorPlugin> =
        crate::registries::processor::all_plugins().collect();
    plugins.sort_by_key(|p| p.name);

    if let Some(filter) = type_filter {
        plugins.retain(|p| p.processor_type.as_str() == filter);
        if plugins.is_empty() {
            anyhow::bail!("No processors of type '{filter}'. Valid types: checker, generator, creator, explicit.");
        }
    }

    if crate::json_output::is_json_mode() {
        let entries: Vec<crate::json_output::ProcessorListEntry> = plugins.iter()
            .map(|p| crate::json_output::ProcessorListEntry {
                name: p.name.to_string(),
                processor_type: p.processor_type.as_str().to_string(),
                enabled: false,
                detected: false,
                batch: false, // not available from static metadata
                native: p.is_native,
                fix: p.can_fix,
                description: p.description.to_string(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if verbose {
        let rows: Vec<Vec<String>> = plugins.iter().map(|p| {
            let native_tag = if p.is_native { "native" } else { "external" };
            let fix_tag = tables::yes_no(p.can_fix);
            vec![p.name.to_string(), p.processor_type.as_str().to_string(), native_tag.to_string(), fix_tag.to_string(), p.description.to_string()]
        }).collect();
        tables::print_table(&["Name", "Type", "Native", "Fix", "Description"], &rows);
    } else {
        let rows: Vec<Vec<String>> = plugins.iter().map(|p| {
            let native_tag = if p.is_native { "native" } else { "external" };
            let fix_tag = tables::yes_no(p.can_fix);
            vec![p.name.to_string(), p.processor_type.as_str().to_string(), native_tag.to_string(), fix_tag.to_string()]
        }).collect();
        tables::print_table(&["Name", "Type", "Native", "Fix"], &rows);
    }

    Ok(())
}

/// Print a table mapping file extensions to the recommended processor for each.
pub fn list_recommendations() {
    // Each entry: (extension, processor, reason)
    let recommendations: &[(&str, &str, &str)] = &[
        (".py",          "ruff",         "fastest Python linter, replaces flake8/pylint"),
        (".c",           "cppcheck",     "best static analysis for C"),
        (".cc",          "cppcheck",     "best static analysis for C++"),
        (".h",           "cppcheck",     "best static analysis for C/C++ headers"),
        (".hh",          "cppcheck",     "best static analysis for C++ headers"),
        (".rs",          "clippy",       "official Rust linter"),
        (".js",          "eslint",       "industry standard JS/TS linter"),
        (".jsx",         "eslint",       "industry standard JS/TS linter"),
        (".ts",          "eslint",       "industry standard JS/TS linter"),
        (".tsx",         "eslint",       "industry standard JS/TS linter"),
        (".html",        "tidy",         "most thorough HTML validator"),
        (".htm",         "tidy",         "most thorough HTML validator"),
        (".css",         "stylelint",    "industry standard CSS linter"),
        (".scss",        "stylelint",    "industry standard CSS/SCSS linter"),
        (".sass",        "stylelint",    "industry standard CSS/Sass linter"),
        (".md",          "markdownlint", "comprehensive markdown linting"),
        (".yml",         "yamllint",     "best YAML syntax and style checker"),
        (".yaml",        "yamllint",     "best YAML syntax and style checker"),
        (".json",        "jsonlint",     "JSON syntax validator"),
        (".toml",        "taplo",        "TOML formatter and validator"),
        (".xml",         "xmllint",      "XML/DTD validator"),
        (".svg",         "xmllint",      "SVG is XML — xmllint validates structure"),
        (".java",        "checkstyle",   "Java style and static analysis"),
        (".sh",          "shellcheck",   "best shell script analyzer"),
        (".bash",        "shellcheck",   "best shell script analyzer"),
        (".lua",         "luacheck",     "Lua static analyzer"),
        (".pl",          "perlcritic",   "Perl best-practice checker"),
        (".pm",          "perlcritic",   "Perl module best-practice checker"),
        (".php",         "php_lint",     "PHP syntax checker"),
        (".tex",         "pdflatex",     "compile and validate LaTeX"),
        (".proto",       "protobuf",     "Protocol Buffer compiler"),
        (".mmd",         "mermaid",      "render Mermaid diagrams"),
        (".drawio",      "drawio",       "export draw.io diagrams"),
        (".tera",        "tera",         "Tera template renderer"),
        (".j2",          "jinja2",       "Jinja2 template renderer"),
        (".mako",        "mako",         "Mako template renderer"),
        ("Dockerfile",   "hadolint",     "best Dockerfile linter"),
        ("Makefile",     "make",         "run make to validate"),
        ("Cargo.toml",   "cargo",        "build and validate Rust project"),
        ("book.toml",    "mdbook",       "build mdBook documentation"),
        ("package.json", "npm",          "run npm to validate Node project"),
        ("Gemfile",      "gem",          "run bundler to validate Ruby project"),
        ("conf.py",      "sphinx",       "build Sphinx documentation"),
    ];

    if crate::json_output::is_json_mode() {
        let entries: Vec<serde_json::Value> = recommendations.iter()
            .map(|(ext, proc, reason)| serde_json::json!({
                "extension": ext,
                "processor": proc,
                "reason": reason,
            }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries).expect(crate::errors::JSON_SERIALIZE));
    } else {
        let rows: Vec<Vec<String>> = recommendations.iter()
            .map(|(ext, proc, reason)| vec![ext.to_string(), proc.to_string(), reason.to_string()])
            .collect();
        tables::print_table(&["Extension / File", "Processor", "Reason"], &rows);
    }
}

/// Return a JSON value containing only fields that differ from the default config.
/// Render a JSON value as a single-line table cell. Strings appear unquoted;
/// arrays and objects are rendered as compact JSON.
fn format_config_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null      => "—".to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b)   => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other                        => other.to_string(),
    }
}

fn config_diff(name: &str, current: &serde_json::Value) -> serde_json::Value {
    let default_json = ProcessorConfig::defconfig_json(name);
    let default_value = default_json
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok());
    let (Some(serde_json::Value::Object(default_obj)), serde_json::Value::Object(current_obj)) =
        (default_value.as_ref(), current)
    else {
        return current.clone();
    };
    let mut diff = serde_json::Map::new();
    for (key, val) in current_obj {
        match default_obj.get(key) {
            Some(def_val) if def_val == val => {}
            _ => { diff.insert(key.clone(), val.clone()); }
        }
    }
    serde_json::Value::Object(diff)
}

/// Print metadata annotations (required fields and checksum-affecting fields) for a processor.
/// Only shown in text mode (not JSON mode).
///
/// Columns: Field, Type, Default, Required, Checksum (and Description in verbose mode).
/// - Required: "yes" if the field is in `must_fields()` (must be set non-empty by user).
/// - Checksum: "yes" if the field is in `checksum_fields()` (changes affect what the tool produces
///   and trigger rebuilds).
fn print_processor_metadata(name: &str, verbose: bool) {
    use crate::config::{SCAN_FIELD_DESCRIPTIONS, SHARED_FIELD_DESCRIPTIONS};

    let proc_descs = crate::config::ProcessorConfig::field_descriptions_for(name)
        .unwrap_or(&[]);

    let must_fields: std::collections::HashSet<&str> =
        crate::config::ProcessorConfig::must_fields_for(name)
            .unwrap_or(&[])
            .iter()
            .copied()
            .collect();
    let checksum_fields: std::collections::HashSet<&str> =
        crate::config::ProcessorConfig::checksum_fields_for(name)
            .unwrap_or(&[])
            .iter()
            .copied()
            .collect();

    let defaults: serde_json::Value = crate::config::ProcessorConfig::defconfig_json(name)
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or(serde_json::Value::Null);

    // Processor-specific fields first, then shared dep/exec, then scan fields.
    let all_descs: Vec<(&str, &str)> = proc_descs.iter()
        .map(|(f, d)| (*f, *d))
        .chain(SHARED_FIELD_DESCRIPTIONS.iter().map(|(f, d)| (*f, *d)))
        .chain(SCAN_FIELD_DESCRIPTIONS.iter().map(|(f, d)| (*f, *d)))
        .collect();

    let plugin = crate::registries::processor::find_plugin(name);
    let rows: Vec<Vec<String>> = all_descs.iter().map(|(field, desc)| {
        let val = defaults.get(*field);
        let type_str = match val {
            Some(serde_json::Value::String(_))  => "string",
            Some(serde_json::Value::Array(_))   => "string[]",
            Some(serde_json::Value::Bool(_))    => "bool",
            Some(serde_json::Value::Number(_))  => "int",
            Some(serde_json::Value::Object(_))  => "object",
            _ if *field == "max_jobs"           => "int",
            _                                   => "?",
        };
        let default_str = if *field == "batch"
            && let Some(p) = plugin
            && !p.supports_batch
        {
            "false".to_string()
        } else {
            tables::opt_json(val)
        };
        let required = tables::yes_no(must_fields.contains(*field));
        let checksum = tables::yes_no(checksum_fields.contains(*field));
        if verbose {
            vec![field.to_string(), type_str.to_string(), default_str, required.to_string(), checksum.to_string(), desc.to_string()]
        } else {
            vec![field.to_string(), type_str.to_string(), default_str, required.to_string(), checksum.to_string()]
        }
    }).collect();

    if verbose {
        tables::print_table(&["Field", "Type", "Default", "Required", "Checksum", "Description"], &rows);
    } else {
        tables::print_table(&["Field", "Type", "Default", "Required", "Checksum"], &rows);
    }
}

/// Show default configuration for a processor (works without rsconstruct.toml).
/// In text mode, prints only the parameters table.
/// In JSON mode (`--json`), prints only the defaults as JSON.
pub fn processor_defconfig(name: &str, verbose: bool) -> Result<()> {
    let json = ProcessorConfig::defconfig_json(name)
        .ok_or_else(|| anyhow::anyhow!("Unknown processor: '{name}'. Run 'rsconstruct processors list' to see available processors."))?;

    if crate::json_output::is_json_mode() {
        println!("{json}");
    } else {
        print_processor_metadata(name, verbose);
    }
    Ok(())
}

impl Builder {
    /// Handle `rsconstruct processor` subcommands
    pub fn processor(&self, ctx: &crate::build_context::BuildContext, action: ProcessorAction, verbose: bool) -> Result<()> {
        let processors = self.create_processors()?;

        let proc_names = sorted_keys(&processors);

        match action {
            ProcessorAction::List { .. } | ProcessorAction::Recommend | ProcessorAction::Types | ProcessorAction::Add { .. }
            | ProcessorAction::Delete { .. } | ProcessorAction::Disable { .. } | ProcessorAction::Enable { .. }
            | ProcessorAction::Search { .. } => unreachable!("handled before Builder is constructed"),
            ProcessorAction::Used => {
                if crate::json_output::is_json_mode() {
                    let entries: Vec<serde_json::Value> = proc_names.iter().map(|name| {
                        let proc = &processors[name.as_str()];
                        serde_json::json!({
                            "name": name,
                            "type": crate::registries::processor::processor_type_of(name.as_str()).as_str(),
                            "detected": proc.auto_detect(&self.file_index),
                            "description": crate::registries::processor::description_of(name.as_str()),
                        })
                    }).collect();
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                } else if verbose {
                    let rows: Vec<Vec<String>> = proc_names.iter().map(|name| {
                        let proc = &processors[name.as_str()];
                        let detected_str = tables::yes_no(proc.auto_detect(&self.file_index));
                        vec![
                            name.to_string(),
                            crate::registries::processor::processor_type_of(name.as_str()).as_str().to_string(),
                            detected_str.to_string(),
                            crate::registries::processor::description_of(name.as_str()).to_string(),
                        ]
                    }).collect();
                    tables::print_table(&["Name", "Type", "Detected", "Description"], &rows);
                } else {
                    let rows: Vec<Vec<String>> = proc_names.iter().map(|name| {
                        let proc = &processors[name.as_str()];
                        let detected_str = tables::yes_no(proc.auto_detect(&self.file_index));
                        vec![
                            name.to_string(),
                            crate::registries::processor::processor_type_of(name.as_str()).as_str().to_string(),
                            detected_str.to_string(),
                        ]
                    }).collect();
                    tables::print_table(&["Name", "Type", "Detected"], &rows);
                }
            }
            ProcessorAction::Config { ref iname, diff } => {
                let names: Vec<&str> = if let Some(n) = iname {
                    if !processors.contains_key(n.as_str()) {
                        bail!("Unknown processor: '{n}'. Run 'rsconstruct processors list' to see available processors.");
                    }
                    vec![n.as_str()]
                } else {
                    proc_names.iter()
                        .map(|s| s.as_str())
                        .collect()
                };

                if crate::json_output::is_json_mode() {
                    let mut map = serde_json::Map::new();
                    for n in &names {
                        let proc = &processors[*n];
                        if let Some(json) = proc.config_json() {
                            let value: serde_json::Value = serde_json::from_str(&json)?;
                            let value = if diff {
                                config_diff(n, &value)
                            } else {
                                value
                            };
                            map.insert(n.to_string(), value);
                        }
                    }
                    println!("{}", serde_json::to_string_pretty(&serde_json::Value::Object(map))?);
                    return Ok(());
                }

                for (i, n) in names.iter().enumerate() {
                    let proc = &processors[*n];
                    if let Some(json) = proc.config_json() {
                        let value: serde_json::Value = serde_json::from_str(&json)?;
                        let value = if diff {
                            config_diff(n, &value)
                        } else {
                            value
                        };
                        if names.len() > 1 {
                            println!("{n}:");
                        }
                        let rows: Vec<Vec<String>> = match &value {
                            serde_json::Value::Object(map) => map.iter()
                                .map(|(k, v)| vec![k.clone(), format_config_value(v)])
                                .collect(),
                            other => vec![vec!["(value)".to_string(), format_config_value(other)]],
                        };
                        crate::tables::print_table(&["Field", "Value"], &rows);
                        // Show the processor's type name for metadata lookup.
                        // Multi-instance names are like "explicit.report" — strip the instance suffix.
                        let type_name = n.split('.').next().unwrap_or(n);
                        println!("\nParameters:");
                        print_processor_metadata(type_name, verbose);
                        if i + 1 < names.len() {
                            println!();
                        }
                    } else if iname.is_some() {
                        println!("Processor '{n}' does not expose configuration.");
                    }
                }
            }
            ProcessorAction::Defconfig { ref pname } => {
                processor_defconfig(pname, verbose)?;
            }
            ProcessorAction::Allowlist => {
                let enabled: Vec<&str> = proc_names.iter()
                    .map(|s| s.as_str())
                    .collect();
                if crate::json_output::is_json_mode() {
                    println!("{}", serde_json::to_string_pretty(&enabled)?);
                } else {
                    println!("enabled = [{}]", enabled.iter()
                        .map(|n| format!("\"{n}\""))
                        .collect::<Vec<_>>()
                        .join(", "));
                }
            }
            ProcessorAction::Names => {
                if crate::json_output::is_json_mode() {
                    println!("{}", serde_json::to_string_pretty(&proc_names)?);
                } else {
                    for name in &proc_names {
                        println!("{name}");
                    }
                }
            }
            ProcessorAction::Graph { format } => {
                let graph = self.build_graph(ctx)?;
                let proc_deps = graph.processor_dependencies();
                let effective_format = if crate::json_output::is_json_mode() {
                    crate::cli::GraphFormat::Json
                } else {
                    format
                };
                match effective_format {
                    crate::cli::GraphFormat::Text => {
                        for (proc, deps) in &proc_deps {
                            if deps.is_empty() {
                                println!("{proc}");
                            } else {
                                println!("{} \u{2192} {}", proc, deps.iter().cloned().collect::<Vec<_>>().join(", "));
                            }
                        }
                    }
                    crate::cli::GraphFormat::Dot => {
                        println!("digraph processors {{");
                        println!("    rankdir=LR;");
                        println!("    node [fontname=\"sans-serif\" shape=box style=filled fillcolor=lightyellow];");
                        for (proc, deps) in &proc_deps {
                            for dep in deps {
                                println!("    \"{proc}\" -> \"{dep}\";");
                            }
                        }
                        println!("}}");
                    }
                    crate::cli::GraphFormat::Mermaid => {
                        println!("graph LR");
                        for (proc, deps) in &proc_deps {
                            for dep in deps {
                                println!("    {proc} --> {dep}");
                            }
                        }
                    }
                    crate::cli::GraphFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&proc_deps)?);
                    }
                    crate::cli::GraphFormat::Svg => {
                        let mut dot = String::from("digraph processors {\n    rankdir=LR;\n    node [fontname=\"sans-serif\" shape=box style=filled fillcolor=lightyellow];\n");
                        for (proc, deps) in &proc_deps {
                            for dep in deps {
                                dot.push_str(&format!("    \"{proc}\" -> \"{dep}\";\n"));
                            }
                        }
                        dot.push_str("}\n");
                        let svg = crate::processors::dot_to_svg(&dot)?;
                        println!("{svg}");
                    }
                }
            }
            ProcessorAction::Files { iname: name, headers } => {
                if let Some(ref n) = name
                    && !processors.contains_key(n.as_str()) {
                        bail!("Unknown processor: '{n}'. Run 'rsconstruct processors list' to see available processors.");
                    }

                let graph = self.build_graph_filtered(ctx, name.as_deref(), false)?;

                let products = graph.products();

                if crate::json_output::is_json_mode() {
                    let entries: Vec<crate::json_output::ProcessorFileEntry> = products.iter()
                        .map(|p| {
                            let proc_type = crate::registries::processor::processor_type_of(p.processor.as_str()).as_str();
                            crate::json_output::ProcessorFileEntry {
                                processor: p.processor.clone(),
                                processor_type: proc_type.to_string(),
                                inputs: p.inputs.iter().map(|i| i.display().to_string()).collect(),
                                outputs: p.outputs.iter().map(|o| o.display().to_string()).collect(),
                            }
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                    return Ok(());
                }

                if products.is_empty() {
                    if let Some(ref n) = name {
                        println!("[{n}] (no files)");
                    } else {
                        println!("No files discovered by any processor.");
                    }
                    return Ok(());
                }

                let mut counts: HashMap<&str, usize> = HashMap::new();
                for p in products {
                    *counts.entry(p.processor.as_str()).or_insert(0) += 1;
                }

                let mut current_processor = "";
                for product in products {
                    if product.processor.as_str() != current_processor {
                        if headers && !current_processor.is_empty() {
                            println!();
                        }
                        current_processor = product.processor.as_str();
                        if headers {
                            let n = counts.get(current_processor).copied().unwrap_or(0);
                            println!("[{}] ({} {})", current_processor, n, if n == 1 { "product" } else { "products" });
                        }
                    }
                    let inputs: Vec<String> = product.inputs.iter()
                        .map(|p| p.display().to_string())
                        .collect();
                    let proc_type = if processors.contains_key(product.processor.as_str()) {
                        Some(crate::registries::processor::processor_type_of(product.processor.as_str()))
                    } else {
                        None
                    };
                    if product.outputs.is_empty() {
                        let label = match proc_type {
                            Some(crate::processors::ProcessorType::Creator) => "(creator)",
                            _ => "(checker)",
                        };
                        println!("{} \u{2192} {}", inputs.join(", "), color::dim(label));
                    } else {
                        let outputs: Vec<String> = product.outputs.iter()
                            .map(|p| p.display().to_string())
                            .collect();
                        println!("{} \u{2192} {}", inputs.join(", "), outputs.join(", "));
                    }
                }
            }
        }

        Ok(())
    }
}
