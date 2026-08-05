mod cc_single_file;
mod generator;
mod ipdfunite;
mod jinja2;
mod linux_module;
mod mako;
mod pdflatex;
mod pdfunite;
mod rust_single_file;
mod mermaid;
mod drawio;
mod sass;
mod protobuf;
mod chromium;
mod markdown2html;
mod libreoffice;
mod marp;
mod pandoc;
mod a2x;
mod objdump;
mod imarkdown2html;
mod isass;
mod yaml2json;

mod requirements;
pub mod tags;
pub mod tera;


use std::path::{Path, PathBuf};
use anyhow::Result;
use serde::Serialize;

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::BuildGraph;

/// Represents a single template file to be processed (shared by tera and mako).
pub(super) struct TemplateItem {
    /// Path to the template file
    pub source_path: PathBuf,
    /// Path where the rendered output will be written
    pub output_path: PathBuf,
}

impl TemplateItem {
    pub const fn new(source_path: PathBuf, output_path: PathBuf) -> Self {
        Self {
            source_path,
            output_path,
        }
    }
}

/// Find all template files matching configured extensions, stripping the
/// extension to produce the output path. Shared by tera and mako processors.
pub(super) fn find_templates(scan: &StandardConfig, file_index: &FileIndex) -> Vec<TemplateItem> {
    let paths = file_index.scan(scan, true);
    let extensions = scan.src_extensions();
    let src_dirs = scan.src_dirs();

    let mut items = Vec::new();
    for path in paths {
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        for ext in extensions {
            if filename.ends_with(ext.as_str()) {
                let output_name = &filename[..filename.len() - ext.len()];
                if !output_name.is_empty() {
                    // Strip the matching scan_dir prefix to get the output path
                    let output_path = src_dirs.iter()
                        .filter(|d| !d.is_empty())
                        .find_map(|d| path.strip_prefix(d).ok().map(|r| r.with_file_name(output_name)))
                        .unwrap_or_else(|| PathBuf::from(output_name));
                    items.push(TemplateItem::new(path.clone(), output_path));
                    break;
                }
            }
        }
    }

    items
}

/// Parameters shared by multi-format and single-format discover helpers.
pub struct DiscoverParams<'a, C: Serialize> {
    pub scan: &'a StandardConfig,
    pub dep_inputs: &'a [String],
    pub config: &'a C,
    pub output_dir: &'a str,
    pub processor_name: &'a str,
    /// Allowlist of field names to include in the config-change checksum,
    /// derived from the plugin's `FieldSpec` list (`checksum_fields_of`).
    pub checksum_fields: Vec<&'static str>,
}

/// Compute the output path for a source file.
///
/// Strips the matching `src_dirs` prefix from the source path, replaces the extension,
/// and joins the result under `output_dir`. This is the single place where
/// source-to-output path mapping is defined.
pub(super) fn output_path(source: &Path, src_dirs: &[String], output_dir: &str, extension: &str) -> PathBuf {
    let full_parent = super::parent_dir_or_empty(source);
    let parent = src_dirs.iter()
        .filter(|d| !d.is_empty())
        .find_map(|d| full_parent.strip_prefix(d).ok())
        .unwrap_or(full_parent);
    let stem = source.file_stem().unwrap_or_default();
    let output_name = format!("{}.{}", stem.to_string_lossy(), extension);
    Path::new(output_dir).join(parent).join(output_name)
}

/// Discover one product per source x format pair. Returns Ok(()) immediately
/// if the scan root is invalid (directory doesn't exist).
pub fn discover_multi_format(
    graph: &mut BuildGraph,
    file_index: &FileIndex,
    params: &DiscoverParams<'_, impl Serialize>,
    formats: &[String],
) -> Result<()> {
    let Some(files) = crate::processors::scan_or_skip(params.scan, file_index) else {
        return Ok(());
    };

    let hash = Some(output_config_hash(params.config, &params.checksum_fields));
    let extra = resolve_extra_inputs(params.dep_inputs)?;
    let src_dirs = params.scan.src_dirs();

    for source in &files {
        for format in formats {
            let output = output_path(source, src_dirs, params.output_dir, format);

            let mut inputs = Vec::with_capacity(1 + extra.len());
            inputs.push(source.clone());
            inputs.extend_from_slice(&extra);

            graph.add_product_with_variant(inputs, vec![output], params.processor_name, hash.clone(), Some(format))?;
        }
    }

    Ok(())
}

/// Discover one product per source file. Returns Ok(()) immediately
/// if the scan root is invalid (directory doesn't exist).
pub fn discover_single_format(
    graph: &mut BuildGraph,
    file_index: &FileIndex,
    params: &DiscoverParams<'_, impl Serialize>,
    extension: &str,
) -> Result<()> {
    let format = extension.to_owned();
    discover_multi_format(graph, file_index, params, &[format])
}
