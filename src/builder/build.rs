use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::{Duration, Instant};
use anyhow::{Context, Result};
use crate::cli::{BuildOptions, BuildPhase, DisplayOptions};
use crate::color;
use crate::tables;
use crate::errors;
use crate::executor::{Executor, ExecutorOptions};
use crate::processors::{BuildStats, ProcessorMap, ProcessorType};
use super::{Builder, GraphSnapshot, ProductStatusLabels, StatusPrintOptions, phases_debug, print_graph_stats};

/// Expand `@`-prefixed shortcuts in the processor filter.
///
/// Three categories of shortcuts:
/// - **By type**: `@checkers`, `@generators`, `@creators`, `@lua`
/// - **By tool**: `@python3`, `@node`, etc. — matches processors whose `required_tools()` contains the name
/// - **By processor name**: `@ruff` → `"ruff"` — strips the `@` prefix
fn expand_aliases(filter: &[String], processors: &ProcessorMap) -> Vec<String> {
    let mut expanded = Vec::new();
    for name in filter {
        if let Some(alias) = name.strip_prefix('@') {
            match alias {
                "checkers" => {
                    expanded.extend(
                        processors.iter()
                            .filter(|(name, _)| crate::registries::processor::processor_type_of(name.as_str()) == ProcessorType::Checker)
                            .map(|(n, _)| n.clone())
                    );
                }
                "generators" => {
                    expanded.extend(
                        processors.iter()
                            .filter(|(name, _)| crate::registries::processor::processor_type_of(name.as_str()) == ProcessorType::Generator)
                            .map(|(n, _)| n.clone())
                    );
                }
                "creators" => {
                    expanded.extend(
                        processors.iter()
                            .filter(|(name, _)| crate::registries::processor::processor_type_of(name.as_str()) == ProcessorType::Creator)
                            .map(|(n, _)| n.clone())
                    );
                }
                "lua" => {
                    expanded.extend(
                        processors.iter()
                            .filter(|(name, _)| crate::registries::processor::processor_type_of(name.as_str()) == ProcessorType::Lua)
                            .map(|(n, _)| n.clone())
                    );
                }
                _ => {
                    // Check if it's a tool name
                    let by_tool: Vec<_> = processors.iter()
                        .filter(|(_, p)| p.required_tools().iter().any(|t| t == alias))
                        .map(|(n, _)| n.clone())
                        .collect();
                    if !by_tool.is_empty() {
                        expanded.extend(by_tool);
                    } else if processors.contains_key(alias) {
                        // Fall back to processor name
                        expanded.push(alias.to_string());
                    } else {
                        // Unknown alias — keep original so validation reports the error
                        expanded.push(name.clone());
                    }
                }
            }
        } else {
            expanded.push(name.clone());
        }
    }
    expanded.sort();
    expanded.dedup();
    expanded
}

/// Verify that every enabled processor's required tools exist on PATH.
/// `only` restricts the check to the named processor instances (used by the
/// deferred, product-aware check in shared-config mode); `None` checks every
/// enabled instance that passes `processor_filter`.
fn check_required_tools(
    processors: &ProcessorMap,
    processor_filter: Option<&[String]>,
    only: Option<&std::collections::HashSet<&str>>,
) -> Result<()> {
    let active_names: Vec<&String> = processors.keys()
        .filter(|k| processor_filter.is_none_or(|filter| filter.iter().any(|f| f == *k)))
        .filter(|k| only.is_none_or(|set| set.contains(k.as_str())))
        .filter(|k| processors[*k].scan_config().enabled)
        .collect();
    let mut missing: Vec<(String, Vec<String>)> = Vec::new();
    let mut checked: std::collections::HashSet<String> = std::collections::HashSet::new();
    for name in &active_names {
        for tool in processors[*name].required_tools() {
            if !checked.insert(tool.clone()) {
                continue;
            }
            if which::which(&tool).is_err() {
                let procs: Vec<String> = active_names.iter()
                    .filter(|n| processors[**n].required_tools().contains(&tool))
                    .map(|n| (*n).clone())
                    .collect();
                missing.push((tool, procs));
            }
        }
    }
    if !missing.is_empty() {
        missing.sort_by(|a, b| a.0.cmp(&b.0));
        let mut msg = String::from("Missing required tools:\n");
        for (tool, procs) in &missing {
            let install_hint = crate::processors::tool_install_command(tool)
                .map(|cmd| format!("  install: {cmd}"))
                .unwrap_or_default();
            let _ = writeln!(msg, "  {} (needed by: {}){}", tool, procs.join(", "), install_hint);
        }
        msg.push_str("\nRun `rsconstruct tools install` to install missing tools.");
        return Err(crate::exit_code::RsconstructError::new(
            crate::exit_code::RsconstructExitCode::ToolError,
            msg.trim_end(),
        ).into());
    }
    Ok(())
}

impl Builder {
    /// Execute an incremental build using the dependency graph
    pub fn build(&mut self, ctx: &crate::build_context::BuildContext, opts: &BuildOptions, init_timings: Vec<(String, Duration)>) -> Result<(), anyhow::Error> {
        // CLI override for zspell and aspell auto_add_words
        if opts.auto_add_words {
            for inst in &mut self.config.processor.instances {
                if (inst.type_name == "zspell" || inst.type_name == "aspell")
                    && let Some(table) = inst.config_toml.as_table_mut()
                {
                    table.insert("auto_add_words".to_string(), toml::Value::Boolean(true));
                }
            }
        }

        // CLI override for mtime pre-check
        if opts.no_mtime {
            ctx.set_mtime_check(false);
        }

        // Apply the configured argv-length threshold (build.max_arg_len) so
        // run_checker can read it via ctx.max_arg_len().
        ctx.set_max_arg_len(self.config.build.max_arg_len);

        // Create processors
        let t = Instant::now();
        let processors = self.create_processors()?;
        let create_processors_dur = t.elapsed();

        // Expand @-prefixed shortcuts in both filters, then compute the final
        // set of processors to run:
        //   - if -p is given: start from that set (else all processors)
        //   - if -x is given: remove those from the set
        //   - conflict (same name in both) → error
        //   - unknown names in either filter → error
        let include_expanded = opts.processor_filter.as_ref()
            .map(|f| expand_aliases(f, &processors));
        let exclude_expanded = opts.exclude_filter.as_ref()
            .map(|f| expand_aliases(f, &processors));

        // Validate unknown names in either filter, pooling both error reports.
        let mut unknown: Vec<String> = Vec::new();
        for filter in [&include_expanded, &exclude_expanded].iter().copied().flatten() {
            for name in filter {
                if !processors.contains_key(name) {
                    unknown.push(name.clone());
                }
            }
        }
        if !unknown.is_empty() {
            let mut available: Vec<&String> = processors.keys().collect();
            available.sort();
            return Err(crate::exit_code::RsconstructError::new(
                crate::exit_code::RsconstructExitCode::ConfigError,
                format!("Unknown processor(s): {unknown:?}. Available: {available:?}"),
            ).into());
        }

        // Reject overlap between -p and -x: contradictory intent should fail
        // loudly rather than silently picking one side.
        if let (Some(inc), Some(exc)) = (&include_expanded, &exclude_expanded) {
            let conflicts: Vec<&String> = exc.iter().filter(|e| inc.contains(e)).collect();
            if !conflicts.is_empty() {
                return Err(crate::exit_code::RsconstructError::new(
                    crate::exit_code::RsconstructExitCode::ConfigError,
                    format!("Processor(s) {conflicts:?} appear in both -p and -x"),
                ).into());
            }
        }

        // Build the final filter. When -x is the only filter, synthesize an
        // include list from "all processors minus excludes" so downstream code
        // can treat it as a regular allow-list.
        let expanded_filter: Option<Vec<String>> = match (include_expanded, exclude_expanded) {
            (Some(inc), Some(exc)) => Some(inc.into_iter().filter(|n| !exc.contains(n)).collect()),
            (Some(inc), None) => Some(inc),
            (None, Some(exc)) => Some(
                processors.keys().filter(|n| !exc.contains(n)).cloned().collect()
            ),
            (None, None) => None,
        };
        let processor_filter = expanded_filter.as_deref();

        // Pre-flight: verify all required tools are available for declared processors.
        // Disabled instances (`enabled = false`) are exempt — disabling a
        // processor exists precisely to keep its stanza while its tool is absent.
        // In shared-config mode (skip_missing_src_dirs), the check is deferred
        // until after the graph is built and restricted to processors that
        // actually discovered products — a processor with no work in this
        // repo doesn't need its tool installed.
        let deferred_tool_check = self.config.build.skip_missing_src_dirs;
        if !deferred_tool_check {
            check_required_tools(&processors, processor_filter, None)?;
        }

        // Check for config changes and display diffs
        self.detect_config_changes(&processors, opts.show_all_config_changes);

        // Build the dependency graph (may stop early based on stop_after)
        let (mut graph, mut phase_timings) = self.build_graph_with_processors_and_phase(ctx, &processors, opts.stop_after, processor_filter, opts.verbose)?;

        if deferred_tool_check {
            let with_products: std::collections::HashSet<&str> = graph.products().iter()
                .map(|p| p.processor.as_str())
                .collect();
            check_required_tools(&processors, processor_filter, Some(&with_products))?;
        }

        // Filter by target patterns if specified
        if let Some(ref targets) = opts.targets {
            graph.filter_by_targets(targets)?;
        }

        // Prepend create_processors and init timings
        phase_timings.insert(0, ("create_processors".to_string(), create_processors_dur));
        for (i, timing) in init_timings.into_iter().enumerate() {
            phase_timings.insert(i, timing);
        }

        // If we stopped early (before classify), we're done
        if opts.stop_after != BuildPhase::Build && opts.stop_after != BuildPhase::Classify {
            if crate::json_output::human_output_enabled() {
                println!("Stopped after {:?} phase.", opts.stop_after);
            }
            return Ok(());
        }

        // Phase: Classify products (skip/restore/build). Printed in two lines,
        // matching the dep-scan phase style:
        //   - forward-looking total before classify runs (this is the checksum
        //     pass, which is the expensive work for large graphs)
        //   - post-classify breakdown showing what will actually be built
        if phases_debug() {
            eprintln!("{}", color::dim("  Phase: classify"));
        }
        let t = Instant::now();
        let order = graph.topological_sort()?;
        if crate::json_output::human_output_enabled() {
            println!("[build] {} products to check for updates", order.len());
        }
        let policy = crate::executor::IncrementalPolicy;
        let classification =
            crate::executor::classify_products(ctx, &policy, &graph, &order, &self.object_store, opts.force);
        phase_timings.push(("classify".to_string(), t.elapsed()));
        if crate::json_output::human_output_enabled() {
            println!("[build] {} to build, {} to restore ({} up-to-date)",
                classification.build_count, classification.restore_count, classification.skip_count);
        }
        print_graph_stats(GraphSnapshot::AfterClassify, &graph);

        if opts.stop_after == BuildPhase::Classify {
            return Ok(());
        }

        // Unlink outputs of every product we intend to execute. Doing this up
        // front (rather than per-product right before execute) means a stale
        // downstream output cannot survive on disk if its upstream fails: the
        // downstream's outputs are already gone before execution starts.
        crate::executor::unlink_pending_outputs(&graph, &self.object_store, &classification)?;

        // Create executor with parallelism from command line, env var, or config
        let parallel = opts.jobs
            .or_else(|| std::env::var("RSCONSTRUCT_THREADS").ok().and_then(|v| v.parse().ok()))
            .unwrap_or(self.config.build.parallel);
        let effective_parallel = if parallel == 0 {
            std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
        } else {
            parallel
        };
        if crate::json_output::human_output_enabled() {
            println!("[rsconstruct] using {effective_parallel} threads");
        }
        // CLI overrides config for batch_size (CLI -1 maps to None = disable)
        let batch_size = opts.batch_size.unwrap_or(Some(self.config.build.batch_size));
        let executor = Executor::new(&processors, ctx, &policy, ExecutorOptions {
            parallel: effective_parallel,
            verbose: opts.verbose,
            display_opts: opts.display_opts,
            batch_size,
            explain: opts.explain,
            retry: opts.retry,
        });

        // Execute the build (enable timings collection if trace output is requested)
        let t = Instant::now();
        let collect_timings = opts.timings || opts.trace.is_some();
        let result = executor.execute(&graph, &self.object_store, opts.force, collect_timings, opts.keep_going, &classification);
        let build_dur = t.elapsed();
        print_graph_stats(GraphSnapshot::AfterExecute, &graph);

        // Exit if interrupted
        if ctx.is_interrupted() {
            return Err(crate::exit_code::RsconstructError::new(
                crate::exit_code::RsconstructExitCode::Interrupted,
                "Build interrupted",
            ).into());
        }

        let mut stats = result?;

        // Add phase timings to stats
        phase_timings.push(("build".to_string(), build_dur));
        stats.phase_timings = phase_timings;

        // Print summary
        stats.print_summary(opts.summary, opts.timings);

        // Write Chrome trace file if requested
        if let Some(ref trace_path) = opts.trace {
            write_trace_file(trace_path, &stats)?;
        }

        // Return error if there were failures in keep-going mode
        if stats.failed_count > 0 {
            return Err(crate::exit_code::RsconstructError::new(
                crate::exit_code::RsconstructExitCode::BuildError,
                format!("Build completed with {} error(s)", stats.failed_count),
            ).into());
        }

        Ok(())
    }

    /// Show what would happen without executing anything
    pub fn dry_run(&self, ctx: &crate::build_context::BuildContext, force: bool, explain: bool) -> anyhow::Result<()> {
        let processors = self.create_processors()?;
        let graph = self.build_graph_with_processors(ctx, &processors)?;

        let order = graph.topological_sort()?;
        if order.is_empty() {
            println!("No products discovered.");
            return Ok(());
        }

        let products: Vec<_> = order.iter()
            .map(|&id| graph.get_product(id).expect(errors::INVALID_PRODUCT_ID))
            .collect();

        let labels = ProductStatusLabels {
            current: (color::dim("SKIP"), "skip"),
            restorable: (color::cyan("RESTORE"), "restore"),
            stale: (color::yellow("BUILD"), "build"),
            new: (color::yellow("BUILD"), "build-new"),
        };

        self.print_product_status(ctx, &products, &StatusPrintOptions {
            force, labels: &labels, explain,
            display_opts: DisplayOptions::default(), verbose: true,
            all_processor_names: &[],
            native_processors: &std::collections::HashSet::new(),
        });
        Ok(())
    }

    /// Show the status of each product in the build graph
    pub fn status(&self, ctx: &crate::build_context::BuildContext, verbose: bool, breakdown: bool) -> anyhow::Result<()> {
        let processors = self.create_processors()?;
        let graph = self.build_graph_with_processors(ctx, &processors)?;

        let products: Vec<&_> = graph.products().iter().collect();
        if products.is_empty() && processors.is_empty() {
            println!("No products discovered.");
            return Ok(());
        }

        let labels = ProductStatusLabels {
            current: (color::green("UP-TO-DATE"), "up-to-date"),
            restorable: (color::cyan("RESTORABLE"), "restorable"),
            stale: (color::yellow("STALE"), "stale"),
            new: (color::magenta("NEW"), "new"),
        };

        // Collect all processor names so we also show processors with 0 files
        let all_proc_names: Vec<&str> = super::sorted_keys(&processors)
            .into_iter()
            .map(std::string::String::as_str)
            .collect();
        let native_set: std::collections::HashSet<&str> = processors.iter()
            .filter(|(name, _)| crate::registries::processor::is_native(name.as_str()))
            .map(|(name, _)| name.as_str())
            .collect();
        self.print_product_status(ctx, &products, &StatusPrintOptions {
            force: false, labels: &labels, explain: false,
            display_opts: DisplayOptions::default(), verbose,
            all_processor_names: &all_proc_names,
            native_processors: &native_set,
        });

        if breakdown {
            // Collect unique source files per processor, then count by extension
            let mut per_processor_files: BTreeMap<&str, std::collections::HashSet<&std::path::Path>> = BTreeMap::new();
            // Seed with all processors so 0-file processors are shown
            for name in &all_proc_names {
                per_processor_files.entry(name).or_default();
            }
            for product in &products {
                let files = per_processor_files.entry(&product.processor).or_default();
                for input in &product.inputs {
                    files.insert(input.as_path());
                }
            }
            let mut per_processor: BTreeMap<&str, BTreeMap<String, usize>> = BTreeMap::new();
            for (proc_name, files) in &per_processor_files {
                let ext_counts = per_processor.entry(proc_name).or_default();
                for path in files {
                    let ext = path.extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("(no ext)");
                    *ext_counts.entry(ext.to_string()).or_default() += 1;
                }
            }
            println!();
            println!("{}:", color::bold("Source files by processor"));
            let rows: Vec<Vec<String>> = per_processor.iter().map(|(proc_name, ext_counts)| {
                let total: usize = ext_counts.values().sum();
                let breakdown_str = if total == 0 {
                    String::new()
                } else {
                    ext_counts.iter()
                        .map(|(ext, count)| format!("{count} .{ext}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                vec![proc_name.to_string(), format!("{} files", total), breakdown_str]
            }).collect();
            tables::print_table(&["Processor", "Files", "Breakdown"], &rows);
        }

        Ok(())
    }

    /// Show source file counts by extension.
    pub fn info_source(&self, ctx: &crate::build_context::BuildContext) -> anyhow::Result<()> {
        let processors = self.create_processors()?;
        let graph = self.build_graph_with_processors(ctx, &processors)?;

        let products = graph.products();
        let mut all_inputs: std::collections::HashSet<&std::path::Path> = std::collections::HashSet::new();
        for product in products {
            for input in &product.inputs {
                all_inputs.insert(input.as_path());
            }
        }

        let mut ext_counts: BTreeMap<String, usize> = BTreeMap::new();
        for path in &all_inputs {
            let ext = path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("(no ext)");
            *ext_counts.entry(ext.to_string()).or_default() += 1;
        }

        if crate::json_output::is_json_mode() {
            let json = serde_json::json!({
                "total": all_inputs.len(),
                "by_extension": ext_counts,
            });
            println!("{}", serde_json::to_string_pretty(&json).expect(crate::errors::JSON_SERIALIZE));
        } else {
            println!("{}: {}", color::bold("Total source files"), all_inputs.len());
            let rows: Vec<Vec<String>> = ext_counts.iter()
                .map(|(ext, count)| vec![format!(".{}", ext), count.to_string()])
                .collect();
            tables::print_table(&["Extension", "Count"], &rows);
        }
        Ok(())
    }

    /// Classify and print the status of each product, with per-processor and total summary.
    /// When `verbose` is false, only the per-processor and total summary lines are printed.
    pub(super) fn print_product_status(
        &self,
        ctx: &crate::build_context::BuildContext,
        products: &[&crate::graph::Product],
        opts: &StatusPrintOptions<'_>,
    ) {
        use crate::object_store::ExplainAction;

        const NUM_STATES: usize = 4; // current, restorable, stale, new
        let mut counts = [0usize; NUM_STATES];
        let mut per_processor: BTreeMap<&str, [usize; NUM_STATES]> = BTreeMap::new();
        // Seed with all processor names so processors with 0 products are shown
        for name in opts.all_processor_names {
            per_processor.entry(name).or_default();
        }

        let status_labels = [
            &opts.labels.current.0,
            &opts.labels.restorable.0,
            &opts.labels.stale.0,
            &opts.labels.new.0,
        ];

        for product in products {
            let display = product.display(opts.display_opts);

            let Ok(input_checksum) = crate::checksum::combined_input_checksum(ctx, &product.inputs) else {
                // Can't compute checksum (an input is unreadable) — without a
                // descriptor key, stale and new are indistinguishable; report
                // as new.
                let idx = 3;
                if opts.verbose {
                    println!("{} [{}] {}", status_labels[idx], product.processor, display);
                }
                counts[idx] += 1;
                per_processor.entry(&product.processor).or_default()[idx] += 1;
                continue;
            };

            // Same classification with and without --explain — the flag only
            // adds the reason text.
            let desc_key = product.descriptor_key(&input_checksum);
            let action = self.object_store.explain_descriptor(ctx, &desc_key, &product.outputs, opts.force);
            let reason = if opts.explain { format!(" ({action})") } else { String::new() };
            let status_idx = match action {
                ExplainAction::Skip => 0,
                ExplainAction::Restore(_) => 1,
                ExplainAction::Rebuild(crate::object_store::RebuildReason::NoCacheEntry) => 3,
                ExplainAction::Rebuild(_) => 2,
            };

            if opts.verbose {
                println!("{} [{}] {}{}", status_labels[status_idx], product.processor, display, reason);
            }

            counts[status_idx] += 1;
            per_processor.entry(&product.processor).or_default()[status_idx] += 1;
        }

        if crate::json_output::is_json_mode() {
            let processors_json: Vec<serde_json::Value> = per_processor.iter().map(|(name, pc)| {
                serde_json::json!({
                    "name": name,
                    "up_to_date": pc[0],
                    "restorable": pc[1],
                    "stale": pc[2],
                    "new": pc[3],
                    "total": pc[0] + pc[1] + pc[2] + pc[3],
                    "native": opts.native_processors.contains(name),
                })
            }).collect();
            let json = serde_json::json!({
                "processors": processors_json,
                "totals": {
                    "up_to_date": counts[0],
                    "restorable": counts[1],
                    "stale": counts[2],
                    "new": counts[3],
                    "total": counts[0] + counts[1] + counts[2] + counts[3],
                },
            });
            println!("{}", serde_json::to_string_pretty(&json).expect(crate::errors::JSON_SERIALIZE));
            return;
        }

        // Per-processor table
        let col_labels = [
            opts.labels.current.1,
            opts.labels.restorable.1,
            opts.labels.stale.1,
            opts.labels.new.1,
        ];

        let rows: Vec<Vec<String>> = per_processor.iter().map(|(name, pc)| {
            let native = crate::tables::yes_no(opts.native_processors.contains(name));
            vec![
                name.to_string(),
                pc[0].to_string(), pc[1].to_string(), pc[2].to_string(), pc[3].to_string(),
                native.to_string(),
            ]
        }).collect();
        let total = vec![
            "Total".to_string(),
            counts[0].to_string(), counts[1].to_string(), counts[2].to_string(), counts[3].to_string(),
            String::new(),
        ];
        tables::print_table_with_total(
            &["Processor", col_labels[0], col_labels[1], col_labels[2], col_labels[3], "native"],
            &rows,
            &total,
        );
    }
}


/// Write a Chrome trace format JSON file from build statistics.
/// The file can be opened in <chrome://tracing> or <https://ui.perfetto.dev>
fn write_trace_file(path: &str, stats: &BuildStats) -> Result<()> {
    let mut events: Vec<serde_json::Value> = Vec::new();
    let mut tid_counter = 1u64;

    // Phase timings on tid=0
    let mut phase_offset_us = 0i64;
    for (name, dur) in &stats.phase_timings {
        let dur_us = dur.as_micros() as i64;
        events.push(serde_json::json!({
            "name": name,
            "cat": "phase",
            "ph": "X",
            "ts": phase_offset_us,
            "dur": dur_us,
            "pid": 1,
            "tid": 0
        }));
        phase_offset_us += dur_us;
    }

    // Product timings
    for cat in &stats.categories {
        for pt in &cat.product_timings {
            let dur_us = pt.duration.as_micros() as i64;
            let ts_us = pt.start_offset
                .map_or(0, |off| off.as_micros() as i64);
            let name = format!("{}:{}", pt.processor, pt.display);
            events.push(serde_json::json!({
                "name": name,
                "cat": "build",
                "ph": "X",
                "ts": ts_us,
                "dur": dur_us,
                "pid": 1,
                "tid": tid_counter
            }));
            tid_counter += 1;
        }
    }

    let trace = serde_json::json!({ "traceEvents": events });
    let trace_json = serde_json::to_string_pretty(&trace)?;
    std::fs::write(path, trace_json)
        .with_context(|| format!("Failed to write trace file: {path}"))?;
    if crate::json_output::human_output_enabled() {
        println!("Wrote trace to {}", color::bold(path));
    }
    Ok(())
}
