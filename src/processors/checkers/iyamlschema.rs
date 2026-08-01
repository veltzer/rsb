use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;

use crate::config::IyamlschemaConfig;
use crate::graph::Product;

/// Custom retriever that fetches remote schemas via the webcache.
/// Carries the TTL because `Retrieve::retrieve` takes only the URI.
struct WebCacheRetriever {
    ttl_secs: u64,
}

impl jsonschema::Retrieve for WebCacheRetriever {
    fn retrieve(&self, uri: &jsonschema::Uri<String>) -> std::result::Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let url = uri.as_str();
        let body = crate::webcache::fetch(url, self.ttl_secs)?;
        let value: Value = serde_json::from_str(&body)?;
        Ok(value)
    }
}

pub struct IyamlschemaProcessor {
    config: IyamlschemaConfig,
}

impl IyamlschemaProcessor {
    pub const fn new(config: IyamlschemaConfig) -> Self {
        Self { config }
    }

    fn execute_product(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.check_files(ctx, &[product.primary_input()])
    }

    fn check_files(&self, ctx: &crate::build_context::BuildContext, files: &[&Path]) -> Result<()> {
        let mut errors = Vec::new();

        for file in files {
            if let Err(e) = self.validate_file(ctx, file) {
                errors.push(format!("{}: {}", file.display(), e));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            bail!("YAML schema validation failed:\n{}", errors.join("\n"))
        }
    }

    fn validate_file(&self, ctx: &crate::build_context::BuildContext, path: &Path) -> Result<()> {
        let ttl_secs = ctx.webcache_ttl_secs();
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        // Parse YAML into a JSON Value (for jsonschema validation)
        let data: Value = serde_yml::from_str(&contents)
            .with_context(|| format!("Failed to parse YAML in {}", path.display()))?;

        // Extract $schema URL
        let schema_url = data.get("$schema")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("no $schema field found"))?;

        // Fetch schema (cached)
        let schema_str = crate::webcache::fetch(schema_url, ttl_secs)
            .with_context(|| format!("Failed to fetch schema {schema_url}"))?;
        let schema: Value = serde_json::from_str(&schema_str)
            .with_context(|| format!("Failed to parse schema from {schema_url}"))?;

        // Validate data against schema (with custom retriever for remote $ref resolution)
        let validator = jsonschema::options()
            .with_retriever(WebCacheRetriever { ttl_secs })
            .build(&schema)
            .with_context(|| format!("Failed to compile schema from {schema_url}"))?;

        let validation_errors: Vec<String> = validator.iter_errors(&data)
            .map(|e| format!("  {}: {}", e.instance_path(), e))
            .collect();

        if !validation_errors.is_empty() {
            bail!("schema validation errors:\n{}", validation_errors.join("\n"));
        }

        // Check property ordering
        if self.config.check_ordering {
            let mut ordering_errors = Vec::new();
            check_property_ordering(&data, &schema, "", &mut ordering_errors);

            if !ordering_errors.is_empty() {
                bail!("property ordering errors:\n{}", ordering_errors.join("\n"));
            }
        }

        Ok(())
    }
}

/// Recursively check that data object keys match the `propertyOrdering`
/// declared in the schema.
fn check_property_ordering(
    data: &Value,
    schema: &Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    match (data, schema) {
        (Value::Object(data_map), Value::Object(schema_map)) => {
            // Check ordering at this level
            if let Some(Value::Array(expected_order)) = schema_map.get("propertyOrdering") {
                let expected: Vec<&str> = expected_order.iter()
                    .filter_map(|v| v.as_str())
                    .collect();

                let actual_keys: Vec<&str> = data_map.keys()
                    .map(std::string::String::as_str)
                    .collect();

                // Filter actual keys to only those in the expected list
                let actual_ordered: Vec<&str> = actual_keys.iter()
                    .copied()
                    .filter(|k| expected.contains(k))
                    .collect();

                // Filter expected to only those present in data
                let expected_ordered: Vec<&str> = expected.iter()
                    .copied()
                    .filter(|k| actual_keys.contains(k))
                    .collect();

                if actual_ordered != expected_ordered {
                    let display_path = if path.is_empty() { "root" } else { path };
                    errors.push(format!(
                        "  {display_path}: expected key order {expected_ordered:?}, got {actual_ordered:?}",
                    ));
                }
            }

            // Recurse into properties
            if let Some(Value::Object(props)) = schema_map.get("properties") {
                for (key, prop_schema) in props {
                    if let Some(value) = data_map.get(key) {
                        let child_path = if path.is_empty() {
                            key.to_string()
                        } else {
                            format!("{path}.{key}")
                        };
                        check_property_ordering(value, prop_schema, &child_path, errors);
                    }
                }
            }

            // Recurse into items (for arrays-of-objects)
            if let Some(items_schema) = schema_map.get("items")
                && let Value::Array(arr) = data
            {
                for (i, item) in arr.iter().enumerate() {
                    let child_path = format!("{path}[{i}]");
                    check_property_ordering(item, items_schema, &child_path, errors);
                }
            }
        }
        (Value::Array(arr), schema_val) => {
            // Schema might have "items" at this level
            if let Some(items_schema) = schema_val.get("items") {
                for (i, item) in arr.iter().enumerate() {
                    let child_path = if path.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("{path}[{i}]")
                    };
                    check_property_ordering(item, items_schema, &child_path, errors);
                }
            }
        }
        _ => {}
    }
}

impl crate::processors::Processor for IyamlschemaProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn standard_config(&self) -> Option<&crate::config::StandardConfig> {
        Some(&self.config.standard)
    }

    fn auto_detect(&self, file_index: &crate::file_index::FileIndex) -> bool {
        crate::processors::checker_auto_detect(&self.config.standard, file_index)
    }

    fn required_tools(&self) -> Vec<String> {
        Vec::new()
    }

    fn discover(
        &self,
        graph: &mut crate::graph::BuildGraph,
        file_index: &crate::file_index::FileIndex,
        instance_name: &str,
    ) -> anyhow::Result<()> {
        crate::processors::discover_checker_products(
            graph, &self.config.standard, file_index,
            &self.config.standard.dep_inputs, &self.config.standard.dep_auto,
            &self.config,
            <crate::config::IyamlschemaConfig as crate::config::KnownFields>::checksum_fields(),
            instance_name,
        )
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_product(ctx, product)
    }

    fn execute_batch(&self, ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        crate::processors::execute_checker_batch_per_file(products, |file| self.check_files(ctx, &[file]))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(IyamlschemaProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "iyamlschema",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::IyamlschemaConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::IyamlschemaConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::IyamlschemaConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::IyamlschemaConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::IyamlschemaConfig>,
        keywords: &["yaml", "yml", "schema", "validator"],
        description: "Validate YAML files against JSON schemas (in-process)",
        is_native: true,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
