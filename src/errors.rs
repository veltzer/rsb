/// Add context to a Result with automatic <file:line> location.
///
/// Usage:
///   `ctx(fs::read(&path)`, "Failed to read config")?;
///   let data = `ctx(fs::read(&path)`, &format!("Failed to read {}", `path.display()`))?;
///
/// Produces error messages like:
///   "Failed to read config at src/config/mod.rs:42: No such file or directory"
///
/// The source error is kept in the anyhow chain (context, not a formatted
/// string): `classify_error` walks the chain looking for `io::Error`, so a
/// version of this that stringified the source made exit code 5 (`IoError`)
/// unreachable for everything routed through here.
#[track_caller]
pub fn ctx<T, E>(r: Result<T, E>, msg: &str) -> anyhow::Result<T>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let loc = std::panic::Location::caller();
    r.map_err(|e| anyhow::Error::new(e)
        .context(format!("{} at {}:{}", msg, loc.file(), loc.line())))
}

/// Like `ctx()` but for Option — converts None to an error with <file:line> location.
#[track_caller]
pub fn ctx_opt<T>(r: Option<T>, msg: &str) -> anyhow::Result<T> {
    let loc = std::panic::Location::caller();
    r.ok_or_else(|| anyhow::anyhow!("{} at {}:{}", msg, loc.file(), loc.line()))
}

/// Centralized catalog of `.expect()` messages for internal errors.
///
/// Every `expect()` in production code should reference a constant from this
/// module so that messages stay consistent and are easy to grep for.
// -- Product / graph lookups --
pub const INVALID_PRODUCT_ID: &str = "internal error: invalid product id";
pub const EMPTY_PRODUCT_INPUTS: &str = "internal error: product has no inputs";
pub const EMPTY_PRODUCT_OUTPUTS: &str = "internal error: product has no outputs";
pub const PROCESSOR_NOT_IN_MAP: &str = "internal error: processor not in map";
pub const PROCESSOR_NOT_IN_TOTALS: &str = "internal error: processor not in total_per_processor map";

// -- Progress bar --
pub const INVALID_PROGRESS_TEMPLATE: &str = "internal error: invalid progress bar template";

// -- Regex compilation (compile-time constant patterns) --
pub const INVALID_REGEX: &str = "internal error: invalid regex";

// -- Serialization --
pub const JSON_SERIALIZE: &str = "internal error: failed to serialize JSON";
pub const CONFIG_SERIALIZE: &str = "internal error: failed to serialize config";

// -- Config resolution --
pub const SCAN_CONFIG_NOT_RESOLVED: &str = "internal error: scan fields not resolved";
pub const CAPTURE_GROUP_MISSING: &str = "internal error: capture group missing";

// -- Runtime / system --
pub const TOKIO_RUNTIME: &str = "internal error: failed to create tokio runtime";
pub const SIGNAL_HANDLER_RUNTIME: &str = "internal error: failed to create signal handler runtime";
pub const SIGNAL_LISTEN: &str = "internal error: failed to listen for Ctrl+C";
pub const SYSTEM_CLOCK: &str = "internal error: system clock before UNIX epoch";
