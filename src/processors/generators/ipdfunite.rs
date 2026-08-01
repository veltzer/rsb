use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{IpdfuniteConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::Processor;

pub struct IpdfuniteProcessor {
    config: IpdfuniteConfig,
}

impl IpdfuniteProcessor {
    pub const fn new(config: IpdfuniteConfig) -> Self {
        Self {
            config,
        }
    }

    /// Source files under `source_dir` with the configured extension, grouped
    /// by their containing directory (sorted, so both the directory order and
    /// the merge order within a directory are deterministic).
    ///
    /// Goes through `FileIndex` rather than walking the filesystem: that is
    /// what makes `.rsconstructignore` apply and lets virtual files produced
    /// by an upstream processor be merged. Walking `fs::read_dir` here saw
    /// neither, and re-ran the IO on every fixed-point discovery pass.
    fn source_dirs(&self, file_index: &FileIndex) -> Vec<(PathBuf, Vec<PathBuf>)> {
        let ext = if self.config.source_ext.starts_with('.') {
            self.config.source_ext.clone()
        } else {
            format!(".{}", self.config.source_ext)
        };
        let base = Path::new(&self.config.source_dir);
        let files = file_index.query(base, &[&ext], &[], &[], &[], &[]);

        let mut by_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        for file in files {
            let dir = crate::processors::parent_dir_or_empty(&file).to_path_buf();
            by_dir.entry(dir).or_default().push(file);
        }
        for files in by_dir.values_mut() {
            files.sort();
        }
        by_dir.into_iter().collect()
    }
}

/// Merge multiple PDF files into a single output using lopdf.
/// Follows the lopdf merge example: renumber objects, collect pages and objects,
/// then assemble a new document with a unified catalog and pages tree.
fn merge_pdfs(inputs: &[PathBuf], output: &Path) -> Result<()> {
    use lopdf::{Document, Object, ObjectId};

    let mut documents: Vec<Document> = Vec::with_capacity(inputs.len());
    for input in inputs {
        let doc = Document::load(input)
            .with_context(|| format!("Failed to load PDF: {}", input.display()))?;
        documents.push(doc);
    }

    let mut max_id = 1;
    let mut documents_pages: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut documents_objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut document = Document::with_version("1.5");

    for mut doc in documents {
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;

        for object_id in doc.get_pages().into_values() {
            let obj = doc.get_object(object_id)
                .with_context(|| format!("PDF object {object_id:?} referenced by Pages tree not found in document"))?
                .to_owned();
            documents_pages.insert(object_id, obj);
        }
        documents_objects.extend(doc.objects);
    }

    // Find "Catalog" and "Pages" objects from the collected objects
    let mut catalog_object: Option<(ObjectId, Object)> = None;
    let mut pages_object: Option<(ObjectId, Object)> = None;

    for (object_id, object) in &documents_objects {
        let type_name = object.as_dict()
            .ok()
            .and_then(|d| d.get(b"Type").ok())
            .and_then(|t| t.as_name().ok())
            .map(<[u8]>::to_vec);

        match type_name.as_deref() {
            Some(b"Catalog") => {
                catalog_object = Some((*object_id, object.clone()));
            }
            Some(b"Pages") => {
                if let Ok(dictionary) = object.as_dict() {
                    let mut dictionary = dictionary.clone();
                    dictionary.remove(b"Outlines");
                    if let Some((_, ref existing)) = pages_object
                        && let Ok(old_dict) = existing.as_dict()
                    {
                        dictionary.extend(old_dict);
                    }
                    pages_object = Some((
                        pages_object.as_ref().map_or(*object_id, |(id, _)| *id),
                        Object::Dictionary(dictionary),
                    ));
                }
            }
            Some(b"Page") => {} // handled separately
            Some(b"Outlines" | b"Outline") => {} // not supported
            _ => {
                document.objects.insert(*object_id, object.clone());
            }
        }
    }

    let catalog_object = catalog_object
        .context("No PDF Catalog found in input documents")?;
    let pages_object = pages_object
        .context("No PDF Pages tree found in input documents")?;

    // Set page parents and insert into document
    for (object_id, object) in &documents_pages {
        if let Ok(dictionary) = object.as_dict() {
            let mut dictionary = dictionary.clone();
            dictionary.set("Parent", pages_object.0);
            document.objects.insert(*object_id, Object::Dictionary(dictionary));
        }
    }

    // Build new Pages object with all kids
    if let Ok(dictionary) = pages_object.1.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Count", documents_pages.len() as u32);
        dictionary.set(
            "Kids",
            documents_pages.into_keys()
                .map(Object::Reference)
                .collect::<Vec<_>>(),
        );
        document.objects.insert(pages_object.0, Object::Dictionary(dictionary));
    }

    // Build new Catalog
    if let Ok(dictionary) = catalog_object.1.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Pages", pages_object.0);
        dictionary.remove(b"Outlines");
        document.objects.insert(catalog_object.0, Object::Dictionary(dictionary));
    }

    document.trailer.set("Root", catalog_object.0);
    document.max_id = max_id;
    document.renumber_objects();
    document.adjust_zero_pages();
    if let Some(n) = document.build_outline()
        && let Ok(Object::Dictionary(dict)) = document.get_object_mut(catalog_object.0)
    {
        dict.set("Outlines", Object::Reference(n));
    }
    document.compress();

    document.save(output)
        .with_context(|| format!("Failed to write merged PDF: {}", output.display()))?;

    Ok(())
}

impl Processor for IpdfuniteProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        !self.source_dirs(file_index).is_empty()
    }

    fn required_tools(&self) -> Vec<String> {
        Vec::new()
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let base = Path::new(&self.config.source_dir);

        let hash = Some(output_config_hash(&self.config, <crate::config::IpdfuniteConfig as crate::config::KnownFields>::checksum_fields()));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        let upstream_scan_dir = Path::new(&self.config.source_dir)
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .context("source_dir is empty")?;
        let upstream_scan_dirs = [upstream_scan_dir];

        for (dir_path, source_files) in self.source_dirs(file_index) {
            let inputs: Vec<PathBuf> = source_files.iter().map(|src| {
                super::output_path(src, &upstream_scan_dirs, &self.config.source_output_dir, "pdf")
            }).chain(extra.iter().cloned()).collect();

            let relative = dir_path.strip_prefix(base).unwrap_or(&dir_path);
            let parent = crate::processors::parent_dir_or_empty(relative);
            let leaf = relative.file_name()
                .with_context(|| format!("Cannot extract leaf directory name from {}", dir_path.display()))?;
            let outputs = vec![
                Path::new(&self.config.standard.output_dir).join(parent).join(format!("{}.pdf", leaf.to_string_lossy())),
            ];

            graph.add_product(inputs, outputs, instance_name, hash.clone())?;
        }

        Ok(())
    }

    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let output = product.primary_output();
        crate::processors::ensure_output_dir(output)?;

        let pdf_inputs: Vec<PathBuf> = product.inputs.iter()
            .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
            .cloned()
            .collect();

        if pdf_inputs.is_empty() {
            anyhow::bail!("No PDF inputs found for {}", output.display());
        }

        merge_pdfs(&pdf_inputs, output)
    }

}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(IpdfuniteProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "ipdfunite",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::IpdfuniteConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::IpdfuniteConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::IpdfuniteConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::IpdfuniteConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::IpdfuniteConfig>,
        keywords: &["pdf", "merger", "generator"],
        description: "Merge PDFs from subdirectories into course bundles (in-process)",
        is_native: true,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
