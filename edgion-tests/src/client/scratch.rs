//! Repository-local scratch directory for test-owned temporary files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Directory for test-owned temporary files, created on first use.
///
/// An integration run keeps every artifact on the repository's own volume.
/// `std::env::temp_dir()` may resolve to a different and much smaller disk —
/// on macOS it is `$TMPDIR` on the system volume even when the checkout lives
/// on an external drive — which lets an unrelated volume's free space decide
/// whether the suite passes. Scenarios that need a scratch file must use this
/// directory instead. `EDGION_TEST_SCRATCH_DIR` overrides the default for a
/// harness that owns its own work directory.
pub async fn scratch_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("EDGION_TEST_SCRATCH_DIR") {
        Some(value) => PathBuf::from(value),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("tmp/integration_testing/scratch"),
    };
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create test scratch directory: {}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_scratch_dir_is_repository_local_and_exists() {
        if std::env::var_os("EDGION_TEST_SCRATCH_DIR").is_some() {
            return;
        }
        let dir = scratch_dir().await.expect("scratch directory must be creatable");
        assert!(dir.is_dir(), "scratch directory was not created: {}", dir.display());
        assert!(
            !dir.starts_with(std::env::temp_dir()),
            "scratch must not live on the system temp volume: {}",
            dir.display()
        );
        assert!(
            dir.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("repository root")),
            "scratch must live inside the repository: {}",
            dir.display()
        );
    }
}
