use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::build_context::BuildContext;
use crate::processors::ProcessorMap;

const LOCK_FILE: &str = ".tools.versions";
const LOCK_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolLockFile {
    pub version: u32,
    pub locked_at: String,
    pub tools: BTreeMap<String, LockedTool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LockedTool {
    pub path: String,
    pub version: Option<String>,
    pub version_output: String,
    pub version_args: Vec<String>,
}

/// Extract the first version string from version output.
/// Matches `X.Y.Z`, `X.Y`, or bare `X` (in that priority order).
pub fn extract_semver(version_output: &str) -> Option<&str> {
    let re = regex::Regex::new(r"\d+\.\d+\.\d+|\d+\.\d+|\d+").unwrap();
    re.find(version_output).map(|m| m.as_str())
}

/// Query a single tool for its version information.
pub fn query_tool_version(tool_name: &str, version_args: &[String]) -> Result<LockedTool> {
    let path = which::which(tool_name)
        .with_context(|| format!("Tool not found on PATH: {tool_name}"))?;

    let mut cmd = Command::new(&path);
    for arg in version_args {
        cmd.arg(arg);
    }

    let output = cmd.output()
        .with_context(|| format!("Failed to run: {} {}", path.display(), version_args.join(" ")))?;

    // Some tools write version to stdout, others to stderr; capture both
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let version_output = if stdout.is_empty() {
        stderr
    } else if stderr.is_empty() {
        stdout
    } else {
        format!("{stdout}\n{stderr}")
    };

    if version_output.is_empty() {
        anyhow::bail!(
            "Tool '{}' produced no version output with args: {}",
            tool_name,
            version_args.join(" ")
        );
    }

    let version = extract_semver(&version_output).map(String::from);

    Ok(LockedTool {
        path: path.display().to_string(),
        version,
        version_output,
        version_args: version_args.to_vec(),
    })
}

/// Collect tool version commands from all enabled processors, deduplicated.
pub fn collect_tool_commands(
    processors: &ProcessorMap,
    enabled: &dyn Fn(&str) -> bool,
) -> Vec<(String, Vec<String>)> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    let mut names: Vec<&String> = processors.keys().collect();
    names.sort();

    for name in names {
        if !enabled(name) {
            continue;
        }
        for (tool, args) in processors[name].tool_version_commands() {
            if seen.insert(tool.clone()) {
                result.push((tool, args));
            }
        }
    }

    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

/// Query all tools and build a lock file structure.
pub fn create_lock(
    tool_commands: &[(String, Vec<String>)],
) -> Result<ToolLockFile> {
    let mut tools = BTreeMap::new();

    for (tool_name, version_args) in tool_commands {
        let locked = query_tool_version(tool_name, version_args)?;
        tools.insert(tool_name.clone(), locked);
    }

    let locked_at = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    Ok(ToolLockFile {
        version: LOCK_VERSION,
        locked_at,
        tools,
    })
}

/// Write the lock file to disk atomically (temp file + rename), so a crash or
/// concurrent reader never observes a truncated lock file.
pub fn write_lock_file(lock: &ToolLockFile) -> Result<()> {
    let path = Path::new(LOCK_FILE);
    let json = serde_json::to_string_pretty(lock)
        .context("Failed to serialize lock file")?;
    let tmp = format!("{LOCK_FILE}.tmp-{}", std::process::id());
    fs::write(&tmp, format!("{json}\n"))
        .with_context(|| format!("Failed to write {tmp}"))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("Failed to move {tmp} into place as {}", path.display()))?;
    Ok(())
}

/// Read the lock file from disk. Returns None if it doesn't exist.
pub fn read_lock_file() -> Result<Option<ToolLockFile>> {
    let path = Path::new(LOCK_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let lock: ToolLockFile = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(Some(lock))
}

/// Identify a single tool for cache-keying purposes.
///
/// Prefers the locked `version_output` when a lock file pins the tool: that
/// is the user's declared identity for it, and it stays stable across
/// reinstalls of the same version. Otherwise falls back to a content hash of
/// the resolved binary, which needs no `--version` subprocess (the mtime
/// cache makes repeat builds a stat) and is strictly stronger — it also
/// catches a rebuilt binary that reports an unchanged version.
///
/// Returns None for a tool that is not on PATH: a missing tool is the tool
/// preflight's problem, not the cache key's, and failing here would break
/// builds whose products don't need that processor at all.
fn tool_identity(ctx: &BuildContext, tool: &str, lock: Option<&ToolLockFile>) -> Option<String> {
    if let Some(locked) = lock.and_then(|l| l.tools.get(tool)) {
        return Some(format!("{}={}", tool, locked.version_output));
    }
    let path = which::which(tool).ok()?;
    let (checksum, _) = crate::checksum::checksum_fast(ctx, &path).ok()?;
    Some(format!("{tool}@{checksum}"))
}

/// Compute a tool version hash per processor.
///
/// Returns a map from processor name to a hash of its tools' identities, for
/// mixing into every product that processor builds — so upgrading a tool
/// invalidates what that tool produced.
///
/// This is unconditional as of the finding-3 fix. It was previously gated on
/// a `.tools.versions` lock file existing, which meant the overwhelmingly
/// common case (no lock file) mixed in nothing at all and a tool upgrade
/// silently left every cached PASS valid. The lock file now only *refines*
/// the identity; its absence no longer disables the mechanism. Set
/// `[build] hash_tool_versions = false` to opt out.
pub fn processor_tool_hashes(
    ctx: &BuildContext,
    processors: &ProcessorMap,
    enabled: &dyn Fn(&str) -> bool,
) -> Result<std::collections::HashMap<String, String>> {
    use sha2::{Digest, Sha256};

    let lock = read_lock_file()?;

    let mut result = std::collections::HashMap::new();
    // One tool is typically required by several processors; resolving and
    // hashing it once per processor would multiply the work for nothing.
    let mut identity_cache: BTreeMap<String, Option<String>> = BTreeMap::new();

    let mut names: Vec<&String> = processors.keys().collect();
    names.sort();

    for name in names {
        if !enabled(name) {
            continue;
        }
        let tools = processors[name].required_tools();
        if tools.is_empty() {
            continue;
        }

        let mut version_parts: Vec<String> = Vec::new();
        for tool in &tools {
            let identity = identity_cache
                .entry(tool.clone())
                .or_insert_with(|| tool_identity(ctx, tool, lock.as_ref()));
            if let Some(identity) = identity {
                version_parts.push(identity.clone());
            }
        }

        if !version_parts.is_empty() {
            version_parts.sort();
            let combined = version_parts.join("\n");
            let hash = Sha256::digest(combined.as_bytes());
            result.insert(name.clone(), hex::encode(hash));
        }
    }

    Ok(result)
}

/// Verify that the current tool versions match the lock file.
/// Returns Ok(()) if everything matches, or an error describing mismatches.
/// Errors if the lock file does not exist — callers must run `rsconstruct
/// tools lock` to create one.
pub fn verify_lock_file(
    tool_commands: &[(String, Vec<String>)],
) -> Result<()> {
    let Some(lock) = read_lock_file()? else {
        anyhow::bail!("No {LOCK_FILE} found. Run `rsconstruct tools lock` to create one.");
    };

    let mut mismatches = Vec::new();

    for (tool_name, version_args) in tool_commands {
        let Some(locked) = lock.tools.get(tool_name) else {
            mismatches.push(format!("{tool_name} — not in lock file (new tool?)"));
            continue;
        };

        // Query current version
        let current = match query_tool_version(tool_name, version_args) {
            Ok(c) => c,
            Err(e) => {
                mismatches.push(format!("{tool_name} — {e}"));
                continue;
            }
        };

        if current.version_output != locked.version_output {
            mismatches.push(format!(
                "{} — version changed (locked: {}, current: {})",
                tool_name,
                extract_semver(&locked.version_output).unwrap_or("?"),
                extract_semver(&current.version_output).unwrap_or("?"),
            ));
        } else if current.path != locked.path {
            mismatches.push(format!(
                "{} — path changed (locked: {}, current: {})",
                tool_name, locked.path, current.path,
            ));
        }
    }

    // Check for tools in lock file that are no longer required
    for tool_name in lock.tools.keys() {
        if !tool_commands.iter().any(|(name, _)| name == tool_name) {
            mismatches.push(format!("{tool_name} — in lock file but no longer required"));
        }
    }

    if !mismatches.is_empty() {
        return Err(crate::exit_code::RsconstructError::new(
            crate::exit_code::RsconstructExitCode::ToolError,
            format!("Tool version mismatch (run 'rsconstruct tools lock' to update):\n{}", mismatches.join("\n")),
        ).into());
    }

    Ok(())
}
