//! Remote cache support for sharing build artifacts across machines.
//!
//! Supported backends:
//! - `s3://bucket/prefix` - Amazon S3 (requires AWS credentials)
//! - `http://host:port/path` or `https://...` - HTTP server with GET/PUT support
//! - `file:///absolute/path` - Local filesystem (for testing or network mounts)

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::errors;
use crate::processors::{check_command_output, run_command_capture};

/// Remote cache backend trait
pub trait RemoteCache: Send + Sync {
    /// Check if an object exists in the remote cache
    fn exists(&self, ctx: &crate::build_context::BuildContext, key: &str) -> Result<bool>;

    /// Upload a local file to remote cache
    fn upload(&self, ctx: &crate::build_context::BuildContext, key: &str, src: &Path) -> Result<()>;

    /// Download raw bytes (for index entries)
    fn download_bytes(&self, ctx: &crate::build_context::BuildContext, key: &str) -> Result<Option<Vec<u8>>>;

    /// Upload raw bytes (for index entries).
    /// Default implementation writes to a temp file and delegates to `upload()`.
    fn upload_bytes(&self, ctx: &crate::build_context::BuildContext, key: &str, data: &[u8]) -> Result<()> {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("rsconstruct-upload-{}", uuid_simple()));
        // create_new refuses to follow a pre-planted symlink or reuse an
        // existing file in the shared temp directory.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_file)
            .with_context(|| format!("Failed to create temp upload file: {}", temp_file.display()))?;
        file.write_all(data)
            .with_context(|| format!("Failed to write temp upload file: {}", temp_file.display()))?;
        drop(file);
        let result = self.upload(ctx, key, &temp_file);
        let _ = fs::remove_file(&temp_file);
        result
    }
}

/// Parse a remote URL and create the appropriate backend
pub fn create_backend(url: &str) -> Result<Box<dyn RemoteCache>> {
    if url.starts_with("s3://") {
        Ok(Box::new(S3Backend::new(url)?))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Ok(Box::new(HttpBackend::new(url)))
    } else if url.starts_with("file://") {
        Ok(Box::new(FileBackend::new(url)?))
    } else {
        anyhow::bail!(
            "Unsupported remote cache URL: {url}. Supported schemes: s3://, http://, https://, file://"
        )
    }
}

/// S3 backend using AWS CLI
pub struct S3Backend {
    bucket: String,
    prefix: String,
}

impl S3Backend {
    pub fn new(url: &str) -> Result<Self> {
        // Parse s3://bucket/prefix
        let without_scheme = url
            .strip_prefix("s3://")
            .context("Invalid S3 URL")?;

        let (bucket, prefix) = match without_scheme.find('/') {
            Some(idx) => {
                let (b, p) = without_scheme.split_at(idx);
                (b.to_string(), p[1..].to_string()) // Skip the leading '/'
            }
            None => (without_scheme.to_string(), String::new()),
        };

        anyhow::ensure!(!bucket.is_empty(), "Invalid S3 URL: missing bucket name in {url}");

        Ok(Self { bucket, prefix })
    }

    fn s3_key(&self, key: &str) -> String {
        if self.prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}/{}", self.prefix.trim_end_matches('/'), key)
        }
    }

    fn s3_uri(&self, key: &str) -> String {
        format!("s3://{}/{}", self.bucket, self.s3_key(key))
    }
}

impl RemoteCache for S3Backend {
    fn exists(&self, ctx: &crate::build_context::BuildContext, key: &str) -> Result<bool> {
        let mut cmd = Command::new("aws");
        cmd.args(["s3", "ls", &self.s3_uri(key)]);
        let output = run_command_capture(ctx, &cmd)?;
        Ok(output.status.success())
    }

    fn upload(&self, ctx: &crate::build_context::BuildContext, key: &str, src: &Path) -> Result<()> {
        let mut cmd = Command::new("aws");
        cmd.args([
            "s3", "cp",
            &src.display().to_string(),
            &self.s3_uri(key),
            "--only-show-errors",
        ]);
        let output = run_command_capture(ctx, &cmd)?;
        check_command_output(&output, "S3 upload")
    }

    fn download_bytes(&self, ctx: &crate::build_context::BuildContext, key: &str) -> Result<Option<Vec<u8>>> {
        let mut cmd = Command::new("aws");
        cmd.args(["s3", "cp", &self.s3_uri(key), "-"]);
        let output = run_command_capture(ctx, &cmd)?;

        if output.status.success() {
            Ok(Some(output.stdout))
        } else {
            Ok(None)
        }
    }
}

/// HTTP backend using curl
pub struct HttpBackend {
    base_url: String,
}

impl HttpBackend {
    pub fn new(url: &str) -> Self {
        Self {
            base_url: url.trim_end_matches('/').to_string(),
        }
    }

    fn full_url(&self, key: &str) -> String {
        format!("{}/{}", self.base_url, key.trim_start_matches('/'))
    }
}

impl RemoteCache for HttpBackend {
    fn exists(&self, ctx: &crate::build_context::BuildContext, key: &str) -> Result<bool> {
        let mut cmd = Command::new("curl");
        cmd.args([
            "-s", "-o", "/dev/null", "-w", "%{http_code}", "--head",
            &self.full_url(key),
        ]);
        let output = run_command_capture(ctx, &cmd)?;
        let status_code = String::from_utf8_lossy(&output.stdout);
        Ok(status_code.trim() == "200")
    }

    fn upload(&self, ctx: &crate::build_context::BuildContext, key: &str, src: &Path) -> Result<()> {
        let mut cmd = Command::new("curl");
        cmd.args([
            "-s", "-f", "-X", "PUT",
            "--data-binary", &format!("@{}", src.display()),
            &self.full_url(key),
        ]);
        let output = run_command_capture(ctx, &cmd)?;
        check_command_output(&output, "HTTP upload")
    }

    fn download_bytes(&self, ctx: &crate::build_context::BuildContext, key: &str) -> Result<Option<Vec<u8>>> {
        let mut cmd = Command::new("curl");
        cmd.args(["-s", "-f", &self.full_url(key)]);
        let output = run_command_capture(ctx, &cmd)?;

        if output.status.success() {
            Ok(Some(output.stdout))
        } else {
            Ok(None)
        }
    }
}

/// File backend for local/network filesystem
pub struct FileBackend {
    base_path: PathBuf,
}

impl FileBackend {
    pub fn new(url: &str) -> Result<Self> {
        // Parse file:///path
        let path = url
            .strip_prefix("file://")
            .context("Invalid file:// URL")?;

        let base_path = PathBuf::from(path);

        // Create base directory if it doesn't exist
        fs::create_dir_all(&base_path)
            .with_context(|| format!("Failed to create remote cache directory: {}", base_path.display()))?;

        Ok(Self { base_path })
    }

    fn full_path(&self, key: &str) -> PathBuf {
        self.base_path.join(key)
    }
}

impl RemoteCache for FileBackend {
    fn exists(&self, _ctx: &crate::build_context::BuildContext, key: &str) -> Result<bool> {
        Ok(self.full_path(key).exists())
    }

    fn upload(&self, _ctx: &crate::build_context::BuildContext, key: &str, src: &Path) -> Result<()> {
        let dest = self.full_path(key);

        let parent = dest.parent()
            .with_context(|| format!("Remote cache key has no parent directory: {}", dest.display()))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory for local cache upload: {}", parent.display()))?;

        // Copy to a unique temp name and rename into place: the shared mount
        // is read by other machines, which must never observe a half-written
        // object; racing uploaders of the same key harmlessly overwrite.
        let tmp = parent.join(format!(".tmp-upload-{}", uuid_simple()));
        fs::copy(src, &tmp)
            .with_context(|| format!("Failed to copy to remote cache: {}", tmp.display()))?;

        // Make read-only to prevent corruption, consistent with local cache objects
        crate::platform::set_permissions_mode(&tmp, 0o444)
            .with_context(|| format!("Failed to set remote cache object read-only: {}", tmp.display()))?;

        if let Err(e) = fs::rename(&tmp, &dest) {
            let _ = fs::remove_file(&tmp);
            if !dest.exists() {
                return Err(e).with_context(|| format!("Failed to move remote cache object into place: {}", dest.display()));
            }
        }

        Ok(())
    }

    fn download_bytes(&self, _ctx: &crate::build_context::BuildContext, key: &str) -> Result<Option<Vec<u8>>> {
        let path = self.full_path(key);
        if !path.exists() {
            return Ok(None);
        }

        let data = fs::read(&path)
            .with_context(|| format!("Failed to read from remote cache: {}", path.display()))?;

        Ok(Some(data))
    }

    fn upload_bytes(&self, _ctx: &crate::build_context::BuildContext, key: &str, data: &[u8]) -> Result<()> {
        let path = self.full_path(key);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory for cache upload: {}", parent.display()))?;
        }

        fs::write(&path, data)
            .with_context(|| format!("Failed to write to remote cache: {}", path.display()))?;

        // Make read-only to prevent corruption, consistent with upload()
        crate::platform::set_permissions_mode(&path, 0o444)
            .with_context(|| format!("Failed to set remote cache entry read-only: {}", path.display()))?;

        Ok(())
    }
}

/// Generate a simple unique identifier (timestamp + pid + counter)
fn uuid_simple() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect(errors::SYSTEM_CLOCK)
        .as_nanos();
    let pid = std::process::id();
    // Relaxed is fine: this counter only needs to be unique within a process,
    // not synchronized with other memory operations.
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("{timestamp:x}-{pid:x}-{seq:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_file_backend() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let url = format!("file://{}", temp_dir.path().display());
        let backend = FileBackend::new(&url).expect("failed to create file backend");
        let ctx = crate::build_context::BuildContext::new();

        // Test upload and download bytes
        let key = "test/data.txt";
        let data = b"hello world";

        backend.upload_bytes(&ctx, key, data).expect("upload failed");
        assert!(backend.exists(&ctx, key).expect("exists check failed"));

        let downloaded = backend.download_bytes(&ctx, key).expect("download failed");
        assert_eq!(downloaded, Some(data.to_vec()));
    }

    #[test]
    fn test_s3_url_parsing() {
        let backend = S3Backend::new("s3://my-bucket/cache/prefix").expect("failed to parse S3 URL");
        assert_eq!(backend.bucket, "my-bucket");
        assert_eq!(backend.prefix, "cache/prefix");
        assert_eq!(backend.s3_key("objects/ab/cdef"), "cache/prefix/objects/ab/cdef");

        let backend2 = S3Backend::new("s3://bucket").expect("failed to parse S3 URL");
        assert_eq!(backend2.bucket, "bucket");
        assert_eq!(backend2.prefix, "");
        assert_eq!(backend2.s3_key("objects/ab/cdef"), "objects/ab/cdef");
    }
}
