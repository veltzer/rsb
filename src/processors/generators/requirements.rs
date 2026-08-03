//! Requirements generator — produces a `requirements.txt` from Python imports.
//!
//! Scans every `.py` file in the project, collects the top-level import names,
//! filters out local modules (resolve to project files) and stdlib, maps each
//! remaining import name to its `PyPI` distribution name, and writes the sorted
//! result to `requirements.txt`.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::analyzers::python::scan_python_imports;
use crate::config::{RequirementsConfig, output_config_hash, resolve_extra_inputs, KnownFields};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, ensure_output_dir};

pub struct RequirementsProcessor {
    config: RequirementsConfig,
}

impl RequirementsProcessor {
    pub const fn new(config: RequirementsConfig) -> Self {
        Self {
            config,
        }
    }

    /// Map an import name to a distribution name: user config wins over the
    /// built-in curated table, which in turn wins over identity.
    fn distribution_for(&self, import_name: &str) -> String {
        if let Some(mapped) = self.config.mapping.get(import_name) {
            return mapped.clone();
        }
        resolve_distribution(import_name).to_string()
    }
}

impl Processor for RequirementsProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        !file_index.scan(&self.config.standard, false).is_empty()
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let files = file_index.scan(&self.config.standard, true);
        if files.is_empty() {
            return Ok(());
        }

        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;
        let mut inputs = Vec::with_capacity(files.len() + extra.len());
        inputs.extend(files);
        inputs.extend_from_slice(&extra);

        let output = PathBuf::from(&self.config.output);
        graph.add_product(
            inputs,
            vec![output],
            instance_name,
            Some(output_config_hash(&self.config, RequirementsConfig::checksum_fields())),
        )?;
        Ok(())
    }

    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let output_path = product.primary_output();
        ensure_output_dir(output_path)?;

        // The file index is not available inside execute(). Build a local set
        // of input .py files to recognize local imports that resolve to
        // another product input.
        let local_py: HashSet<&Path> = product.inputs.iter()
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("py"))
            .map(std::path::PathBuf::as_path)
            .collect();

        let exclude: HashSet<&str> = self.config.exclude.iter()
            .map(std::string::String::as_str)
            .collect();

        // Preserve first-seen order for the non-sorted case; BTreeSet gives
        // sorted order for free when requested.
        let mut first_seen: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for input in &product.inputs {
            if input.extension().and_then(|e| e.to_str()) != Some("py") {
                continue;
            }
            let modules = scan_python_imports(input)
                .with_context(|| format!("Failed to scan imports in {}", input.display()))?;
            for module in modules {
                let top = module.split('.').next().unwrap_or(&module);
                if top.is_empty() {
                    continue;
                }
                if exclude.contains(top) {
                    continue;
                }
                if is_stdlib(top) {
                    continue;
                }
                if is_local(input, top, &local_py, &self.config.python_paths) {
                    continue;
                }
                let dist = self.distribution_for(top);
                if seen.insert(dist.clone()) {
                    first_seen.push(dist);
                }
            }
        }

        // Distributions explicitly listed in `extra` are appended after the
        // import-derived set. They bypass exclude and stdlib filters because
        // they were declared by the user on purpose. Order: import-derived
        // first, then extras (only matters when sorted=false).
        for dist in &self.config.extra {
            if seen.insert(dist.clone()) {
                first_seen.push(dist.clone());
            }
        }

        let entries: Vec<String> = if self.config.sorted {
            let set: BTreeSet<String> = first_seen.into_iter().collect();
            set.into_iter().collect()
        } else {
            first_seen
        };

        let mut file = fs::File::create(output_path)
            .with_context(|| format!("Failed to create {}", output_path.display()))?;
        if self.config.header {
            writeln!(file, "# Generated by rsconstruct — do not edit by hand")
                .with_context(|| format!("Failed to write header to {}", output_path.display()))?;
        }
        for entry in &entries {
            writeln!(file, "{entry}")
                .with_context(|| format!("Failed to write entry to {}", output_path.display()))?;
        }

        Ok(())
    }
}

/// Check whether an import from `source` resolves to a file that's part of
/// the project's Python input set. The importer's own directory and the
/// project root are always checked. Additional roots come from the
/// processor's `python_paths` config — directories the user declares as
/// being on `sys.path` at runtime (e.g. via PYTHONPATH or `sys.path.insert`).
fn is_local(
    source: &Path,
    module: &str,
    local_py: &HashSet<&Path>,
    python_paths: &[String],
) -> bool {
    let module_path = module.replace('.', "/");
    let source_dir = crate::processors::parent_dir(source);

    let mut roots: Vec<PathBuf> = Vec::with_capacity(2 + python_paths.len());
    roots.push(source_dir.to_path_buf());
    roots.push(PathBuf::from("."));
    for p in python_paths {
        roots.push(PathBuf::from(p));
    }

    for root in &roots {
        let candidates = [
            root.join(format!("{module_path}.py")),
            root.join(&module_path).join("__init__.py"),
        ];
        for candidate in &candidates {
            if local_py.contains(candidate.as_path()) {
                return true;
            }
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Import name → PyPI distribution name mapping.
//
// Most PyPI distributions use the same name as their top-level import — we
// default to identity. This table lists the common exceptions where the
// import name differs from the distribution name. Users can override these
// via the `mapping` config field; user entries win.
// ---------------------------------------------------------------------------

/// Resolve a Python import name to a `PyPI` distribution name using the curated
/// table. Returns the distribution name if the import is mapped, or the
/// original import name otherwise. Callers should consult the user's
/// configured mapping first.
fn resolve_distribution(import_name: &str) -> &str {
    MAPPINGS.binary_search_by_key(&import_name, |&(k, _)| k)
        .ok()
        .map_or(import_name, |i| MAPPINGS[i].1)
}

/// Sorted list of (`import_name`, `distribution_name`) pairs. Must stay sorted —
/// `resolve_distribution` relies on binary search.
const MAPPINGS: &[(&str, &str)] = &[
    ("PIL",                    "Pillow"),
    ("attr",                   "attrs"),
    ("bs4",                    "beautifulsoup4"),
    ("cv2",                    "opencv-python"),
    ("dateutil",               "python-dateutil"),
    ("discord",                "discord.py"),
    ("dns",                    "dnspython"),
    ("docx",                   "python-docx"),
    ("dotenv",                 "python-dotenv"),
    ("fitz",                   "PyMuPDF"),
    ("git",                    "GitPython"),
    ("google",                 "google-api-python-client"),
    ("grpc",                   "grpcio"),
    ("gym",                    "gymnasium"),
    ("jwt",                    "PyJWT"),
    ("magic",                  "python-magic"),
    ("mpl_toolkits",           "matplotlib"),
    ("mx",                     "egenix-mx-base"),
    ("nacl",                   "PyNaCl"),
    ("pptx",                   "python-pptx"),
    ("psycopg2",               "psycopg2-binary"),
    ("pycountry",              "pycountry"),
    ("pycryptodome",           "pycryptodome"),
    ("serial",                 "pyserial"),
    ("skimage",                "scikit-image"),
    ("sklearn",                "scikit-learn"),
    ("slugify",                "python-slugify"),
    ("socks",                  "PySocks"),
    ("tensorflow_datasets",    "tensorflow-datasets"),
    ("tensorflow_hub",         "tensorflow-hub"),
    ("tensorflow_probability", "tensorflow-probability"),
    ("uvicorn",                "uvicorn"),
    ("win32api",               "pywin32"),
    ("win32com",               "pywin32"),
    ("win32con",               "pywin32"),
    ("wx",                     "wxPython"),
    ("yaml",                   "PyYAML"),
    ("zmq",                    "pyzmq"),
];

// ---------------------------------------------------------------------------
// Python stdlib module names.
//
// Generated from `python3 -c 'import sys; print(sorted(sys.stdlib_module_names))'`
// on Python 3.12. Covers 3.10+ (names added in later minor releases are included;
// removed names are not present in older releases, which matches desired behavior).
//
// `is_stdlib` checks only the top-level name (`os.path` → `os`), which matches
// how Python `sys.stdlib_module_names` is structured.
// ---------------------------------------------------------------------------

/// Returns true if the given top-level module name is part of the Python
/// stdlib. `module` should be the top-level name (e.g. "os" from "os.path").
fn is_stdlib(module: &str) -> bool {
    STDLIB_MODULES.binary_search(&module).is_ok()
}

/// Sorted list of stdlib top-level module names. Must stay sorted — `is_stdlib`
/// relies on binary search.
const STDLIB_MODULES: &[&str] = &[
    "__future__", "_abc", "_aix_support", "_ast", "_asyncio", "_bisect", "_blake2",
    "_bz2", "_codecs", "_codecs_cn", "_codecs_hk", "_codecs_iso2022", "_codecs_jp",
    "_codecs_kr", "_codecs_tw", "_collections", "_collections_abc", "_compat_pickle",
    "_compression", "_contextvars", "_csv", "_ctypes", "_curses", "_curses_panel",
    "_datetime", "_decimal", "_elementtree", "_frozen_importlib",
    "_frozen_importlib_external", "_functools", "_hashlib", "_heapq", "_imp", "_io",
    "_json", "_locale", "_lsprof", "_lzma", "_markupbase", "_md5", "_multibytecodec",
    "_multiprocessing", "_opcode", "_operator", "_osx_support", "_pickle",
    "_posixshmem", "_posixsubprocess", "_py_abc", "_pydecimal", "_pyio", "_queue",
    "_random", "_sha1", "_sha2", "_sha3", "_signal", "_sitebuiltins", "_socket",
    "_sqlite3", "_sre", "_ssl", "_stat", "_statistics", "_string", "_strptime",
    "_struct", "_symtable", "_thread", "_threading_local", "_tkinter", "_tokenize",
    "_tracemalloc", "_typing", "_uuid", "_warnings", "_weakref", "_weakrefset",
    "_zoneinfo", "abc", "aifc", "antigravity", "argparse", "array", "ast",
    "asynchat", "asyncio", "asyncore", "atexit", "audioop", "base64", "bdb",
    "binascii", "bisect", "builtins", "bz2", "cProfile", "calendar", "cgi",
    "cgitb", "chunk", "cmath", "cmd", "code", "codecs", "codeop", "collections",
    "colorsys", "compileall", "concurrent", "configparser", "contextlib",
    "contextvars", "copy", "copyreg", "crypt", "csv", "ctypes", "curses",
    "dataclasses", "datetime", "dbm", "decimal", "difflib", "dis", "distutils",
    "doctest", "email", "encodings", "ensurepip", "enum", "errno", "faulthandler",
    "fcntl", "filecmp", "fileinput", "fnmatch", "fractions", "ftplib",
    "functools", "gc", "genericpath", "getopt", "getpass", "gettext", "glob",
    "graphlib", "grp", "gzip", "hashlib", "heapq", "hmac", "html", "http",
    "idlelib", "imaplib", "imghdr", "imp", "importlib", "inspect", "io",
    "ipaddress", "itertools", "json", "keyword", "lib2to3", "linecache", "locale",
    "logging", "lzma", "mailbox", "mailcap", "marshal", "math", "mimetypes",
    "mmap", "modulefinder", "msilib", "msvcrt", "multiprocessing", "netrc", "nis",
    "nntplib", "ntpath", "nturl2path", "numbers", "opcode", "operator",
    "optparse", "os", "ossaudiodev", "pathlib", "pdb", "pickle", "pickletools",
    "pipes", "pkgutil", "platform", "plistlib", "poplib", "posix", "posixpath",
    "pprint", "profile", "pstats", "pty", "pwd", "py_compile", "pyclbr", "pydoc",
    "pydoc_data", "pyexpat", "queue", "quopri", "random", "re", "readline",
    "reprlib", "resource", "rlcompleter", "runpy", "sched", "secrets", "select",
    "selectors", "shelve", "shlex", "shutil", "signal", "site", "smtpd", "smtplib",
    "sndhdr", "socket", "socketserver", "spwd", "sqlite3", "sre_compile",
    "sre_constants", "sre_parse", "ssl", "stat", "statistics", "string",
    "stringprep", "struct", "subprocess", "sunau", "symtable", "sys",
    "sysconfig", "syslog", "tabnanny", "tarfile", "telnetlib", "tempfile",
    "termios", "test", "textwrap", "this", "threading", "time", "timeit",
    "tkinter", "token", "tokenize", "tomllib", "trace", "traceback",
    "tracemalloc", "tty", "turtle", "turtledemo", "types", "typing", "unicodedata",
    "unittest", "urllib", "uu", "uuid", "venv", "warnings", "wave", "weakref",
    "webbrowser", "winreg", "winsound", "wsgiref", "xdrlib", "xml", "xmlrpc",
    "zipapp", "zipfile", "zipimport", "zlib", "zoneinfo",
];

/// `is_stdlib` binary-searches the table above, which silently returns wrong
/// answers if it is ever out of order — an unsorted entry would make a real
/// stdlib module look like a third-party dependency. Checking at compile time
/// means a mis-ordered edit cannot be built, let alone shipped.
///
/// `str` comparison is not available in const context, so this compares bytes
/// directly; that matches `binary_search`'s ordering, which is bytewise for
/// `&str`.
const _: () = {
    /// Returns true when `a < b` bytewise.
    const fn lt(a: &str, b: &str) -> bool {
        let (a, b) = (a.as_bytes(), b.as_bytes());
        let mut i = 0;
        while i < a.len() && i < b.len() {
            if a[i] != b[i] {
                return a[i] < b[i];
            }
            i += 1;
        }
        a.len() < b.len()
    }

    let mut i = 1;
    while i < STDLIB_MODULES.len() {
        assert!(lt(STDLIB_MODULES[i - 1], STDLIB_MODULES[i]), "STDLIB_MODULES must stay sorted");
        i += 1;
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_are_sorted() {
        for pair in MAPPINGS.windows(2) {
            assert!(pair[0].0 < pair[1].0, "MAPPINGS not sorted: {} >= {}", pair[0].0, pair[1].0);
        }
    }

    #[test]
    fn known_mappings() {
        assert_eq!(resolve_distribution("cv2"), "opencv-python");
        assert_eq!(resolve_distribution("yaml"), "PyYAML");
        assert_eq!(resolve_distribution("PIL"), "Pillow");
        assert_eq!(resolve_distribution("sklearn"), "scikit-learn");
    }

    #[test]
    fn unmapped_returns_identity() {
        assert_eq!(resolve_distribution("requests"), "requests");
        assert_eq!(resolve_distribution("numpy"), "numpy");
    }

    // STDLIB_MODULES sortedness is asserted at compile time (see the
    // `const _` block above), so there is no runtime test for it.

    #[test]
    fn common_stdlib_names() {
        assert!(is_stdlib("os"));
        assert!(is_stdlib("sys"));
        assert!(is_stdlib("json"));
        assert!(is_stdlib("collections"));
        assert!(is_stdlib("typing"));
    }

    #[test]
    fn not_stdlib() {
        assert!(!is_stdlib("requests"));
        assert!(!is_stdlib("numpy"));
        assert!(!is_stdlib("flask"));
    }
}

fn plugin_create(toml: &toml::Value) -> Result<Box<dyn Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(RequirementsProcessor::new(cfg)))
}

inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "requirements",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<RequirementsConfig>,
        known_fields: crate::registries::typed_known_fields::<RequirementsConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<RequirementsConfig>,
        must_fields: crate::registries::typed_must_fields::<RequirementsConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<RequirementsConfig>,
        keywords: &["python", "pip", "requirements", "dependencies", "generator", "py"],
        description: "Generate requirements.txt from Python import statements",
        is_native: true,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
