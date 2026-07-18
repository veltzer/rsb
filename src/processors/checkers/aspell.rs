use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::io::Write;

use crate::config::AspellConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, scan_root_valid, log_command, format_command};
use crate::word_manager::WordManager;

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

    fn check_file(&self, file: &Path) -> Result<()> {
        let content = std::fs::read_to_string(file)
            .with_context(|| format!("Failed to read file: {}", file.display()))?;

        let mut cmd = Command::new(&self.config.standard.command);
        cmd.arg("--conf").arg(&self.config.conf);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        cmd.arg("list");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        log_command(&cmd);

        let mut child = cmd.spawn()
            .with_context(|| format!("Failed to spawn: {}", format_command(&cmd)))?;

        // Feed stdin from a separate thread while wait_with_output drains
        // stdout/stderr: writing everything first deadlocks once aspell fills
        // the pipe buffer with misspelled words.
        let mut stdin = child.stdin.take()
            .context("aspell stdin was not piped")?;
        let writer = std::thread::spawn(move || stdin.write_all(content.as_bytes()));

        let output = child.wait_with_output()
            .context("Failed to wait for aspell")?;
        let write_result = writer.join()
            .map_err(|_| anyhow::anyhow!("aspell stdin writer thread panicked"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("aspell failed for {}: {}", file.display(), stderr.trim_end());
        }
        // A write failure with a successful exit means aspell saw truncated
        // input; the results would be incomplete.
        write_result.context("Failed to write to aspell stdin")?;

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

    fn standard_config(&self) -> Option<&crate::config::StandardConfig> {
        Some(&self.config.standard)
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
        if !scan_root_valid(&self.config.standard) {
            return Ok(());
        }

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
            <crate::config::AspellConfig as crate::config::KnownFields>::checksum_fields(),
            instance_name,
        )
    }

    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.words.execute_with_flush(
            product,
            self.config.auto_add_words,
            |file| self.check_file(file),
            "aspell",
        )
    }

    fn execute_batch(&self, _ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        self.words.execute_batch_with_flush(
            products,
            self.config.auto_add_words,
            |file| self.check_file(file),
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
        defconfig_json: crate::registries::default_config_json::<crate::config::AspellConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::AspellConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::AspellConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::AspellConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::AspellConfig>,
        keywords: &["spellcheck", "spelling", "english", "checker"],
        description: "Check spelling using aspell",
        is_native: false,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
