use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use parking_lot::Mutex;

use crate::processors::flush_words;
use crate::graph::Product;

/// Shared word-file management for spell-checking processors (aspell, zspell).
///
/// Handles loading custom words, collecting misspelled words in auto-add mode,
/// and flushing new words to disk. Also provides the shared execute/batch pattern
/// where files are checked and words are flushed afterward.
pub struct WordManager {
    /// Words known on disk plus words already flushed this build.
    custom_words: Mutex<HashSet<String>>,
    words_to_add: Mutex<HashSet<String>>,
    words_file: String,
    header_line: Option<&'static str>,
}

impl WordManager {
    pub fn new(
        custom_words: HashSet<String>,
        words_file: String,
        header_line: Option<&'static str>,
    ) -> Self {
        Self {
            custom_words: Mutex::new(custom_words),
            words_to_add: Mutex::new(HashSet::new()),
            words_file,
            header_line,
        }
    }

    /// Check if a word is in the custom words set.
    pub fn is_known(&self, word: &str) -> bool {
        self.custom_words.lock().contains(word)
    }

    /// Handle misspelled words: collect them if auto_add_words is true, or return an error.
    pub fn handle_misspelled(
        &self,
        misspelled: &[impl AsRef<str>],
        file: &Path,
        auto_add_words: bool,
    ) -> Result<()> {
        if misspelled.is_empty() {
            return Ok(());
        }
        if auto_add_words {
            let mut words_to_add = self.words_to_add.lock();
            for word in misspelled {
                words_to_add.insert(word.as_ref().to_lowercase());
            }
            drop(words_to_add);
            Ok(())
        } else {
            let words: Vec<&str> = misspelled.iter().map(std::convert::AsRef::as_ref).collect();
            anyhow::bail!(
                "Misspelled words in {}:\n{}",
                file.display(),
                words.join("\n"),
            )
        }
    }

    /// Flush collected words to the words file. Flushed words move into the
    /// known set and the pending set is drained, so a later flush (there is
    /// one after every product) never re-appends them.
    pub fn flush(&self) -> Result<()> {
        let mut words_to_add = self.words_to_add.lock();
        if words_to_add.is_empty() {
            return Ok(());
        }
        let mut custom_words = self.custom_words.lock();
        let words_path = Path::new(&self.words_file);
        flush_words(
            &custom_words,
            &words_to_add,
            words_path,
            self.header_line,
        )?;
        custom_words.extend(words_to_add.drain());
        Ok(())
    }

    /// Execute a single product with auto-flush: check the file, then flush if
    /// auto_add_words. A failed flush fails the product — the collected words
    /// would otherwise be lost while the product caches as passing.
    pub fn execute_with_flush(
        &self,
        product: &Product,
        auto_add_words: bool,
        check_fn: impl FnOnce(&Path) -> Result<()>,
        processor_name: &str,
    ) -> Result<()> {
        let result = check_fn(product.primary_input());
        if auto_add_words
            && let Err(e) = self.flush()
        {
            return result.and(Err(e.context(format!("Failed to flush {processor_name} words file"))));
        }
        result
    }

    /// Execute a batch of products with auto-flush: check all files, then
    /// flush once. A failed flush fails every otherwise-passing product in the
    /// batch — the collected words would otherwise be lost while the products
    /// cache as passing.
    pub fn execute_batch_with_flush(
        &self,
        products: &[&Product],
        auto_add_words: bool,
        check_fn: impl Fn(&Path) -> Result<()>,
        processor_name: &str,
    ) -> Vec<Result<()>> {
        let mut results: Vec<Result<()>> = products
            .iter()
            .map(|p| check_fn(p.primary_input()))
            .collect();

        if auto_add_words
            && let Err(e) = self.flush()
        {
            let msg = format!("Failed to flush {processor_name} words file: {e:#}");
            for r in &mut results {
                if r.is_ok() {
                    *r = Err(anyhow::anyhow!("{msg}"));
                }
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// flush() runs after every product; an already-flushed word must never be
    /// appended again, and flushed words become known for later files.
    #[test]
    fn flush_does_not_reappend_words() {
        let tmp = tempfile::TempDir::new().unwrap();
        let words_file = tmp.path().join("words.txt");
        let mgr = WordManager::new(
            HashSet::new(),
            words_file.display().to_string(),
            None,
        );

        mgr.handle_misspelled(&["Frobnicate"], Path::new("a.md"), true).unwrap();
        mgr.flush().unwrap();
        assert!(mgr.is_known("frobnicate"), "flushed word must become known");

        // Later products flush again (and may re-collect nothing new)
        mgr.flush().unwrap();
        mgr.handle_misspelled(&["quuxify"], Path::new("b.md"), true).unwrap();
        mgr.flush().unwrap();

        let content = std::fs::read_to_string(&words_file).unwrap();
        let frob_count = content.lines().filter(|l| *l == "frobnicate").count();
        let quux_count = content.lines().filter(|l| *l == "quuxify").count();
        assert_eq!(frob_count, 1, "word must appear exactly once, got:\n{content}");
        assert_eq!(quux_count, 1, "word must appear exactly once, got:\n{content}");
    }
}
