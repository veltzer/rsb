//! Import name → `PyPI` distribution name mapping for the `requirements` generator.
//!
//! Most `PyPI` distributions use the same name as their top-level import — we
//! default to identity. This table lists the common exceptions where the
//! import name differs from the distribution name.
//!
//! Users can override these via the `mapping` config field; user entries win.

/// Resolve a Python import name to a `PyPI` distribution name using the curated
/// table. Returns the distribution name if the import is mapped, or the
/// original import name otherwise. Callers should consult the user's
/// configured mapping first.
pub fn resolve_distribution(import_name: &str) -> &str {
    MAPPINGS.binary_search_by_key(&import_name, |&(k, _)| k)
        .ok()
        .map(|i| MAPPINGS[i].1)
        .unwrap_or(import_name)
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
}
