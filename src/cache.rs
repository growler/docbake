//! Artifact cache for rendered diagrams.
//!
//! Cached files are stored as `<blake3-hex>.<renderer>` where `<renderer>`
//! identifies the rendering backend and output format (e.g. `mermaid.svg`,
//! `plantuml.svg`, `drawio.pdf`).
//!
//! The cache directory is created lazily on the first write.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Application-specific subdirectory name inside the platform cache root.
const APP_CACHE_DIR: &str = "docbake";

/// Artifact cache backed by a filesystem directory.
///
/// When `dir` is `None` the cache is disabled — all lookups return `None`
/// and stores are no-ops.
#[derive(Debug, Clone, Default)]
pub struct Cache {
    dir: Option<PathBuf>,
}

impl Cache {
    /// Create a new cache instance.
    ///
    /// - If `disabled` is `true`, the cache is turned off entirely.
    /// - If `cli_override` is `Some`, that path is used as the cache
    ///   directory.
    /// - Otherwise the platform-standard user cache directory is used
    ///   (`~/.cache/docbake` on Linux, `~/Library/Caches/docbake`
    ///   on macOS, `%LocalAppData%\docbake` on Windows).
    pub fn new(cli_override: Option<PathBuf>, disabled: bool) -> Result<Self> {
        if disabled {
            return Ok(Self { dir: None });
        }
        let dir = match cli_override {
            Some(p) => p,
            None => dirs::cache_dir()
                .context("failed to determine platform cache directory")?
                .join(APP_CACHE_DIR),
        };
        Ok(Self { dir: Some(dir) })
    }

    /// Returns `true` when the cache is active (not disabled).
    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// Look up a cached artifact.
    ///
    /// `content` is the raw bytes that identify the source (e.g. diagram
    /// source text, or file bytes + page number for Draw.io).
    /// `renderer` is a dot-separated tag such as `"mermaid.svg"`.
    ///
    /// Returns `Some(data)` on a cache hit, `None` on miss or if the cache
    /// is disabled.
    pub fn lookup(&self, content: &[u8], renderer: &str) -> Option<Vec<u8>> {
        let dir = self.dir.as_ref()?;
        let path = cache_path(dir, content, renderer);
        fs::read(&path).ok()
    }

    /// Returns `true` when the cache holds an artifact for `content` and
    /// `renderer`, without reading it.
    pub fn contains(&self, content: &[u8], renderer: &str) -> bool {
        self.dir
            .as_ref()
            .is_some_and(|dir| cache_path(dir, content, renderer).is_file())
    }

    /// Store an artifact in the cache.
    ///
    /// Creates the cache directory on first call. Returns `Ok(())` silently
    /// when the cache is disabled.
    pub fn store(&self, content: &[u8], renderer: &str, data: &[u8]) -> Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        fs::create_dir_all(dir)
            .with_context(|| format!("failed to create cache directory {}", dir.display()))?;
        let path = cache_path(dir, content, renderer);
        fs::write(&path, data)
            .with_context(|| format!("failed to write cache entry {}", path.display()))?;
        Ok(())
    }
}

/// Compute the filesystem path for a cache entry.
fn cache_path(dir: &Path, content: &[u8], renderer: &str) -> PathBuf {
    let hash = blake3::hash(content);
    dir.join(format!("{}.{}", hash.to_hex(), renderer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_cache_returns_none() {
        let cache = Cache::new(None, true).unwrap();
        assert!(!cache.is_enabled());
        assert!(cache.lookup(b"hello", "mermaid.svg").is_none());
        // store should be a no-op, not an error
        cache.store(b"hello", "mermaid.svg", b"<svg/>").unwrap();
    }

    #[test]
    fn round_trip_with_override_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache = Cache::new(Some(tmp.path().to_path_buf()), false).unwrap();
        assert!(cache.is_enabled());

        let content = b"graph LR; A-->B";
        let renderer = "mermaid.svg";
        let data = b"<svg>...</svg>";

        assert!(cache.lookup(content, renderer).is_none());
        cache.store(content, renderer, data).unwrap();
        let cached = cache.lookup(content, renderer).unwrap();
        assert_eq!(cached, data);
    }

    #[test]
    fn different_renderer_tags_do_not_collide() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache = Cache::new(Some(tmp.path().to_path_buf()), false).unwrap();

        let content = b"same source";
        cache.store(content, "mermaid.svg", b"svg-data").unwrap();
        cache.store(content, "drawio.pdf", b"pdf-data").unwrap();

        assert_eq!(cache.lookup(content, "mermaid.svg").unwrap(), b"svg-data");
        assert_eq!(cache.lookup(content, "drawio.pdf").unwrap(), b"pdf-data");
    }
}
