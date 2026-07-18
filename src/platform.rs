/// Cross-platform wrappers for OS-specific operations.
///
/// On Unix, file permissions use mode bits (e.g. 0o644).
/// On Windows, only read-only vs read-write is supported.
///
/// Reset SIGPIPE to default behavior so piping to head/less doesn't cause errors.
/// No-op on Windows (SIGPIPE doesn't exist there).
pub fn reset_sigpipe() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Get the Unix permission mode bits for a file.
/// Returns `None` on non-Unix platforms.
#[allow(clippy::unnecessary_wraps)] // Non-unix branch returns None; clippy only sees the unix path.
pub fn get_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// Whether package-manager invocations should be prefixed with `sudo`.
///
/// Returns false when:
/// - Already running as root (uid 0). sudo is a no-op and may not exist
///   (e.g. inside a bare ubuntu container).
/// - sudo is not on PATH. We can't use it; let the package manager fail
///   on its own with its native "are you root?" message.
///
/// Returns true otherwise (the normal local-dev case: non-root user with
/// passwordless or interactive sudo configured).
///
/// On non-Unix platforms (Windows), always returns false — there's no
/// sudo equivalent and the package managers we use there don't need it.
pub fn needs_sudo() -> bool {
    #[cfg(unix)]
    {
        let is_root = unsafe { libc::geteuid() } == 0;
        !is_root && which::which("sudo").is_ok()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Create a symbolic link to a file. On Windows, file symlinks require
/// elevated privileges or developer mode; we surface the OS error as-is.
pub fn symlink_file(original: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(original, link)
    }
    #[cfg(not(unix))]
    {
        std::os::windows::fs::symlink_file(original, link)
    }
}

/// Set file permissions from a Unix mode. On Unix this sets the exact mode bits.
/// On Windows this approximates by setting read-only when the mode has no owner
/// write bit (i.e. `mode & 0o200 == 0`).
pub fn set_permissions_mode(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let readonly = mode & 0o200 == 0;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_readonly(readonly);
        std::fs::set_permissions(path, perms)
    }
}
