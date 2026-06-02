use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use anyhow::{Context, Result, bail};
use crate::color;
use crate::config::SymlinkInstallConfig;

/// Execute the symlink-install command.
/// For each configured source→target pair, symlinks all files recursively.
pub fn run(config: &SymlinkInstallConfig) -> Result<()> {
    if config.sources.is_empty() {
        bail!("No symlink_install paths configured.\n\
               Add sources and targets arrays to [command.symlink_install] in rsconstruct.toml.");
    }
    if config.sources.len() != config.targets.len() {
        bail!("symlink_install: sources ({}) and targets ({}) must have the same length.",
            config.sources.len(), config.targets.len());
    }

    let mut total_created = 0usize;
    let mut total_updated = 0usize;
    let mut total_unchanged = 0usize;

    for (source, target) in config.sources.iter().zip(&config.targets) {
        let target = expand_tilde(target);
        println!("{} → {}", color::bold(source), color::bold(&target));
        let (c, u, n) = install_dir(Path::new(source), Path::new(&target), Path::new(source))?;
        total_created += c;
        total_updated += u;
        total_unchanged += n;
    }

    println!("{}", color::green(&format!(
        "Symlink install: {total_created} created, {total_updated} updated, {total_unchanged} unchanged"
    )));
    Ok(())
}

/// Expand ~ to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return format!("{home}/{rest}");
    }
    path.to_string()
}

/// Recursively symlink all files from source_dir to target_root,
/// preserving directory structure relative to source_root.
fn install_dir(source_dir: &Path, target_root: &Path, source_root: &Path) -> Result<(usize, usize, usize)> {
    if !source_dir.is_dir() {
        bail!("Source folder does not exist: {}", source_dir.display());
    }

    // Create target directory if needed
    if !target_root.exists() {
        fs::create_dir_all(target_root)
            .with_context(|| format!("Failed to create target folder: {}", target_root.display()))?;
    }

    let mut created = 0;
    let mut updated = 0;
    let mut unchanged = 0;

    let entries = fs::read_dir(source_dir)
        .with_context(|| format!("Failed to read directory: {}", source_dir.display()))?;

    for entry in entries {
        let entry = entry.with_context(|| format!("Failed to read entry in directory: {}", source_dir.display()))?;
        let source_path = entry.path();
        let relative = source_path.strip_prefix(source_root)
            .context("Failed to compute relative path")?;
        let target_path = target_root.join(relative);

        if source_path.is_dir() {
            if !target_path.exists() {
                fs::create_dir_all(&target_path)
                    .with_context(|| format!("Failed to create directory: {}", target_path.display()))?;
            }
            let (c, u, n) = install_dir(&source_path, target_root, source_root)?;
            created += c;
            updated += u;
            unchanged += n;
        } else {
            let abs_source = fs::canonicalize(&source_path)
                .with_context(|| format!("Failed to resolve absolute path: {}", source_path.display()))?;

            match install_symlink(&abs_source, &target_path)? {
                LinkResult::Created => {
                    println!("  {} {}", color::green("created"), relative.display());
                    created += 1;
                }
                LinkResult::Updated => {
                    println!("  {} {}", color::cyan("updated"), relative.display());
                    updated += 1;
                }
                LinkResult::Unchanged => {
                    unchanged += 1;
                }
            }
        }
    }

    Ok((created, updated, unchanged))
}

enum LinkResult {
    Created,
    Updated,
    Unchanged,
}

/// Create or update a single symlink.
fn install_symlink(source: &Path, target: &Path) -> Result<LinkResult> {
    if target.is_symlink() {
        if let Ok(existing) = fs::read_link(target)
            && existing == source
        {
            return Ok(LinkResult::Unchanged);
        }
        fs::remove_file(target)
            .with_context(|| format!("Failed to remove existing symlink: {}", target.display()))?;
        symlink(source, target)
            .with_context(|| format!("Failed to create symlink: {} -> {}", target.display(), source.display()))?;
        return Ok(LinkResult::Updated);
    }

    if target.exists() {
        fs::remove_file(target)
            .with_context(|| format!("Failed to remove existing file: {}", target.display()))?;
    }

    symlink(source, target)
        .with_context(|| format!("Failed to create symlink: {} -> {}", target.display(), source.display()))?;
    Ok(LinkResult::Created)
}
