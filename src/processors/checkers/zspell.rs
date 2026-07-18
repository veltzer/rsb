use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use crate::config::ZspellConfig;
use crate::errors;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, discover_checker_products};
use crate::word_manager::WordManager;

pub struct ZspellProcessor {
    config: ZspellConfig,
    /// Cached dictionary, built once on first use and reused across all execute() calls
    cached_dict: OnceLock<Result<zspell::Dictionary, String>>,
    words: WordManager,
}

impl ZspellProcessor {
    pub fn new(config: ZspellConfig) -> Result<Self> {
        let words_path = Path::new(&config.words_file);
        // An unreadable words file is an error — treating it as empty would
        // report every allow-listed word as misspelled.
        let custom_words = if words_path.exists() {
            Self::load_custom_words(words_path)?
        } else {
            HashSet::new()
        };
        let words = WordManager::new(
            custom_words,
            config.words_file.clone(),
            None,
        );
        Ok(Self {
            config,
            cached_dict: OnceLock::new(),
            words,
        })
    }

    /// Load custom words from the words file
    fn load_custom_words(words_path: &Path) -> Result<HashSet<String>> {
        let content = crate::errors::ctx(fs::read_to_string(words_path), &format!("Failed to read words file: {}", words_path.display()))?;
        let mut words = HashSet::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                words.insert(trimmed.to_lowercase());
            }
        }
        Ok(words)
    }

    /// Build a zspell Dictionary from system hunspell files
    fn build_dictionary(&self) -> Result<zspell::Dictionary> {
        let lang = &self.config.language;
        let dict_dir = Path::new(&self.config.dict_dir);
        let aff_path = dict_dir.join(format!("{lang}.aff"));
        let dic_path = dict_dir.join(format!("{lang}.dic"));

        let aff_content = fs::read_to_string(&aff_path)
            .with_context(|| format!("Failed to read affix file: {}. Is the hunspell dictionary for '{}' installed?", aff_path.display(), lang))?;
        let dic_content = fs::read_to_string(&dic_path)
            .with_context(|| format!("Failed to read dictionary file: {}. Is the hunspell dictionary for '{}' installed?", dic_path.display(), lang))?;

        let dict = zspell::builder()
            .config_str(&aff_content)
            .dict_str(&dic_content)
            .build()
            .context("Failed to build zspell dictionary")?;

        Ok(dict)
    }

    /// Get or build the cached dictionary (built once, reused across all files)
    fn get_dictionary(&self) -> Result<&zspell::Dictionary> {
        let result = self.cached_dict.get_or_init(|| {
            self.build_dictionary().map_err(|e| e.to_string())
        });
        match result {
            Ok(dict) => Ok(dict),
            Err(msg) => anyhow::bail!("{msg}"),
        }
    }

    /// Extract words from markdown text, stripping code blocks, inline code, URLs, and HTML tags
    fn extract_words(text: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut in_fenced_block = false;

        for line in text.lines() {
            let trimmed = line.trim();

            // Toggle fenced code blocks
            if trimmed.starts_with("```") {
                in_fenced_block = !in_fenced_block;
                continue;
            }
            if in_fenced_block {
                continue;
            }

            // Skip indented code blocks (4 spaces or 1 tab)
            if line.starts_with("    ") || line.starts_with('\t') {
                continue;
            }

            let cleaned = Self::strip_markdown(line);

            // Split on non-alphabetic characters and collect words
            for word in cleaned.split(|c: char| !c.is_alphabetic()) {
                if word.len() >= 2 {
                    result.push(word.to_string());
                }
            }
        }

        result
    }

    /// Strip markdown syntax from a line using a single regex pass.
    fn strip_markdown(line: &str) -> String {
        static MARKDOWN_RE: OnceLock<Regex> = OnceLock::new();
        let re = MARKDOWN_RE.get_or_init(|| Regex::new(concat!(
            r"`[^`]*`",                          // inline code spans
            r"|\[([^\]]*)\]\([^)]*\)",            // [text](url) — capture group 1 = link text
            r#"|https?://[^\s)>""]+"#,            // bare URLs
            r"|<[^>]*>",                          // HTML tags
        )).expect(errors::INVALID_REGEX));

        re.replace_all(line, |caps: &regex::Captures| {
            // For markdown links, keep the link text; for everything else, replace with space
            caps.get(1).map_or(" ".to_string(), |m| m.as_str().to_string())
        }).into_owned()
    }

    /// Check a single file for spelling errors
    fn check_file(&self, doc_file: &Path) -> Result<()> {
        let dict = self.get_dictionary()?;

        let content = fs::read_to_string(doc_file)
            .with_context(|| format!("Failed to read document file: {}", doc_file.display()))?;

        let words = Self::extract_words(&content);
        let mut misspelled: Vec<String> = Vec::new();
        let mut seen = HashSet::new();

        for word in &words {
            let lower = word.to_lowercase();
            if seen.contains(&lower) {
                continue;
            }
            seen.insert(lower.clone());

            if self.words.is_known(&lower) {
                continue;
            }

            if !dict.check_word(&lower) {
                misspelled.push(word.clone());
            }
        }

        misspelled.sort();
        self.words.handle_misspelled(&misspelled, doc_file, self.config.auto_add_words)
    }
}

impl Processor for ZspellProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        // The personal dictionary is a real input: removing a word must
        // invalidate cached passing checks. dep_auto only includes the file
        // when it exists.
        let mut dep_auto = self.config.standard.dep_auto.clone();
        dep_auto.push(self.config.words_file.clone());

        discover_checker_products(
            graph,
            &self.config.standard,
            file_index,
            &self.config.standard.dep_inputs,
            &dep_auto,
            &self.config,
            <crate::config::ZspellConfig as crate::config::KnownFields>::checksum_fields(),
            instance_name,
        )
    }

    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.words.execute_with_flush(
            product,
            self.config.auto_add_words,
            |file| self.check_file(file),
            "zspell",
        )
    }

    fn execute_batch(&self, _ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        self.words.execute_batch_with_flush(
            products,
            self.config.auto_add_words,
            |file| self.check_file(file),
            "zspell",
        )
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_try_create(toml, |cfg| Ok(Box::new(ZspellProcessor::new(cfg)?)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "zspell",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::ZspellConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::ZspellConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::ZspellConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::ZspellConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::ZspellConfig>,
        keywords: &["spellcheck", "spelling", "markdown", "md", "english"],
        description: "Check documentation files for spelling errors",
        is_native: true,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
