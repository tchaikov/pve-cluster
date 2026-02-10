//! File locking utilities
//!
//! This module provides file-based locking to ensure only one pmxcfs instance
//! runs at a time. It uses the flock(2) system call with exclusive locks.

use anyhow::{Context, Result};
use pmxcfs_api_types::PmxcfsError;
use std::fs::File;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use tracing::{info, warn};

/// RAII wrapper for a file lock
///
/// The lock is automatically released when the FileLock is dropped.
pub struct FileLock(File);

impl FileLock {
    const MAX_RETRIES: u32 = 10;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

    /// Acquire an exclusive file lock with retries (async)
    ///
    /// This function attempts to acquire an exclusive, non-blocking lock on the
    /// specified file. It will retry up to 10 times with 1-second delays between
    /// attempts, matching the C implementation's behavior.
    ///
    /// The blocking operations (file I/O and sleep) are executed on a blocking
    /// thread pool to avoid blocking the async runtime.
    ///
    /// # Arguments
    ///
    /// * `lockfile_path` - Path to the lock file
    ///
    /// # Returns
    ///
    /// Returns a `FileLock` which automatically releases the lock when dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The lock file cannot be created
    /// - The lock cannot be acquired after 10 retry attempts
    pub async fn acquire(lockfile_path: PathBuf) -> Result<Self> {
        // Open/create the lock file on blocking thread pool
        let file = tokio::task::spawn_blocking({
            let lockfile_path = lockfile_path.clone();
            move || {
                File::options()
                    .create(true)
                    .read(true)
                    .append(true)
                    .mode(0o600)
                    .open(&lockfile_path)
                    .with_context(|| {
                        format!("Unable to create lock file at {}", lockfile_path.display())
                    })
            }
        })
        .await
        .context("Failed to spawn blocking task for file creation")??;

        // Try to acquire lock with retries (matching C implementation)
        for attempt in 0..=Self::MAX_RETRIES {
            if Self::try_lock(&file).await? {
                info!(path = %lockfile_path.display(), "Acquired pmxcfs lock");
                return Ok(FileLock(file));
            }

            if attempt == Self::MAX_RETRIES {
                return Err(PmxcfsError::System("Unable to acquire pmxcfs lock".into()).into());
            }

            if attempt == 0 {
                warn!("Unable to acquire pmxcfs lock - retrying");
            }

            tokio::time::sleep(Self::RETRY_DELAY).await;
        }

        unreachable!("Loop should have returned or errored")
    }

    /// Attempt to acquire the lock (non-blocking)
    async fn try_lock(file: &File) -> Result<bool> {
        let result = tokio::task::spawn_blocking({
            let fd = file.as_raw_fd();
            move || unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) }
        })
        .await
        .context("Failed to spawn blocking task for flock")?;

        Ok(result == 0)
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Safety: We own the file descriptor
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
