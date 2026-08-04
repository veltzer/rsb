use std::fs;
use std::path::PathBuf;
use anyhow::{Context, Result, bail};
use crate::color;
use crate::tables;
use crate::deps_cache::DepsCache;
use super::{Builder, sorted_keys};

/// List all available dependency analyzers (works without rsconstruct.toml).
pub fn list_analyzers(verbose: bool) {
    use crate::registries as registry;

    let mut plugins: Vec<_> = registry::all_analyzer_plugins().collect();
    plugins.sort_by_key(|p| p.name);

    if crate::json_output::is_json_mode() {
        #[derive(serde::Serialize)]
        struct Entry { name: &'static str, native: bool, description: &'static str }
        let entries: Vec<Entry> = plugins.iter()
            .map(|p| Entry { name: p.name, native: p.is_native, description: p.description })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries).expect("JSON serialize"));
        return;
    }

    if verbose {
        let rows: Vec<Vec<String>> = plugins.iter().map(|plugin| {
            let native_tag = if plugin.is_native { "native" } else { "external" };
            vec![plugin.name.to_string(), native_tag.to_string(), plugin.description.to_string()]
        }).collect();
        tables::print_table(&["Name", "Native", "Description"], &rows);
    } else {
        let rows: Vec<Vec<String>> = plugins.iter().map(|plugin| {
            let native_tag = if plugin.is_native { "native" } else { "external" };
            vec![plugin.name.to_string(), native_tag.to_string()]
        }).collect();
        tables::print_table(&["Name", "Native"], &rows);
    }
}

/// Show default analyzer configuration (works without rsconstruct.toml).
pub fn analyzer_defconfig(name: Option<&str>) -> Result<()> {
    use crate::registries as registry;

    let names: Vec<String> = if let Some(name) = name {
        if registry::find_analyzer_plugin(name).is_none() {
            anyhow::bail!("Unknown analyzer '{name}'. Run 'rsconstruct analyzers list' to see available analyzers.");
        }
        vec![name.to_string()]
    } else {
        registry::all_analyzer_names().iter().map(std::string::ToString::to_string).collect()
    };

    if crate::json_output::is_json_mode() {
        #[derive(serde::Serialize)]
        struct Entry { name: String, config: serde_json::Value }
        let entries: Vec<Entry> = names.iter().map(|n| {
            let plugin = registry::find_analyzer_plugin(n).expect("checked above");
            let config = match (plugin.defconfig_toml)() {
                Some(toml_str) => toml::from_str::<serde_json::Value>(&toml_str).unwrap_or(serde_json::Value::Null),
                None => serde_json::Value::Null,
            };
            Entry { name: n.clone(), config }
        }).collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    for (i, n) in names.iter().enumerate() {
        if i > 0 { println!(); }
        let plugin = registry::find_analyzer_plugin(n).expect("checked above");
        println!("[analyzer.{n}]");
        match (plugin.defconfig_toml)() {
            Some(toml_str) => print_config_table(&toml_str)?,
            None => println!("(no configuration options)"),
        }
    }
    Ok(())
}

/// Parse a TOML string and print its fields as a table (Field, Type, Default).
fn print_config_table(toml_str: &str) -> Result<()> {
    let value: toml::Value = toml::from_str(toml_str).context("Failed to parse analyzer defconfig TOML")?;
    let table = value.as_table().context("Analyzer defconfig is not a TOML table")?;

    let rows: Vec<Vec<String>> = table.iter().map(|(key, val)| {
        let type_str = match val {
            toml::Value::String(_)  => "string",
            toml::Value::Integer(_) => "int",
            toml::Value::Float(_)   => "float",
            toml::Value::Boolean(_) => "bool",
            toml::Value::Array(_)   => "string[]",
            toml::Value::Table(_)   => "table",
            toml::Value::Datetime(_) => "datetime",
        };
        let default_str = match val {
            toml::Value::String(s) => format!("\"{s}\""),
            toml::Value::Array(a) if a.is_empty() => "[]".to_string(),
            _ => val.to_string(),
        };
        vec![key.clone(), type_str.to_string(), default_str]
    }).collect();
    tables::print_table(&["Field", "Type", "Default"], &rows);
    Ok(())
}

/// Emit the analyzers `show` results as JSON.
fn print_deps_json(entries: &[(PathBuf, Vec<PathBuf>, String)]) -> Result<()> {
    let rows: Vec<serde_json::Value> = entries.iter().map(|(source, deps, analyzer)| {
        serde_json::json!({
            "source": source.display().to_string(),
            "analyzer": analyzer,
            "dependencies": deps.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        })
    }).collect();
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}

/// Print per-analyzer dependency stats with a total line.
/// `declared` is the list of analyzer names declared in rsconstruct.toml; any
/// declared analyzer missing from `stats` is shown as a zero row so users can
/// spot silent no-ops.
fn print_deps_stats(
    stats: &std::collections::HashMap<String, (usize, usize)>,
    declared: &[String],
) -> Result<()> {
    let mut all: std::collections::BTreeMap<String, (usize, usize)> = std::collections::BTreeMap::new();
    for name in declared {
        all.insert(name.clone(), (0, 0));
    }
    for (name, &v) in stats {
        all.insert(name.clone(), v);
    }

    let mut total_files = 0;
    let mut total_deps = 0;

    if crate::json_output::is_json_mode() {
        let mut analyzers: Vec<serde_json::Value> = Vec::new();
        for (name, (files, deps)) in &all {
            total_files += files;
            total_deps += deps;
            analyzers.push(serde_json::json!({
                "analyzer": name,
                "files": files,
                "dependencies": deps,
            }));
        }
        let out = serde_json::json!({
            "analyzers": analyzers,
            "total": { "files": total_files, "dependencies": total_deps },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    for (name, (files, deps)) in &all {
        total_files += files;
        total_deps += deps;
        rows.push(vec![name.clone(), files.to_string(), deps.to_string()]);
    }
    let total = vec!["Total".to_string(), total_files.to_string(), total_deps.to_string()];
    tables::print_table_with_total(&["Analyzer", "Files", "Dependencies"], &rows, &total);
    Ok(())
}

impl Builder {
    /// Handle `rsconstruct analyzers` subcommands
    pub fn analyzers(&self, ctx: &crate::build_context::BuildContext, action: crate::cli::AnalyzersAction, verbose: bool) -> Result<()> {
        use crate::cli::AnalyzersAction;

        match action {
            AnalyzersAction::List | AnalyzersAction::Defconfig { .. } | AnalyzersAction::Add { .. }
            | AnalyzersAction::Delete { .. } | AnalyzersAction::Disable { .. } | AnalyzersAction::Enable { .. } => unreachable!("handled in main.rs"),
            AnalyzersAction::Used => {
                let analyzers = self.create_analyzers(false)?;
                if crate::json_output::is_json_mode() {
                    let entries: Vec<serde_json::Value> = sorted_keys(&analyzers).into_iter().map(|name| {
                        let analyzer = &analyzers[name];
                        serde_json::json!({
                            "name": name,
                            "detected": analyzer.auto_detect(&self.file_index),
                            "description": analyzer.description(),
                        })
                    }).collect();
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                } else if verbose {
                    let rows: Vec<Vec<String>> = sorted_keys(&analyzers).into_iter().map(|name| {
                        let analyzer = &analyzers[name];
                        let detected = tables::yes_no(analyzer.auto_detect(&self.file_index));
                        vec![name.clone(), detected.to_string(), analyzer.description().to_string()]
                    }).collect();
                    tables::print_table(&["Name", "Detected", "Description"], &rows);
                } else {
                    let rows: Vec<Vec<String>> = sorted_keys(&analyzers).into_iter().map(|name| {
                        let analyzer = &analyzers[name];
                        let detected = tables::yes_no(analyzer.auto_detect(&self.file_index));
                        vec![name.clone(), detected.to_string()]
                    }).collect();
                    tables::print_table(&["Name", "Detected"], &rows);
                }
            }
            AnalyzersAction::Build => {
                let processors = self.create_processors()?;
                let mut graph = crate::graph::BuildGraph::new();

                // Phase 1: Discover products (fixed-point loop for cross-processor deps)
                let active: Vec<String> = sorted_keys(&processors).into_iter()
                    .filter(|name| self.is_processor_active(name, processors[*name].as_ref()))
                    .cloned()
                    .collect();
                self.discover_products(&mut graph, &processors, &active, false)?;

                let product_count = graph.products().len();
                if product_count == 0 {
                    println!("No products discovered.");
                    return Ok(());
                }

                // Phase 2: Run dependency analyzers
                self.run_analyzers(ctx, &mut graph, true)?;

                // Match the main pipeline: resolve edges and validate, so
                // `analyzers build` and `build` agree on whether the project
                // is well-formed. This used to skip both and report success
                // on configs that `build` rejects with a validation error.
                graph.resolve_dependencies();
                let validation_errors = graph.validate(&self.config.graph);
                if !validation_errors.is_empty() {
                    anyhow::bail!("Graph validation failed:\n{}", validation_errors.join("\n"));
                }

                // Show summary from cache
                let deps_cache = DepsCache::open()?;
                let stats = deps_cache.stats_by_analyzer();
                let declared: Vec<String> = self.config.analyzer.instances.iter()
                    .map(|i| i.instance_name.clone()).collect();
                if !stats.is_empty() || !declared.is_empty() {
                    print_deps_stats(&stats, &declared)?;
                }
            }
            AnalyzersAction::Config { iname } => {
                let instances: Vec<&crate::config::AnalyzerInstance> = if let Some(ref n) = iname {
                    let inst = self.config.analyzer.instances.iter().find(|i| &i.instance_name == n)
                        .ok_or_else(|| anyhow::anyhow!("Analyzer instance '{n}' is not declared in rsconstruct.toml"))?;
                    vec![inst]
                } else {
                    self.config.analyzer.instances.iter().collect()
                };

                if crate::json_output::is_json_mode() {
                    let mut map = serde_json::Map::new();
                    for inst in &instances {
                        let value: serde_json::Value = inst.config_toml.clone().try_into()
                            .unwrap_or(serde_json::Value::Null);
                        map.insert(inst.instance_name.clone(), value);
                    }
                    println!("{}", serde_json::to_string_pretty(&serde_json::Value::Object(map))?);
                } else if instances.is_empty() {
                    println!("No analyzers declared in rsconstruct.toml. Add `[analyzer.NAME]` sections to enable.");
                } else {
                    for (i, inst) in instances.iter().enumerate() {
                        if i > 0 { println!(); }
                        let toml_str = crate::errors::ctx(toml::to_string_pretty(&inst.config_toml), &format!("Failed to serialize {} analyzer config", inst.instance_name))?;
                        println!("[analyzer.{}]", inst.instance_name);
                        print!("{toml_str}");
                    }
                }
            }
            AnalyzersAction::Clean { analyzer } => {
                if let Some(analyzer_name) = analyzer {
                    // Clear only entries from specific analyzer
                    let deps_cache = DepsCache::open()?;
                    let removed = deps_cache.remove_by_analyzer(&analyzer_name)
                        .with_context(|| format!("Failed to remove deps for analyzer '{analyzer_name}'"))?;
                    if removed > 0 {
                        println!("Removed {removed} entries from '{analyzer_name}' analyzer.");
                    } else {
                        println!("No entries found for '{analyzer_name}' analyzer.");
                    }
                } else {
                    // Clear the entire dependency cache
                    let deps_file = PathBuf::from(".rsconstruct/deps.redb");
                    if deps_file.exists() {
                        fs::remove_file(&deps_file)
                            .with_context(|| format!("Failed to remove dependency cache: {}", deps_file.display()))?;
                        println!("Dependency cache cleared.");
                    } else {
                        println!("Dependency cache is already empty.");
                    }
                }
            }
            AnalyzersAction::Stats => {
                // Show statistics by analyzer
                let deps_cache = DepsCache::open()?;
                let stats = deps_cache.stats_by_analyzer();
                let declared: Vec<String> = self.config.analyzer.instances.iter()
                    .map(|i| i.instance_name.clone()).collect();
                if stats.is_empty() && declared.is_empty() {
                    if crate::json_output::is_json_mode() {
                        let out = serde_json::json!({
                            "analyzers": serde_json::Value::Array(Vec::new()),
                            "total": { "files": 0, "dependencies": 0 },
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Dependency cache is empty. Run a build first.");
                    }
                    return Ok(());
                }
                print_deps_stats(&stats, &declared)?;
            }
            AnalyzersAction::Show { filter } => {
                use crate::cli::AnalyzersShowFilter;
                let deps_cache = DepsCache::open()?;

                let json_mode = crate::json_output::is_json_mode();
                match filter {
                    AnalyzersShowFilter::All => {
                        let mut entries: Vec<_> = deps_cache.list_all();
                        entries.sort_by(|a, b| a.0.cmp(&b.0));
                        if json_mode {
                            print_deps_json(&entries)?;
                        } else if entries.is_empty() {
                            println!("Dependency cache is empty. Run a build first.");
                        } else {
                            for (source, deps, analyzer) in entries {
                                Self::print_deps(&source, &deps, &analyzer);
                            }
                        }
                    }
                    AnalyzersShowFilter::Files { files, hash_pieces } => {
                        // Query specific files. One path can have multiple
                        // entries — one per analyzer that scanned it.
                        let mut collected: Vec<ShowFileEntry> = Vec::new();
                        let mut found_any = false;
                        // Instantiate analyzers up front only when we need to
                        // recompute hash pieces — otherwise stay in pure
                        // deps-cache-read mode like before.
                        let analyzers = if hash_pieces {
                            Some(self.create_analyzers(false)?)
                        } else {
                            None
                        };
                        for file_arg in &files {
                            let file_path = PathBuf::from(file_arg);
                            let entries = deps_cache.get_raw_for_path(&file_path);
                            if entries.is_empty() {
                                if !json_mode {
                                    eprintln!("{}: '{}' not in dependency cache", color::yellow("Warning"), file_arg);
                                }
                            } else {
                                found_any = true;
                                for (deps, analyzer) in entries {
                                    let pieces = if let Some(ref a) = analyzers {
                                        a.get(&analyzer)
                                            .and_then(|inst| inst.scan_hash_pieces(ctx, &file_path).ok().flatten())
                                    } else {
                                        None
                                    };
                                    if json_mode {
                                        collected.push((file_path.clone(), deps, analyzer, pieces));
                                    } else {
                                        Self::print_deps(&file_path, &deps, &analyzer);
                                        if hash_pieces {
                                            Self::print_hash_pieces(pieces.as_deref());
                                        }
                                    }
                                }
                            }
                        }
                        if !found_any {
                            bail!("No cached dependencies found for the specified files");
                        }
                        if json_mode {
                            print_deps_json_with_pieces(&collected, hash_pieces)?;
                        }
                    }
                    AnalyzersShowFilter::Analyzers { analyzers } => {
                        let mut entries: Vec<_> = deps_cache.list_by_analyzers(&analyzers);
                        entries.sort_by(|a, b| a.0.cmp(&b.0));
                        if json_mode {
                            print_deps_json(&entries)?;
                        } else if entries.is_empty() {
                            println!("No cached dependencies found for analyzers: {}", analyzers.join(", "));
                        } else {
                            for (source, deps, analyzer) in entries {
                                Self::print_deps(&source, &deps, &analyzer);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Print dependencies for a source file
    fn print_deps(source: &std::path::Path, deps: &[PathBuf], analyzer: &str) {
        let analyzer_tag = if analyzer.is_empty() {
            String::new()
        } else {
            format!(" {}", color::dim(&format!("[{analyzer}]")))
        };
        if deps.is_empty() {
            println!("{}:{} {}", source.display(), analyzer_tag, color::dim("(no dependencies)"));
        } else {
            println!("{}:{}", source.display(), analyzer_tag);
            for dep in deps {
                println!("  {}", dep.display());
            }
        }
    }

    /// Print structured hash pieces for a source file. Each piece is in the
    /// form `kind:body`, where `body` may be multi-line for resolved file
    /// lists. None means the analyzer doesn't contribute hash pieces; an
    /// empty Vec means it does but the source had nothing to track.
    fn print_hash_pieces(pieces: Option<&[String]>) {
        let label = color::dim("hash pieces:");
        match pieces {
            None => println!("  {} {}", label, color::dim("(analyzer does not contribute)")),
            Some([]) => println!("  {} {}", label, color::dim("(none)")),
            Some(p) => {
                println!("  {label}");
                for piece in p {
                    let (kind, body) = piece.split_once(':').unwrap_or((piece.as_str(), ""));
                    if body.contains('\n') {
                        println!("    {}", color::cyan(kind));
                        for line in body.lines() {
                            println!("      {line}");
                        }
                    } else {
                        println!("    {} {}", color::cyan(kind), body);
                    }
                }
            }
        }
    }
}

/// One row for `analyzers show files`: source path, dependency list, the
/// analyzer that produced it, and optionally the live-recomputed hash pieces
/// (None when --hash-pieces was not passed OR the analyzer doesn't contribute).
type ShowFileEntry = (PathBuf, Vec<PathBuf>, String, Option<Vec<String>>);

/// JSON printer for `analyzers show files` that may include hash pieces.
/// Always emits the `dependencies` field; emits `hash_pieces` only when the
/// `--hash-pieces` flag is set, so the JSON shape isn't different by accident
/// for callers that don't ask for it. A null `hash_pieces` value means the
/// analyzer does not contribute pieces; an empty array means it does but the
/// source had nothing to track.
fn print_deps_json_with_pieces(
    entries: &[ShowFileEntry],
    include_hash_pieces: bool,
) -> Result<()> {
    let rows: Vec<serde_json::Value> = entries.iter().map(|(source, deps, analyzer, pieces)| {
        let mut obj = serde_json::Map::new();
        obj.insert("source".into(), serde_json::Value::String(source.display().to_string()));
        obj.insert("analyzer".into(), serde_json::Value::String(analyzer.clone()));
        obj.insert(
            "dependencies".into(),
            serde_json::Value::Array(
                deps.iter().map(|d| serde_json::Value::String(d.display().to_string())).collect()
            ),
        );
        if include_hash_pieces {
            obj.insert(
                "hash_pieces".into(),
                match pieces {
                    None => serde_json::Value::Null,
                    Some(p) => serde_json::Value::Array(
                        p.iter().map(|s| serde_json::Value::String(s.clone())).collect()
                    ),
                },
            );
        }
        serde_json::Value::Object(obj)
    }).collect();
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}
