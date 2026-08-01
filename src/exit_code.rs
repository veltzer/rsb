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

/// Shorthand for a typed ConfigError.
pub fn config_error(message: impl Into<String>) -> anyhow::Error {
    RsconstructError::new(RsconstructExitCode::ConfigError, message.into()).into()
}

/// Classify an anyhow error into an exit code.
///
/// Only typed information is consulted: a [`RsconstructError`] anywhere in
/// the chain wins, a raw `std::io::Error` in the chain means IoError, and
/// everything else is a BuildError. There is deliberately no message
/// pattern matching — error chains embed subprocess stderr, so substring
/// matching turns arbitrary tool output into wrong exit codes (a linter
/// printing "unknown field" is not a config error).
pub fn classify_error(err: &anyhow::Error) -> RsconstructExitCode {
    if let Some(rsconstruct_err) = err.downcast_ref::<RsconstructError>() {
        return rsconstruct_err.exit_code;
    }
    if err.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some()) {
        // A raw IO error anywhere in the chain: filesystem-level failure.
        return RsconstructExitCode::IoError;
    }
    RsconstructExitCode::BuildError
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

    /// The typed error must win even when wrapped in layers of context —
    /// downcast_ref on anyhow searches the whole chain.
    #[test]
    fn classify_typed_error_survives_context_wrapping() {
        let err: anyhow::Error = RsconstructError::new(RsconstructExitCode::ToolError, "tool gone").into();
        let wrapped = err.context("while preflighting").context("during build");
        assert_eq!(classify_error(&wrapped), RsconstructExitCode::ToolError);
    }

    /// A raw io::Error anywhere in the chain classifies as IoError — the
    /// only way exit code 5 is reachable.
    #[test]
    fn classify_io_error_in_chain() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = anyhow::Error::new(io).context("Failed to write output");
        assert_eq!(classify_error(&err), RsconstructExitCode::IoError);
    }

    /// Subprocess stderr embedded in an error message must never influence
    /// the exit code — a linter printing "unknown field" is not a config
    /// error, and a tool printing "interrupted" is not exit 130.
    #[test]
    fn classify_never_pattern_matches_messages() {
        for msg in [
            "tool output: unknown field `foo`",
            "tool output: operation interrupted",
            "tool output: cycle detected",
            "tool output: tool version mismatch",
            "something totally unexpected",
        ] {
            assert_eq!(classify_error(&anyhow::anyhow!("{msg}")), RsconstructExitCode::BuildError,
                "untyped message must default to BuildError: {msg}");
        }
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
