use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::StandardConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::Processor;
use crate::word_manager::WordManager;

fn default_aspell_conf() -> String {
    ".aspell.conf".into()
}

fn default_aspell_words_file() -> String {
    ".aspell.en.pws".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AspellConfig {
    #[serde(default = "default_aspell_conf")]
    pub conf: String,
    #[serde(default)]
    pub auto_add_words: bool,
    #[serde(default = "default_aspell_words_file")]
    pub words_file: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for AspellConfig {
    fn default() -> Self {
        Self {
            conf: ".aspell.conf".into(),
            auto_add_words: false,
            words_file: ".aspell.en.pws".into(),
            standard: StandardConfig::default(),
        }
    }
}

pub struct AspellProcessor {
    config: AspellConfig,
    words: WordManager,
}

impl AspellProcessor {
    pub fn new(config: AspellConfig) -> Result<Self> {
        let custom_words = Self::load_custom_words(Path::new(&config.words_file))?;
        let words = WordManager::new(
            custom_words,
            config.words_file.clone(),
            Some("personal_ws-1.1 en 0"),
        );
        Ok(Self {
            config,
            words,
        })
    }

    /// Load custom words from the aspell personal word list (.pws) file.
    /// An unreadable file is an error — treating it as empty would report
    /// every allow-listed word as misspelled.
    fn load_custom_words(words_path: &Path) -> Result<HashSet<String>> {
        if !words_path.exists() {
            return Ok(HashSet::new());
        }
        let content = std::fs::read_to_string(words_path)
            .with_context(|| format!("Failed to read aspell words file: {}", words_path.display()))?;
        Ok(content
            .lines()
            .filter(|l| !l.starts_with("personal_ws"))
            .map(|l| l.trim().to_lowercase())
            .filter(|l| !l.is_empty())
            .collect())
    }

    fn check_file(&self, ctx: &crate::build_context::BuildContext, file: &Path) -> Result<()> {
        let content = std::fs::read_to_string(file)
            .with_context(|| format!("Failed to read file: {}", file.display()))?;

        let mut cmd = Command::new(&self.config.standard.command);
        cmd.arg("--conf").arg(&self.config.conf);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        cmd.arg("list");

        // Goes through the central runner rather than a hand-rolled spawn, so
        // this inherits interrupt handling, kill_on_drop, log_command and the
        // declared-tools check. The stdin write is driven concurrently with
        // draining the output — feeding it all first deadlocks as soon as
        // aspell fills the pipe buffer with misspelled words.
        let output = crate::processors::run_command_with_stdin(ctx, &cmd, content.as_bytes())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("aspell failed for {}: {}", file.display(), stderr.trim_end());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let misspelled: Vec<&str> = stdout.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .filter(|l| !self.words.is_known(&l.to_lowercase()))
            .collect();

        self.words.handle_misspelled(&misspelled, file, self.config.auto_add_words)
    }
}

impl Processor for AspellProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    // Serialize the FULL config (the trait default covers StandardConfig
    // only), so the extra fields reach config-change detection.
    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone()]
    }

    fn discover(
        &self,
        graph: &mut BuildGraph,
        file_index: &FileIndex,
        instance_name: &str,
    ) -> Result<()> {
        // The personal dictionary is a real input: removing a word must
        // invalidate cached passing checks. dep_auto only includes the file
        // when it exists.
        let mut dep_auto = self.config.standard.dep_auto.clone();
        dep_auto.push(self.config.words_file.clone());

        crate::processors::discover_checker_products(
            graph,
            &self.config.standard,
            file_index,
            &self.config.standard.dep_inputs,
            &dep_auto,
            &self.config,
            &crate::config::checksum_fields_of(instance_name),
            instance_name,
        )
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.words.execute_with_flush(
            product,
            self.config.auto_add_words,
            |file| self.check_file(ctx, file),
            "aspell",
        )
    }

    fn execute_batch(&self, ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        self.words.execute_batch_with_flush(
            products,
            self.config.auto_add_words,
            |file| self.check_file(ctx, file),
            "aspell",
        )
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_try_create(toml, |cfg| Ok(Box::new(AspellProcessor::new(cfg)?)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "aspell",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "conf", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Path to the aspell configuration file" },
            crate::config::FieldSpec { name: "auto_add_words", ty: crate::config::FieldType::Bool,
                affects_output: true, required: false,
                doc: "When true, automatically add misspelled words to words_file instead of failing" },
            crate::config::FieldSpec { name: "words_file", ty: crate::config::FieldType::String,
                affects_output: false, required: false,
                doc: "Path to the personal word list file" },
        ],
        omit_standard_fields: &["formats", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] }),
        defaults: Some(crate::config::ProcessorDefaults { command: "aspell", dep_auto: &[".aspell.conf", ".aspell.en.pws", ".aspell.en.prepl"], ..crate::config::ProcessorDefaults::EMPTY }),
        defconfig_json: crate::registries::default_config_json::<AspellConfig>,
        keywords: &["spellcheck", "spelling", "english", "checker"],
        description: "Check spelling using aspell",
        is_native: false,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
