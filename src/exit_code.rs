use std::fmt;

/// Exit codes for rsconstruct, allowing CI scripts to distinguish error types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum RsconstructExitCode {
    Success = 0,
    BuildError = 1,
    ConfigError = 2,
    ToolError = 3,
    GraphError = 4,
    IoError = 5,
    Interrupted = 130,
}

impl RsconstructExitCode {
    pub const fn code(self) -> u8 {
        self as u8
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Success => "EXIT_SUCCESS",
            Self::BuildError => "EXIT_BUILD_ERROR",
            Self::ConfigError => "EXIT_CONFIG_ERROR",
            Self::ToolError => "EXIT_TOOL_ERROR",
            Self::GraphError => "EXIT_GRAPH_ERROR",
            Self::IoError => "EXIT_IO_ERROR",
            Self::Interrupted => "EXIT_INTERRUPTED",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Success => "Build completed successfully",
            Self::BuildError => "One or more processors failed",
            Self::ConfigError => "Invalid or missing configuration",
            Self::ToolError => "Required external tool missing or wrong version",
            Self::GraphError => "Dependency cycle or output conflict in build graph",
            Self::IoError => "File system or I/O operation failed",
            Self::Interrupted => "Build interrupted by signal (Ctrl+C)",
        }
    }
}

/// A typed error that carries an exit code for classification.
#[derive(Debug)]
pub struct RsconstructError {
    pub exit_code: RsconstructExitCode,
    pub message: String,
}

impl RsconstructError {
    pub fn new(exit_code: RsconstructExitCode, message: impl Into<String>) -> Self {
        Self {
            exit_code,
            message: message.into(),
        }
    }
}

impl fmt::Display for RsconstructError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RsconstructError {}

/// Classify an anyhow error into an exit code.
/// First tries downcasting to RsconstructError, then falls back to message pattern matching.
pub fn classify_error(err: &anyhow::Error) -> RsconstructExitCode {
    // Primary: downcast to our typed error
    if let Some(rsconstruct_err) = err.downcast_ref::<RsconstructError>() {
        return rsconstruct_err.exit_code;
    }

    // Fallback: message pattern matching
    let msg = format!("{err:#}");
    let lower = msg.to_lowercase();

    if lower.contains("interrupted") || lower.contains("ctrl+c") {
        RsconstructExitCode::Interrupted
    } else if lower.contains("no rsconstruct.toml found")
        || lower.contains("rsconstruct.toml already exists")
        || lower.contains("unknown processor")
        || lower.contains("unknown shell")
        || lower.contains("undefined variable")
        || lower.contains("failed to parse config")
        || lower.contains("failed to substitute variables")
        || lower.contains("deny_unknown_fields")
        || lower.contains("unknown field")
        || lower.contains("invalid config")
    {
        RsconstructExitCode::ConfigError
    } else if lower.contains("tool version mismatch")
        || lower.contains("tools are missing")
    {
        RsconstructExitCode::ToolError
    } else if lower.contains("cycle detected")
        || lower.contains("output conflict")
    {
        RsconstructExitCode::GraphError
    } else if lower.contains("build completed with") && lower.contains("error") {
        RsconstructExitCode::BuildError
    } else if err.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some()) {
        // A raw IO error anywhere in the chain: filesystem-level failure.
        // Makes the advertised exit code 5 actually reachable.
        RsconstructExitCode::IoError
    } else {
        // Default to BuildError for unclassified errors
        RsconstructExitCode::BuildError
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_rsconstruct_error_downcast() {
        let err: anyhow::Error = RsconstructError::new(RsconstructExitCode::ConfigError, "bad config").into();
        assert_eq!(classify_error(&err), RsconstructExitCode::ConfigError);

        let err: anyhow::Error = RsconstructError::new(RsconstructExitCode::GraphError, "cycle").into();
        assert_eq!(classify_error(&err), RsconstructExitCode::GraphError);
    }

    #[test]
    fn classify_interrupted() {
        let err = anyhow::anyhow!("Build was interrupted by Ctrl+C");
        assert_eq!(classify_error(&err), RsconstructExitCode::Interrupted);

        let err = anyhow::anyhow!("operation interrupted");
        assert_eq!(classify_error(&err), RsconstructExitCode::Interrupted);
    }

    #[test]
    fn classify_config_errors() {
        for msg in [
            "No rsconstruct.toml found in current directory",
            "rsconstruct.toml already exists",
            "unknown processor 'foo'",
            "unknown shell 'fish'",
            "undefined variable 'x'",
            "failed to parse config",
            "failed to substitute variables",
            "unknown field `blah`",
            "Invalid config:\n[processor.pandoc]: field 'src_dirs' must be an array",
        ] {
            assert_eq!(classify_error(&anyhow::anyhow!("{msg}")), RsconstructExitCode::ConfigError,
                "expected ConfigError for: {msg}");
        }
    }

    #[test]
    fn classify_tool_errors() {
        let err = anyhow::anyhow!("tool version mismatch: gcc 12 vs 13");
        assert_eq!(classify_error(&err), RsconstructExitCode::ToolError);

        let err = anyhow::anyhow!("Required tools are missing: ruff");
        assert_eq!(classify_error(&err), RsconstructExitCode::ToolError);
    }

    #[test]
    fn classify_graph_errors() {
        let err = anyhow::anyhow!("Cycle detected in dependency graph");
        assert_eq!(classify_error(&err), RsconstructExitCode::GraphError);

        let err = anyhow::anyhow!("Output conflict: foo.o produced by both [cc] and [cc2]");
        assert_eq!(classify_error(&err), RsconstructExitCode::GraphError);
    }

    #[test]
    fn classify_build_error() {
        let err = anyhow::anyhow!("Build completed with 3 error(s)");
        assert_eq!(classify_error(&err), RsconstructExitCode::BuildError);
    }

    #[test]
    fn classify_unknown_defaults_to_build_error() {
        let err = anyhow::anyhow!("something totally unexpected");
        assert_eq!(classify_error(&err), RsconstructExitCode::BuildError);
    }

    #[test]
    fn exit_codes_have_correct_values() {
        assert_eq!(RsconstructExitCode::Success.code(), 0);
        assert_eq!(RsconstructExitCode::BuildError.code(), 1);
        assert_eq!(RsconstructExitCode::ConfigError.code(), 2);
        assert_eq!(RsconstructExitCode::ToolError.code(), 3);
        assert_eq!(RsconstructExitCode::GraphError.code(), 4);
        assert_eq!(RsconstructExitCode::IoError.code(), 5);
        assert_eq!(RsconstructExitCode::Interrupted.code(), 130);
    }
}
