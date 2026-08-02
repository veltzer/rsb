//! External tool registry and installation engine.
//!
//! Split out of `processors/mod.rs`, which had accreted four unrelated
//! concerns (subprocess infra, this install engine, the tool table, and the
//! actual processor contract). Nothing here depends on the `Processor` trait:
//! this is the data and the mechanics of "what tools exist and how do we
//! install them", which `builder/tools.rs` orchestrates on top of.


/// Central registry of all known external tools — single source of truth for
/// runtime category and install command. Both `tool_install_command()` and
/// `tool_runtime()` (in `builder/tools.rs`) look up data from this table.
///
/// Runtime categories: "python", "node", "ruby", "rust", "perl", "jvm", "system"
///
/// A single way to install a tool.
pub struct InstallMethod {
    /// Package manager or method name (e.g., "pip", "apt", "npm", "snap", "cargo", "binary")
    pub method: &'static str,
    /// Package name for the package manager (e.g., "taplo-cli" for cargo, "texlive-latex-base" for apt)
    pub package: &'static str,
}

/// Context for executing an install: verbosity and eatmydata preference.
///
/// `sudo` is decided at execution time via `crate::platform::needs_sudo()`;
/// callers don't need to pass it in.
pub struct InstallCtx {
    /// When true, print each subprocess argv before running it.
    pub verbose: bool,
    /// When true, prepend `eatmydata` to system-package-manager calls
    /// (apt/dnf/pacman) for faster installs. No-op when eatmydata isn't
    /// on PATH.
    pub use_eatmydata: bool,
}

impl InstallMethod {
    /// Human-readable preview of what installing this entry's package
    /// would do — first line of the first describe step. Lossy for
    /// multi-step installs (binary fetches), but adequate for `tools list`.
    pub fn command(&self) -> String {
        let steps = describe(self.method, &[self.package]);
        steps.first().map_or_else(|| format!("({}) {}", self.method, self.package), |argv| join_argv(argv))
    }
}

/// Display the steps that `run(method, packages, ctx)` would execute,
/// without executing them. Used for logs and previews.
pub fn describe(method: &str, packages: &[&str]) -> Vec<Vec<String>> {
    let sudo = sudo_argv();
    let strs = |parts: &[&str]| -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    };
    let prefix = |head: &[&str], tail: &[&str]| -> Vec<String> {
        sudo.iter().chain(head.iter()).chain(tail.iter())
            .map(|s| (*s).to_string()).collect()
    };
    match method {
        "apt" => {
            // Run `apt-get update` first so the package index isn't stale.
            // CI runners ship with indexes a few days old; without this, an
            // `install` for a security-updated package 404s on the mirror.
            let update = prefix(&["apt-get", "update"], &[]);
            let mut install = prefix(&["apt-get", "install", "-y"], &[]);
            install.extend(packages.iter().map(|s| (*s).to_string()));
            vec![update, install]
        }
        "dnf" => {
            let mut argv = prefix(&["dnf", "install", "-y"], &[]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            vec![argv]
        }
        "pacman" => {
            let mut argv = prefix(&["pacman", "-S", "--noconfirm"], &[]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            vec![argv]
        }
        "brew" => {
            let mut argv = strs(&["brew", "install"]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            vec![argv]
        }
        "snap" => {
            let mut argv = prefix(&["snap", "install"], &[]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            vec![argv]
        }
        "pip"   => vec![{ let mut a = strs(&["pip", "install"]); a.extend(packages.iter().map(|s| (*s).to_string())); a }],
        "npm"   => vec![{ let mut a = strs(&["npm", "install", "-g"]); a.extend(packages.iter().map(|s| (*s).to_string())); a }],
        "cargo" => vec![{ let mut a = strs(&["cargo", "install"]); a.extend(packages.iter().map(|s| (*s).to_string())); a }],
        "gem"   => vec![{ let mut a = strs(&["gem", "install"]); a.extend(packages.iter().map(|s| (*s).to_string())); a }],
        "binary" => packages.iter().flat_map(|p| describe_binary(p)).collect(),
        "manual" => packages.iter()
            .map(|p| vec!["# manual:".to_string(), (*p).to_string()])
            .collect(),
        _ => packages.iter()
            .map(|p| vec![format!("# unknown method '{}':", method), (*p).to_string()])
            .collect(),
    }
}

/// Execute the install for `method` over `packages` (batched when the
/// method supports it). Each subprocess runs argv-style — never via a
/// shell. Returns Err on the first failure.
pub fn run(method: &str, packages: &[&str], ctx: &InstallCtx) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::process::Command;
    if packages.is_empty() {
        return Ok(());
    }
    let exec = |argv: &[String]| -> anyhow::Result<()> {
        if ctx.verbose {
            println!("Running: {}", join_argv(argv));
        }
        // Deliberately NOT routed through run_command: installers must
        // inherit the terminal, both so `sudo` can prompt for a password and
        // so apt/dnf can render progress. Capturing would hang on the
        // password prompt. This is one of the two documented exceptions to
        // the "everything goes through the central runner" rule (see
        // docs/src/internal/architecture.md).
        let status = Command::new(&argv[0]).args(&argv[1..]).status()
            .with_context(|| format!("failed to spawn: {}", join_argv(argv)))?;
        if !status.success() {
            anyhow::bail!(
                "{} exited with code {}",
                join_argv(argv),
                status.code().map_or_else(|| "unknown".to_string(), |c| c.to_string())
            );
        }
        Ok(())
    };
    let sudo = sudo_argv();
    let eatmydata: &[&str] = if ctx.use_eatmydata && which::which("eatmydata").is_ok() {
        &["eatmydata"]
    } else {
        &[]
    };
    let pkgmgr_argv = |head: &[&str]| -> Vec<String> {
        sudo.iter()
            .chain(eatmydata.iter())
            .chain(head.iter())
            .map(|s| (*s).to_string())
            .collect()
    };
    match method {
        "apt" => {
            // Refresh the package index before installing — CI runners ship
            // with stale indexes that 404 on security-updated packages.
            let update = pkgmgr_argv(&["apt-get", "update"]);
            exec(&update)?;
            let mut argv = pkgmgr_argv(&["apt-get", "install", "-y"]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "dnf" => {
            let mut argv = pkgmgr_argv(&["dnf", "install", "-y"]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "pacman" => {
            let mut argv = pkgmgr_argv(&["pacman", "-S", "--noconfirm"]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "brew" => {
            // brew is macOS-only; no sudo, no eatmydata.
            let mut argv = vec!["brew".to_string(), "install".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "snap" => {
            // snap doesn't need eatmydata.
            let mut argv: Vec<String> = sudo.iter().chain(["snap", "install"].iter())
                .map(|s| (*s).to_string()).collect();
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "pip" => {
            let mut argv = vec!["pip".to_string(), "install".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "npm" => {
            let mut argv = vec!["npm".to_string(), "install".to_string(), "-g".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "cargo" => {
            let mut argv = vec!["cargo".to_string(), "install".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "gem" => {
            let mut argv = vec!["gem".to_string(), "install".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "binary" => {
            for pkg in packages {
                run_binary(pkg, ctx)?;
            }
            Ok(())
        }
        "manual" => anyhow::bail!(
            "method '{}' is manual-only — install these packages by hand: {}",
            method, packages.join(", ")
        ),
        other => anyhow::bail!("unknown install method '{other}'"),
    }
}

fn sudo_argv() -> &'static [&'static str] {
    if crate::platform::needs_sudo() { &["sudo"] } else { &[] }
}

fn join_argv(argv: &[String]) -> String {
    argv.join(" ")
}

/// Description of the steps that `run_binary` would execute. Used for
/// preview/logging only.
fn describe_binary(pkg: &str) -> Vec<Vec<String>> {
    match binary_recipe(pkg) {
        // A .deb goes through apt, not the download/chmod/mv shape below: the
        // asset URL is resolved from the latest release at install time, so
        // it can only be described generically here.
        Some(BinaryRecipe { archive: ArchiveKind::Deb { source }, dest, .. }) => {
            let sudo = sudo_argv();
            let deb = format!("/tmp/{dest}.deb");
            let (note, url) = match source {
                DebSource::GithubRelease { repo, asset_pattern } => (
                    Some(format!("# resolve latest '{asset_pattern}' .deb asset from {repo}")),
                    "<resolved-asset-url>".to_string(),
                ),
                DebSource::Url(url) => (None, url.to_string()),
            };
            let mut steps: Vec<Vec<String>> = Vec::new();
            if let Some(note) = note {
                steps.push(vec![note]);
            }
            steps.push(vec!["curl".to_string(), "-fsSL".to_string(), "-o".to_string(),
                            deb.clone(), url]);
            steps.push(sudo.iter().chain(["apt-get", "install", "-y", &deb].iter())
                .map(|s| (*s).to_string()).collect());
            steps
        }
        Some(BinaryRecipe { url, archive, dest, .. }) => {
            let tmp = format!("/tmp/{dest}");
            let dl = format!("/tmp/{dest}.dl");
            let final_path = format!("/usr/local/bin/{dest}");
            let download = vec!["curl".to_string(), "-fsSL".to_string(), "-o".to_string(),
                                dl.clone(), url.to_string()];
            let extract = match archive {
                ArchiveKind::TarGz { inner } => vec![
                    "tar".to_string(), "-xzf".to_string(),
                    dl,
                    "-C".to_string(), "/tmp".to_string(),
                    inner.to_string(),
                ],
                ArchiveKind::Gunzip => vec![
                    "gunzip".to_string(), "-f".to_string(),
                    dl,
                ],
                ArchiveKind::Raw => vec![
                    "mv".to_string(), dl, tmp.clone(),
                ],
                ArchiveKind::Deb { .. } => unreachable!("matched by the Deb arm above"),
            };
            let chmod = vec!["chmod".to_string(), "+x".to_string(), tmp.clone()];
            let sudo = sudo_argv();
            let mv = sudo.iter().chain(["mv", &tmp, &final_path].iter())
                .map(|s| (*s).to_string()).collect();
            vec![download, extract, chmod, mv]
        }
        None => vec![vec![format!("# unknown binary recipe '{pkg}'")]],
    }
}

/// Resolve the browser-download URL of the newest release asset in `repo`
/// whose name contains `asset_pattern` and ends in `.deb`. Resolving at
/// install time (instead of pinning a URL in the recipe) is what keeps the
/// recipe from breaking every time upstream cuts a release.
fn resolve_latest_deb_asset(repo: &str, asset_pattern: &str) -> anyhow::Result<String> {
    use anyhow::Context as _;
    let api = format!("https://api.github.com/repos/{repo}/releases/latest");
    let body = ureq::get(&api)
        // GitHub rejects API requests without a User-Agent.
        .header("User-Agent", "rsconstruct")
        .call()
        .with_context(|| format!("Failed to query GitHub releases API for {repo}"))?
        .body_mut()
        .read_to_string()
        .with_context(|| format!("Failed to read GitHub releases response for {repo}"))?;
    let release: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| format!("Failed to parse GitHub releases JSON for {repo}"))?;
    release["assets"].as_array()
        .with_context(|| format!("GitHub release for {repo} has no 'assets' array"))?
        .iter()
        .find_map(|a| {
            let name = a["name"].as_str()?;
            let is_deb = std::path::Path::new(name).extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("deb"));
            (name.contains(asset_pattern) && is_deb)
                .then(|| a["browser_download_url"].as_str())?
                .map(std::string::ToString::to_string)
        })
        .with_context(|| format!(
            "No .deb asset matching '{asset_pattern}' in the latest {repo} release"
        ))
}

fn run_binary(pkg: &str, ctx: &InstallCtx) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::process::Command;
    let recipe = binary_recipe(pkg)
        .ok_or_else(|| anyhow::anyhow!("no binary install recipe for '{pkg}'"))?;
    let download = format!("/tmp/{}.dl", recipe.dest);
    let final_tmp = format!("/tmp/{}", recipe.dest);
    let final_path = format!("/usr/local/bin/{}", recipe.dest);
    let exec = |argv: &[&str]| -> anyhow::Result<()> {
        if ctx.verbose {
            println!("Running: {}", argv.join(" "));
        }
        // Same exception as `run()` above: the binary installer shells out to
        // `sudo install`, which needs the terminal for its password prompt.
        let status = Command::new(argv[0]).args(&argv[1..]).status()
            .with_context(|| format!("failed to spawn: {}", argv.join(" ")))?;
        if !status.success() {
            anyhow::bail!(
                "{} exited with code {}",
                argv.join(" "),
                status.code().map_or_else(|| "unknown".to_string(), |c| c.to_string())
            );
        }
        Ok(())
    };
    // A .deb is installed by apt, so it skips the download/chmod/mv shape
    // entirely: apt pulls the dependency tree the bare binary would be
    // missing, and the payload is not a single executable.
    if let ArchiveKind::Deb { source } = recipe.archive {
        let deb = format!("/tmp/{}.deb", recipe.dest);
        let asset_url = match source {
            DebSource::GithubRelease { repo, asset_pattern } => {
                resolve_latest_deb_asset(repo, asset_pattern)?
            }
            DebSource::Url(url) => url.to_string(),
        };
        exec(&["curl", "-fsSL", "-o", &deb, &asset_url])?;
        let sudo = sudo_argv();
        let mut install: Vec<&str> = sudo.to_vec();
        install.extend(["apt-get", "install", "-y", &deb]);
        let result = exec(&install);
        std::fs::remove_file(&deb).ok();
        return result;
    }
    exec(&["curl", "-fsSL", "-o", &download, recipe.url])?;
    match recipe.archive {
        ArchiveKind::Deb { .. } => unreachable!("returned early by the Deb branch above"),
        ArchiveKind::TarGz { inner } => {
            exec(&["tar", "-xzf", &download, "-C", "/tmp", inner])?;
            // After tar, the inner file is at /tmp/<inner>; rename to final_tmp.
            let inner_path = format!("/tmp/{inner}");
            if inner_path != final_tmp {
                std::fs::rename(&inner_path, &final_tmp)
                    .with_context(|| format!("rename {inner_path} -> {final_tmp}"))?;
            }
            std::fs::remove_file(&download).ok();
        }
        ArchiveKind::Gunzip => {
            // gunzip strips the .gz extension. Our download path ends in
            // .dl; rename to .dl.gz so gunzip leaves /tmp/<dest>.dl, then
            // move to final_tmp.
            let gz = format!("{download}.gz");
            std::fs::rename(&download, &gz).with_context(|| format!("rename {download} -> {gz}"))?;
            exec(&["gunzip", "-f", &gz])?;
            std::fs::rename(&download, &final_tmp)
                .with_context(|| format!("rename {download} -> {final_tmp}"))?;
        }
        ArchiveKind::Raw => {
            std::fs::rename(&download, &final_tmp)
                .with_context(|| format!("rename {download} -> {final_tmp}"))?;
        }
    }
    crate::platform::set_permissions_mode(std::path::Path::new(&final_tmp), 0o755)
        .with_context(|| format!("chmod +x {final_tmp}"))?;
    let sudo = sudo_argv();
    let mut mv: Vec<&str> = sudo.to_vec();
    mv.extend(["mv", &final_tmp, &final_path]);
    exec(&mv)
}

struct BinaryRecipe {
    url: &'static str,
    archive: ArchiveKind,
    dest: &'static str,
}

enum ArchiveKind {
    TarGz { inner: &'static str },
    Gunzip,
    /// The download is the binary itself, no extraction needed.
    Raw,
    /// The download is a `.deb` installed with apt, not a bare binary dropped
    /// into /usr/local/bin: apt pulls the dependency tree a raw binary would
    /// be missing.
    ///
    /// `source` says where the `.deb` comes from — a GitHub release whose
    /// asset is resolved at install time (so the recipe doesn't pin a version
    /// that goes stale the moment upstream cuts a release), or a vendor URL
    /// that is already stable.
    Deb { source: DebSource },
}

/// Where a [`ArchiveKind::Deb`] payload is fetched from.
enum DebSource {
    /// Newest asset in `repo`'s latest release whose name contains
    /// `asset_pattern` and ends in `.deb`.
    GithubRelease { repo: &'static str, asset_pattern: &'static str },
    /// A vendor URL that always points at the current build.
    Url(&'static str),
}

fn binary_recipe(pkg: &str) -> Option<BinaryRecipe> {
    match pkg {
        "rumdl" => Some(BinaryRecipe {
            url: "https://github.com/rvben/rumdl/releases/download/v0.1.81/rumdl-v0.1.81-x86_64-unknown-linux-gnu.tar.gz",
            archive: ArchiveKind::TarGz { inner: "rumdl" },
            dest: "rumdl",
        }),
        "taplo" => Some(BinaryRecipe {
            url: "https://github.com/tamasfe/taplo/releases/latest/download/taplo-linux-x86_64.gz",
            archive: ArchiveKind::Gunzip,
            dest: "taplo",
        }),
        "hadolint" => Some(BinaryRecipe {
            url: "https://github.com/hadolint/hadolint/releases/latest/download/hadolint-Linux-x86_64",
            archive: ArchiveKind::Raw,
            dest: "hadolint",
        }),
        // checkpatch.pl ships only inside the kernel tree — there is no release
        // artifact and no distro package. Fetching the single script from
        // torvalds/linux master is the automatable equivalent of "copy it out
        // of a kernel checkout", and keeps the tool out of the manual-only
        // bucket that `tools install --all` rejects.
        "checkpatch.pl" => Some(BinaryRecipe {
            url: "https://raw.githubusercontent.com/torvalds/linux/master/scripts/checkpatch.pl",
            archive: ArchiveKind::Raw,
            dest: "checkpatch.pl",
        }),
        // drawio ships an official .deb but has no apt repo, and its snap
        // needs a running snapd — unavailable in containers and flaky on CI
        // runners, which made this the one tool `--all` could not provision.
        "drawio" => Some(BinaryRecipe {
            url: "https://github.com/jgraph/drawio-desktop/releases/latest",
            archive: ArchiveKind::Deb {
                source: DebSource::GithubRelease {
                    repo: "jgraph/drawio-desktop",
                    asset_pattern: "amd64",
                },
            },
            dest: "drawio",
        }),
        // Chrome is not in the Ubuntu archive — `apt install
        // google-chrome-stable` only works after adding Google's repo. The
        // vendor .deb is the official route and is not version-pinned.
        "google-chrome-stable" => Some(BinaryRecipe {
            url: "https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb",
            archive: ArchiveKind::Deb {
                source: DebSource::Url(
                    "https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb",
                ),
            },
            dest: "google-chrome",
        }),
        _ => None,
    }
}

/// Information about an external tool: its name, runtime category, and install methods.
pub struct ToolInfo {
    /// Tool binary name
    pub name: &'static str,
    /// Runtime category ("python", "node", "ruby", "rust", "perl", "jvm", "system")
    pub runtime: &'static str,
    /// Install methods, ordered by preference (first is the default)
    pub install_methods: &'static [InstallMethod],
}

pub static TOOLS: &[ToolInfo] = &[
    // Python tools
    ToolInfo { name: "ruff", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "ruff" }] },
    ToolInfo { name: "pylint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "pylint" }] },
    ToolInfo { name: "mypy", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "mypy" }] },
    ToolInfo { name: "pyrefly", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "pyrefly" }] },
    ToolInfo { name: "yamllint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "yamllint" }] },
    ToolInfo { name: "sphinx-build", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "sphinx" }] },
    ToolInfo { name: "pip", runtime: "python", install_methods: &[InstallMethod { method: "apt", package: "python3-pip" }] },
    ToolInfo { name: "jsonlint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "demjson3" }] },
    ToolInfo { name: "cpplint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "cpplint" }] },
    ToolInfo { name: "black", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "black" }] },
    ToolInfo { name: "pytest", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "pytest" }] },
    ToolInfo { name: "a2x", runtime: "python", install_methods: &[InstallMethod { method: "apt", package: "asciidoc" }] },
    // The mako processor runs `python3 -c "import mako..."`, so what it really
    // needs is the Mako *library*, which the registry can't probe directly (it
    // probes executables). The `mako-render` console script ships in the same
    // distribution, so probing it is an exact proxy for "the library is
    // importable", and installing it installs the library.
    ToolInfo { name: "mako-render", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "mako" }] },
    ToolInfo { name: "python3", runtime: "python", install_methods: &[InstallMethod { method: "apt", package: "python3" }] },
    // Node tools
    ToolInfo { name: "marp", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "@marp-team/marp-cli" }] },
    ToolInfo { name: "mmdc", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "@mermaid-js/mermaid-cli" }] },
    ToolInfo { name: "node_modules/.bin/markdownlint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "markdownlint-cli" }] },
    ToolInfo { name: "markdownlint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "markdownlint-cli" }] },
    ToolInfo { name: "prettier", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "prettier" }] },
    ToolInfo { name: "eslint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "eslint" }] },
    ToolInfo { name: "htmlhint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "htmlhint" }] },
    ToolInfo { name: "jshint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "jshint" }] },
    ToolInfo { name: "npm", runtime: "node", install_methods: &[InstallMethod { method: "apt", package: "npm" }] },
    ToolInfo { name: "node", runtime: "node", install_methods: &[InstallMethod { method: "apt", package: "nodejs" }] },
    // Ruby tools
    ToolInfo { name: "gems/bin/mdl", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "mdl" }] },
    ToolInfo { name: "mdl", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "mdl" }] },
    ToolInfo { name: "bundle", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "bundler" }] },
    ToolInfo { name: "ruby", runtime: "ruby", install_methods: &[InstallMethod { method: "apt", package: "ruby" }] },
    // Rust tools
    ToolInfo { name: "mdbook", runtime: "rust", install_methods: &[InstallMethod { method: "cargo", package: "mdbook" }] },
    ToolInfo { name: "rumdl", runtime: "rust", install_methods: &[
        InstallMethod { method: "binary", package: "rumdl" },
        InstallMethod { method: "cargo", package: "rumdl" },
    ]},
    ToolInfo { name: "taplo", runtime: "rust", install_methods: &[
        InstallMethod { method: "binary", package: "taplo" },
        InstallMethod { method: "cargo", package: "taplo-cli" },
    ]},
    // rustup is the canonical route, but `apt install rustc/cargo` is a real,
    // automatable install and is what makes these reachable from
    // `tools install --all` on a bare machine. A host that already has a
    // rustup toolchain never reaches the install path: `which` finds these
    // first.
    ToolInfo { name: "cargo", runtime: "rust", install_methods: &[InstallMethod { method: "apt", package: "cargo" }] },
    ToolInfo { name: "rustc", runtime: "rust", install_methods: &[InstallMethod { method: "apt", package: "rustc" }] },
    // Perl tools
    ToolInfo { name: "perl", runtime: "perl", install_methods: &[InstallMethod { method: "apt", package: "perl" }] },
    ToolInfo { name: "markdown", runtime: "perl", install_methods: &[InstallMethod { method: "apt", package: "markdown" }] },
    ToolInfo { name: "checkpatch.pl", runtime: "perl", install_methods: &[InstallMethod { method: "binary", package: "checkpatch.pl" }] },
    // System tools
    ToolInfo { name: "shellcheck", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "shellcheck" }] },
    ToolInfo { name: "luacheck", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "lua-check" }] },
    ToolInfo { name: "cppcheck", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "cppcheck" }] },
    ToolInfo { name: "clang-tidy", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "clang-tidy" }] },
    ToolInfo { name: "gcc", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "gcc" }] },
    ToolInfo { name: "g++", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "g++" }] },
    ToolInfo { name: "clang", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "clang" }] },
    ToolInfo { name: "clang++", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "clang" }] },
    ToolInfo { name: "ar", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "binutils" }] },
    ToolInfo { name: "make", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "make" }] },
    ToolInfo { name: "jq", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "jq" }] },
    ToolInfo { name: "aspell", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "aspell" }] },
    ToolInfo { name: "pandoc", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "pandoc" }] },
    ToolInfo { name: "pdflatex", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "texlive-latex-base" }] },
    ToolInfo { name: "qpdf", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "qpdf" }] },
    ToolInfo { name: "dot", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "graphviz" }] },
    ToolInfo { name: "drawio", runtime: "system", install_methods: &[InstallMethod { method: "binary", package: "drawio" }] },
    ToolInfo { name: "libreoffice", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "libreoffice" }] },
    ToolInfo { name: "flock", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "util-linux" }] },
    ToolInfo { name: "uname", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "coreutils" }] },
    ToolInfo { name: "sh", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "dash" }] },
    ToolInfo { name: "git", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "git" }] },
    ToolInfo { name: "pdfunite", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "poppler-utils" }] },
    ToolInfo { name: "google-chrome", runtime: "system", install_methods: &[InstallMethod { method: "binary", package: "google-chrome-stable" }] },
    ToolInfo { name: "objdump", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "binutils" }] },
    ToolInfo { name: "tidy", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "tidy" }] },
    ToolInfo { name: "xmllint", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "libxml2-utils" }] },
    ToolInfo { name: "clojure", runtime: "jvm", install_methods: &[
        // Debian/Ubuntu package the Clojure CLI, which is what makes this
        // installable without the official shell installer; brew is the macOS
        // route. See https://clojure.org/guides/install_clojure
        InstallMethod { method: "apt", package: "clojure" },
        InstallMethod { method: "brew", package: "clojure/tools/clojure" },
    ]},
    ToolInfo { name: "svglint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "svglint" }] },
    ToolInfo { name: "svgo", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "svgo" }] },
    ToolInfo { name: "cmakelint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "cmakelint" }] },
    ToolInfo { name: "protoc", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "protobuf-compiler" }] },
    ToolInfo { name: "sass", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "sass" }] },
    ToolInfo { name: "hadolint", runtime: "system", install_methods: &[
        InstallMethod { method: "binary", package: "hadolint" },
        InstallMethod { method: "brew", package: "hadolint" },
        InstallMethod { method: "apt", package: "hadolint" },
    ]},
    ToolInfo { name: "php", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "php-cli" }] },
    ToolInfo { name: "checkstyle", runtime: "jvm", install_methods: &[InstallMethod { method: "apt", package: "checkstyle" }] },
    ToolInfo { name: "yq", runtime: "system", install_methods: &[
        InstallMethod { method: "pip", package: "yq" },
        InstallMethod { method: "snap", package: "yq" },
        InstallMethod { method: "apt", package: "yq" },
    ]},
    // Node tools (additional)
    ToolInfo { name: "stylelint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "stylelint" }] },
    ToolInfo { name: "jslint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "jslint" }] },
    ToolInfo { name: "standard", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "standard" }] },
    ToolInfo { name: "htmllint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "htmllint-cli" }] },
    ToolInfo { name: "slidev", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "@slidev/cli" }] },
    // Perl tools (additional)
    ToolInfo { name: "perlcritic", runtime: "perl", install_methods: &[InstallMethod { method: "apt", package: "libperl-critic-perl" }] },
    // Ruby tools (additional)
    ToolInfo { name: "jekyll", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "jekyll" }] },
    // Built-in / coreutils
    ToolInfo { name: "true", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "coreutils" }] },
];

/// Look up a tool by name.
pub fn tool_info(tool: &str) -> Option<&'static ToolInfo> {
    TOOLS.iter().find(|t| t.name == tool)
}

/// Return the default install command for a tool, if known.
pub fn tool_install_command(tool: &str) -> Option<String> {
    tool_info(tool).and_then(|t| t.install_methods.first().map(InstallMethod::command))
}

/// Return the runtime category for a tool, if known.
pub fn tool_runtime(tool: &str) -> Option<&'static str> {
    tool_info(tool).map(|t| t.runtime)
}


#[cfg(test)]
mod tests {
    use super::*;

    /// `sudo_argv()` returns either `["sudo"]` or `[]` based on runtime
    /// state. Empty when running as root or when `sudo` isn't on PATH.
    #[test]
    fn sudo_argv_matches_runtime() {
        let prefix = sudo_argv();
        if crate::platform::needs_sudo() {
            assert_eq!(prefix, &["sudo"]);
        } else {
            assert!(prefix.is_empty());
        }
    }

    /// `apt` describe must run `apt-get update` first, then call
    /// `apt-get install -y <pkg>` exactly once with the package name
    /// after the flags. Whether `sudo` is prepended depends on runtime
    /// state.
    #[test]
    fn apt_describe_shape_is_correct() {
        let steps = describe("apt", &["cowsay"]);
        assert_eq!(steps.len(), 2);

        let update = &steps[0];
        let upd_idx = update.iter().position(|s| s == "apt-get").expect("apt-get in update argv");
        assert_eq!(update[upd_idx + 1], "update");
        assert_eq!(update.len(), upd_idx + 2);

        let install = &steps[1];
        let pkgmgr_idx = install.iter().position(|s| s == "apt-get").expect("apt-get in install argv");
        assert_eq!(install[pkgmgr_idx + 1], "install");
        assert_eq!(install[pkgmgr_idx + 2], "-y");
        assert_eq!(install[pkgmgr_idx + 3], "cowsay");
        if install[0] == "sudo" {
            assert_eq!(pkgmgr_idx, 1);
        } else {
            assert_eq!(pkgmgr_idx, 0);
        }
    }

    /// `apt` batch describe collapses many packages into one apt-get
    /// install call (still preceded by a single `apt-get update`).
    #[test]
    fn apt_batch_describe_collapses() {
        let steps = describe("apt", &["foo", "bar", "baz"]);
        assert_eq!(steps.len(), 2);
        let install = &steps[1];
        let pkgmgr_idx = install.iter().position(|s| s == "apt-get").expect("apt-get in argv");
        assert_eq!(&install[pkgmgr_idx + 3..], &["foo", "bar", "baz"]);
    }

    /// `pip` describe never uses sudo, regardless of runtime state.
    #[test]
    fn pip_describe_never_has_sudo() {
        let steps = describe("pip", &["ruff"]);
        assert_eq!(steps.len(), 1);
        let argv = &steps[0];
        assert_eq!(argv[0], "pip");
        assert!(!argv.contains(&"sudo".to_string()));
    }

    /// `binary` describe yields a multi-step plan (download, extract,
    /// chmod, mv) with no shell metacharacters anywhere.
    #[test]
    fn binary_describe_has_no_shell_metachars() {
        for pkg in &["taplo", "rumdl"] {
            let steps = describe("binary", &[pkg]);
            assert!(steps.len() >= 3, "binary {pkg} should have >=3 steps");
            for step in &steps {
                for arg in step {
                    for forbidden in &['|', '>', '<', ';', '&'] {
                        assert!(
                            !arg.contains(*forbidden),
                            "binary {pkg} step contains shell metachar '{forbidden}': {arg:?}"
                        );
                    }
                }
            }
        }
    }
}
