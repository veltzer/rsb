use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;

use crate::config::JsonSchemaConfig;
use crate::graph::Product;

pub struct JsonSchemaProcessor {
    config: JsonSchemaConfig,
}

impl JsonSchemaProcessor {
    pub const fn new(config: JsonSchemaConfig) -> Self {
        Self { config }
    }

    fn execute_product(&self, product: &Product) -> Result<()> {
        let path = product.primary_input();
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let value: Value = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse JSON in {}", path.display()))?;

        let mut errors = Vec::new();
        check_property_ordering(&value, "$", &mut errors);

        if !errors.is_empty() {
            anyhow::bail!(
                "propertyOrdering mismatch in {}:\n{}",
                path.display(),
                errors.join("\n")
            );
        }
        Ok(())
    }
}

/// Recursively check that every object with `type: "object"` has a
/// `propertyOrdering` array that exactly matches its `properties` keys.
fn check_property_ordering(value: &Value, path: &str, errors: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            // Check if this is a JSON Schema object definition
            let is_object_type = map.get("type")
                .and_then(|v| v.as_str())
                .is_some_and(|t| t == "object");

            if is_object_type {
                let has_properties = map.contains_key("properties");
                let has_ordering = map.contains_key("propertyOrdering");

                if has_properties && has_ordering {
                    let prop_keys: BTreeSet<&str> = map["properties"]
                        .as_object()
                        .map(|o| o.keys().map(std::string::String::as_str).collect())
                        .unwrap_or_default();

                    let ordering_keys: BTreeSet<&str> = map["propertyOrdering"]
                        .as_array()
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                        .unwrap_or_default();

                    let missing: Vec<&&str> = prop_keys.difference(&ordering_keys).collect();
                    let extra: Vec<&&str> = ordering_keys.difference(&prop_keys).collect();

                    if !missing.is_empty() {
                        errors.push(format!(
                            "  {path}: missing from propertyOrdering: {missing:?}"
                        ));
                    }
                    if !extra.is_empty() {
                        errors.push(format!(
                            "  {path}: extra in propertyOrdering: {extra:?}"
                        ));
                    }
                }
            }

            // Recurse into all values
            for (key, val) in map {
                let child_path = format!("{path}.{key}");
                check_property_ordering(val, &child_path, errors);
            }
        }
        Value::Array(arr) => {
            for (i, val) in arr.iter().enumerate() {
                let child_path = format!("{path}[{i}]");
                check_property_ordering(val, &child_path, errors);
            }
        }
        _ => {}
    }
}

impl crate::processors::Processor for JsonSchemaProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn required_tools(&self) -> Vec<String> {
        Vec::new()
    }


    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_product(product)
    }

}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(JsonSchemaProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "json_schema",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::JsonSchemaConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::JsonSchemaConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::JsonSchemaConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::JsonSchemaConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::JsonSchemaConfig>,
        keywords: &["json", "schema", "validator", "checker"],
        description: "Validate propertyOrdering in JSON schema files",
        is_native: true,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
