use std::collections::HashSet;
use anyhow::Result;
use crate::color;
use crate::tables;
use super::{Builder, sorted_keys};

impl Builder {
    /// Run fix mode on all (or filtered) checker processors that have fix capability.
    pub fn fix(&self, ctx: &crate::build_context::BuildContext, processor_filter: Option<&[String]>) -> Result<()> {
        let processors = self.create_processors()?;
        let mut graph = self.build_graph_with_processors(ctx, &processors)?;

        // Filter to only processors with fix capability
        let filter_set: Option<HashSet<&str>> = processor_filter
            .map(|names| names.iter().map(std::string::String::as_str).collect());

        let fixable: Vec<&str> = processors.keys()
            .filter(|name| {
                // Fix capability is either static (plugin can_fix) or
                // config-dependent (e.g. script's fix_command).
                if !crate::registries::processor::can_fix(name.as_str())
                    && !processors[name.as_str()].config_has_fix()
                {
                    return false;
                }
                if let Some(ref filter) = filter_set {
                    return filter.contains(name.as_str());
                }
                true
            })
            .map(std::string::String::as_str)
            .collect();

        if fixable.is_empty() {
            if processor_filter.is_some() {
                println!("{}", color::yellow("No matching processors with fix capability."));
            } else {
                println!("{}", color::yellow("No processors with fix capability found."));
            }
            return Ok(());
        }

        // Filter graph to only fixable products
        let fixable_set: HashSet<&str> = fixable.iter().copied().collect();
        graph.retain_products(|p| fixable_set.contains(p.processor.as_str()));

        let products: Vec<_> = graph.products().to_vec();
        if products.is_empty() {
            println!("{}", color::yellow("No files to fix."));
            return Ok(());
        }

        println!("{}", color::bold(&format!(
            "Fixing {} file(s) using: {}",
            products.len(),
            fixable.join(", "),
        )));

        let mut fixed_count = 0usize;
        let mut error_count = 0usize;

        // Group products by processor for batch execution
        let mut by_processor: std::collections::BTreeMap<&str, Vec<&crate::graph::Product>> = std::collections::BTreeMap::new();
        for product in &products {
            by_processor.entry(&product.processor).or_default().push(product);
        }

        for (proc_name, proc_products) in &by_processor {
            let Some(processor) = processors.get(*proc_name) else { continue };

            if processor.supports_fix_batch() && proc_products.len() > 1 {
                // Batch fix
                let refs: Vec<&crate::graph::Product> = proc_products.to_vec();
                let results = processor.fix_batch(ctx, &refs);
                for result in results {
                    match result {
                        Ok(()) => fixed_count += 1,
                        Err(e) => {
                            eprintln!("{}: {}", color::red(&format!("[{proc_name}] fix error")), e);
                            error_count += 1;
                        }
                    }
                }
            } else {
                // Single-file fix
                for product in proc_products {
                    match processor.fix(ctx, product) {
                        Ok(()) => fixed_count += 1,
                        Err(e) => {
                            eprintln!("{}: {}", color::red(&format!("[{proc_name}] fix error")), e);
                            error_count += 1;
                        }
                    }
                }
            }
        }

        if error_count > 0 {
            println!("{}", color::red(&format!(
                "Fix completed: {fixed_count} fixed, {error_count} errors",
            )));
            anyhow::bail!("Fix failed with {error_count} error(s)");
        }
        println!("{}", color::green(&format!(
            "Fix completed: {fixed_count} file(s) processed",
        )));

        Ok(())
    }

    /// List all fix-capable processors declared in this project.
    pub fn fix_list(&self) -> Result<()> {
        let processors = self.create_processors()?;
        let proc_names = sorted_keys(&processors);

        let fixers: Vec<&String> = proc_names.iter()
            .filter(|name| crate::registries::processor::can_fix(name.as_str()))
            .copied()
            .collect();

        if fixers.is_empty() {
            println!("{}", color::yellow("No fix-capable processors declared in this project."));
            return Ok(());
        }

        if crate::json_output::is_json_mode() {
            let entries: Vec<serde_json::Value> = fixers.iter().map(|name| {
                serde_json::json!({
                    "name": name,
                    "type": crate::registries::processor::processor_type_of(name.as_str()).as_str(),
                    "description": crate::registries::processor::description_of(name.as_str()),
                })
            }).collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
            return Ok(());
        }

        let rows: Vec<Vec<String>> = fixers.iter().map(|name| {
            vec![
                name.to_string(),
                crate::registries::processor::processor_type_of(name.as_str()).as_str().to_string(),
                crate::registries::processor::description_of(name.as_str()).to_string(),
            ]
        }).collect();
        tables::print_table(&["Name", "Type", "Description"], &rows);

        Ok(())
    }
}
