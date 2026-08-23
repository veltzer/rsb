//! External tool registry and installation engine.
//!
//! Lives at the crate root because nothing here depends on the `Processor`
//! trait, and every file under `src/processors/` must be a real processor.
//! This is the data and the mechanics of "what tools exist and how do we
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
        "gem"   => vec![{ let mut a = gem_install_argv(); a.extend(packages.iter().map(|s| (*s).to_string())); a }],
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
            let mut argv = gem_install_argv();
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
            // The bin dir a `--user-install` just created becomes visible to
            // the NEXT rsconstruct invocation, via the PATH augmentation at
            // startup. Nothing in this process probes for the tool after
            // installing it, and re-augmenting here would mutate the
            // environment with tokio threads alive (see platform::set_env).
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

/// The argv prefix for installing gems. Appends `--user-install` when a
/// plain `gem install` would die on an unwritable default gem dir — the
/// apt-ruby case, where gems go to root-owned `/var/lib/gems` and rsconstruct
/// deliberately doesn't sudo-wrap language package managers. Used by both
/// `describe` and `run` so the printed plan matches what executes.
fn gem_install_argv() -> Vec<String> {
    let mut argv = vec!["gem".to_string(), "install".to_string()];
    if gem_needs_user_install() {
        argv.push("--user-install".to_string());
    }
    argv
}

/// Whether the default gem dir is unwritable for the current user (never
/// true for root). Asks `gem env gemdir` once per process; a missing or
/// broken `gem` reports false and lets the install fail with gem's own
/// message.
fn gem_needs_user_install() -> bool {
    static NEEDS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *NEEDS.get_or_init(|| {
        if crate::platform::is_root() {
            return false;
        }
        let Ok(out) = std::process::Command::new("gem").args(["env", "gemdir"]).output() else {
            return false;
        };
        if !out.status.success() {
            return false;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let gemdir = std::path::Path::new(stdout.trim());
        if gemdir.as_os_str().is_empty() {
            return false;
        }
        !nearest_existing_ancestor_is_writable(gemdir)
    })
}

/// `access(W_OK)` on the path, or — when it doesn't exist yet — on its
/// nearest existing ancestor, which is what decides whether the path could
/// be created.
fn nearest_existing_ancestor_is_writable(path: &std::path::Path) -> bool {
    let mut p = path;
    loop {
        if p.exists() {
            return crate::platform::path_is_writable(p);
        }
        match p.parent() {
            Some(parent) => p = parent,
            None => return false,
        }
    }
}

/// Make user-installed gem executables resolvable: append the user gem bin
/// dirs that exist to this process's PATH, so `which`-based tool probes and
/// every spawned child (tools, processor scripts) see them. `gem install
/// --user-install` puts binaries in a dir that is on nobody's PATH by
/// default, which would otherwise leave a just-installed tool reported as
/// missing.
///
/// Called once at the top of `main()`, before any thread exists — the
/// safety condition of `platform::set_env`.
pub fn augment_path_with_user_gem_bins() {
    let Some(home) = std::env::var_os("HOME") else { return };
    let gem_home = std::env::var_os("GEM_HOME");
    let dirs = user_gem_bin_dirs(std::path::Path::new(&home), gem_home.as_deref());
    if dirs.is_empty() {
        return;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(new_path) = path_with_appended_dirs(&path, &dirs) {
        crate::platform::set_env("PATH", &new_path);
    }
}

/// Existing user-level gem executable dirs: `$GEM_HOME/bin` plus the
/// per-ruby-version `bin` dirs of both user-install layouts —
/// `~/.gem/ruby/<version>/bin` (upstream rubygems) and
/// `~/.local/share/gem/ruby/<version>/bin` (the Debian/Ubuntu XDG patch).
/// Only dirs that exist are returned; globbing the layouts costs two
/// readdirs and avoids spawning ruby at every startup.
fn user_gem_bin_dirs(home: &std::path::Path, gem_home: Option<&std::ffi::OsStr>) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(gem_home) = gem_home {
        let bin = std::path::Path::new(gem_home).join("bin");
        if bin.is_dir() {
            dirs.push(bin);
        }
    }
    for base in [home.join(".gem/ruby"), home.join(".local/share/gem/ruby")] {
        let Ok(entries) = std::fs::read_dir(&base) else { continue };
        let mut versions: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("bin"))
            .filter(|p| p.is_dir())
            .collect();
        versions.sort();
        dirs.append(&mut versions);
    }
    dirs
}

/// `path` with the dirs not already on it appended, or None when every dir
/// is already present. Append rather than prepend: a system-installed tool
/// keeps winning over a user gem of the same name.
fn path_with_appended_dirs(path: &std::ffi::OsStr, dirs: &[std::path::PathBuf]) -> Option<std::ffi::OsString> {
    let existing: Vec<std::path::PathBuf> = std::env::split_paths(path).collect();
    let missing: Vec<std::path::PathBuf> = dirs.iter()
        .filter(|d| !existing.contains(d))
        .cloned()
        .collect();
    if missing.is_empty() {
        return None;
    }
    // join_paths only errors on an entry containing the separator, which
    // can't happen: every entry came from split_paths or is a real dir.
    std::env::join_paths(existing.into_iter().chain(missing)).ok()
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
            steps.push(crate::download::curl_argv(&url, &deb));
            steps.push(sudo.iter().chain(["apt-get", "install", "-y", &deb].iter())
                .map(|s| (*s).to_string()).collect());
            steps
        }
        Some(BinaryRecipe { url, archive, dest, .. }) => {
            let tmp = format!("/tmp/{dest}");
            let dl = format!("/tmp/{dest}.dl");
            let final_path = format!("/usr/local/bin/{dest}");
            let download = crate::download::curl_argv(url, &dl);
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
    // Retried via download::with_retry for the same reason downloads are:
    // a connection reset here fails the whole install. See
    // docs/src/internal/download-policy.md.
    let body = crate::download::with_retry(|| {
        ureq::get(&api)
            // GitHub rejects API requests without a User-Agent.
            .header("User-Agent", "rsconstruct")
            .call()
            .with_context(|| format!("Failed to query GitHub releases API for {repo}"))?
            .body_mut()
            .read_to_string()
            .with_context(|| format!("Failed to read GitHub releases response for {repo}"))
    })?;
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
        let dl = crate::download::curl_argv(&asset_url, &deb);
        exec(&dl.iter().map(String::as_str).collect::<Vec<_>>())?;
        let sudo = sudo_argv();
        let mut install: Vec<&str> = sudo.to_vec();
        install.extend(["apt-get", "install", "-y", &deb]);
        let result = exec(&install);
        std::fs::remove_file(&deb).ok();
        return result;
    }
    let dl = crate::download::curl_argv(recipe.url, &download);
    exec(&dl.iter().map(String::as_str).collect::<Vec<_>>())?;
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
        // Pinned rather than tracking latest, unlike taplo/hadolint below.
        // zola 0.23 renamed config keys (highlight_code -> [markdown.highlighting])
        // and swapped the syntax highlighter, so an unpinned upgrade can fail a
        // site build on config alone -- a breakage that lands in the consuming
        // project's CI, not here. Bump this deliberately.
        "zola" => Some(BinaryRecipe {
            url: "https://github.com/getzola/zola/releases/download/v0.23.3/zola-v0.23.3-x86_64-unknown-linux-gnu.tar.gz",
            archive: ArchiveKind::TarGz { inner: "zola" },
            dest: "zola",
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
    /// Bare binary name, as found on `$PATH` — never a path.
    ///
    /// This is the detection key: it goes straight to `which::which` (see
    /// `builder/tools.rs` and `tool_lock.rs`), so it decides whether a tool
    /// reports `installed` or `missing`, and which binary gets version-locked.
    ///
    /// `which` switches behaviour on the presence of a separator, so a name
    /// containing `/` is NOT looked up on `$PATH` — it is resolved relative to
    /// the *current working directory*. A vendored-looking entry such as
    /// `gems/bin/mdl` therefore only resolves when rsconstruct happens to be
    /// invoked from the one directory with that subtree beneath it, and reports
    /// `missing` everywhere else — including for a user who has the tool
    /// installed and on `$PATH` at, say, `~/install/gems/bin/mdl`. Two such
    /// entries were removed for exactly this reason; keep names bare.
    ///
    /// In both modes `which` also requires the executable bit, so a present but
    /// non-executable file reads as `missing`.
    ///
    /// To pin a project-local binary, set the per-processor `command` config
    /// field (`config/processor_configs.rs`) instead — that is the value that
    /// actually gets executed. This field stays a bare name so detection works
    /// from any cwd.
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
    ToolInfo { name: "markdownlint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "markdownlint-cli" }] },
    ToolInfo { name: "prettier", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "prettier" }] },
    ToolInfo { name: "eslint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "eslint" }] },
    ToolInfo { name: "htmlhint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "htmlhint" }] },
    ToolInfo { name: "jshint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "jshint" }] },
    ToolInfo { name: "npm", runtime: "node", install_methods: &[InstallMethod { method: "apt", package: "npm" }] },
    ToolInfo { name: "node", runtime: "node", install_methods: &[InstallMethod { method: "apt", package: "nodejs" }] },
    // Ruby tools
    ToolInfo { name: "mdl", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "mdl" }] },
    ToolInfo { name: "bundle", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "bundler" }] },
    ToolInfo { name: "ruby", runtime: "ruby", install_methods: &[InstallMethod { method: "apt", package: "ruby" }] },
    // Rust tools
    ToolInfo { name: "mdbook", runtime: "rust", install_methods: &[InstallMethod { method: "cargo", package: "mdbook" }] },
    ToolInfo { name: "rumdl", runtime: "rust", install_methods: &[
        InstallMethod { method: "binary", package: "rumdl" },
        InstallMethod { method: "cargo", package: "rumdl" },
    ]},
    // Binary only, no cargo fallback: upstream does not publish zola to
    // crates.io. The `zola` crate there is an unrelated squatter -- version
    // 0.0.0, yanked, described as "zola installer" -- so `cargo install zola`
    // would fail or fetch the wrong thing. The GitHub release is the only
    // automatable route.
    ToolInfo { name: "zola", runtime: "rust", install_methods: &[
        InstallMethod { method: "binary", package: "zola" },
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

// Processor files may submit additional ToolInfo entries via
// `inventory::submit!` — a new processor declares its external tool in its
// own file instead of editing this registry. The central TOOLS table above
// keeps the long-standing shared entries; consumers must iterate
// `all_tools()`, never TOOLS directly.
inventory::collect!(ToolInfo);

/// Every known tool: the central table plus processor-file submissions.
pub fn all_tools() -> impl Iterator<Item = &'static ToolInfo> {
    TOOLS.iter().chain(inventory::iter::<ToolInfo>)
}

/// Look up a tool by name.
pub fn tool_info(tool: &str) -> Option<&'static ToolInfo> {
    all_tools().find(|t| t.name == tool)
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    /// A tool name must have exactly one registry entry — a processor
    /// submitting a name the central table already carries (or two
    /// processors submitting the same name) would make `tool_info`'s answer
    /// depend on iteration order.
    #[test]
    fn tool_names_are_unique_across_central_and_submitted() {
        let mut seen = std::collections::HashSet::new();
        let mut dups: Vec<&str> = Vec::new();
        for t in all_tools() {
            if !seen.insert(t.name) {
                dups.push(t.name);
            }
        }
        dups.sort_unstable();
        assert!(dups.is_empty(), "duplicate tool registry entries: {dups:?}");
    }
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

    /// Only bin dirs that actually exist are collected, from all three
    /// layouts: `$GEM_HOME/bin`, `~/.gem/ruby/<v>/bin` (upstream) and
    /// `~/.local/share/gem/ruby/<v>/bin` (Debian/Ubuntu).
    #[test]
    fn user_gem_bin_dirs_collects_existing_layouts() {
        let home = tempfile::TempDir::new().unwrap();
        assert!(user_gem_bin_dirs(home.path(), None).is_empty());

        let upstream = home.path().join(".gem/ruby/3.2.0/bin");
        let debian = home.path().join(".local/share/gem/ruby/3.3.0/bin");
        let gem_home = home.path().join("mygems");
        std::fs::create_dir_all(&upstream).unwrap();
        std::fs::create_dir_all(&debian).unwrap();
        std::fs::create_dir_all(gem_home.join("bin")).unwrap();
        // A version dir without bin/ must not be collected.
        std::fs::create_dir_all(home.path().join(".gem/ruby/2.7.0")).unwrap();

        let dirs = user_gem_bin_dirs(home.path(), Some(gem_home.as_os_str()));
        assert_eq!(dirs, vec![gem_home.join("bin"), upstream, debian]);
    }

    /// Appending keeps the original PATH order, adds only what's missing,
    /// and reports None when there is nothing to add.
    #[test]
    fn path_with_appended_dirs_appends_only_missing() {
        use std::path::PathBuf;
        let path = std::ffi::OsString::from("/usr/bin:/home/u/.gem/ruby/3.2.0/bin");
        let dirs = vec![
            PathBuf::from("/home/u/.gem/ruby/3.2.0/bin"),
            PathBuf::from("/home/u/mygems/bin"),
        ];
        let new_path = path_with_appended_dirs(&path, &dirs).unwrap();
        assert_eq!(new_path, "/usr/bin:/home/u/.gem/ruby/3.2.0/bin:/home/u/mygems/bin");

        assert!(path_with_appended_dirs(&new_path, &dirs).is_none());
        assert!(path_with_appended_dirs(&path, &[]).is_none());
    }

    /// A missing path is judged by its nearest existing ancestor — that is
    /// what decides whether the path could be created.
    #[test]
    fn ancestor_writability_walks_to_existing_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(nearest_existing_ancestor_is_writable(&dir.path().join("a/b/c")));
        if !crate::platform::is_root() {
            crate::platform::set_permissions_mode(dir.path(), 0o555).unwrap();
            assert!(!nearest_existing_ancestor_is_writable(&dir.path().join("a/b/c")));
            crate::platform::set_permissions_mode(dir.path(), 0o755).unwrap();
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
        for pkg in &["taplo", "rumdl", "zola"] {
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
