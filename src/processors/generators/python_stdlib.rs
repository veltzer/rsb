//! Python stdlib module names for the `requirements` generator.
//!
//! Generated from `python3 -c 'import sys; print(sorted(sys.stdlib_module_names))'`
//! on Python 3.12. Covers 3.10+ (names added in later minor releases are included;
//! removed names are not present in older releases, which matches desired behavior).
//!
//! `is_stdlib` checks only the top-level name (`os.path` → `os`), which matches
//! how Python `sys.stdlib_module_names` is structured.

/// Returns true if the given top-level module name is part of the Python
/// stdlib. `module` should be the top-level name (e.g. "os" from "os.path").
pub fn is_stdlib(module: &str) -> bool {
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

    // Sortedness is asserted at compile time (see the `const _` block above),
    // so there is no runtime test for it.

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
