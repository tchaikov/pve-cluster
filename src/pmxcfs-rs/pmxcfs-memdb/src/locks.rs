/// Lock management for memdb
///
/// Locks in pmxcfs are implemented as directory entries stored in the database at
/// `priv/lock/<lockname>`. This ensures locks are:
/// 1. Persistent across restarts
/// 2. Synchronized across the cluster via DFSM
/// 3. Visible to both C and Rust nodes
///
/// The in-memory lock table is a cache rebuilt from the database on startup
/// and updated dynamically during runtime.
use anyhow::Result;
use std::time::{SystemTime, UNIX_EPOCH};

use super::database::MemDb;
use super::types::{LOCK_DIR_PATH, LOCK_TIMEOUT, LockInfo};

/// Check if a path is in the lock directory
///
/// Matches C's path_is_lockdir() function (cfs-utils.c:306)
/// Returns true if path is "{LOCK_DIR_PATH}/<something>" (with or without leading /)
pub fn is_lock_path(path: &str) -> bool {
    let path = path.trim_start_matches('/');
    let lock_prefix = format!("{LOCK_DIR_PATH}/");
    path.starts_with(&lock_prefix) && path.len() > lock_prefix.len()
}

fn lock_cache_key(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.starts_with(LOCK_DIR_PATH) {
        trimmed.to_string()
    } else {
        format!("{}/{}", LOCK_DIR_PATH, trimmed)
    }
}

fn lock_paths(path: &str) -> (String, String) {
    let lock_key = lock_cache_key(path);
    let lock_path = format!("/{}", lock_key);
    (lock_key, lock_path)
}

impl MemDb {
    /// Check if a lock has expired (with side effects matching C semantics)
    ///
    /// This function implements the same behavior as the C version (memdb.c:330-358):
    /// - If no lock exists in cache: Reads from database, creates cache entry, returns `false`
    /// - If lock exists but csum mismatches: Updates csum, resets timeout, logs critical error, returns `false`
    /// - If lock exists, csum matches, and time > LOCK_TIMEOUT: Returns `true` (expired)
    /// - Otherwise: Returns `false` (not expired)
    ///
    /// This function is used for both checking AND managing locks, matching C semantics.
    ///
    /// # Current Usage
    /// - Called from `database::create()` when creating lock directories (matching C memdb.c:928)
    /// - Called from FUSE utimens operation (pmxcfs/src/fuse/filesystem.rs:717) for mtime=0 unlock requests
    /// - Called from DFSM unlock message handlers (pmxcfs/src/memdb_callbacks.rs:142,161)
    ///
    /// Note: DFSM broadcasting of unlock messages to cluster nodes is not yet fully implemented.
    /// See TODOs in filesystem.rs:723 and memdb_callbacks.rs:154 for remaining work.
    pub fn lock_expired(&self, path: &str, csum: &[u8; 32]) -> bool {
        let (lock_key, _lock_path) = lock_paths(path);

        let mut locks = self.inner.locks.lock();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        match locks.get_mut(&lock_key) {
            Some(lock_info) => {
                // Lock exists in cache - check csum
                if lock_info.csum != *csum {
                    // Wrong csum - update and reset timeout
                    lock_info.ltime = now;
                    lock_info.csum = *csum;
                    tracing::error!("Lock checksum mismatch for '{}' - resetting timeout", lock_key);
                    return false;
                }

                // Csum matches - check if expired
                // Use saturating_sub to handle backward clock jumps
                let elapsed = now.saturating_sub(lock_info.ltime);
                if elapsed > LOCK_TIMEOUT {
                    tracing::debug!(path = lock_key, elapsed, "Lock expired");
                    return true; // Expired
                }

                false // Not expired
            }
            None => {
                // No lock in cache - create new cache entry
                locks.insert(lock_key.clone(), LockInfo { ltime: now, csum: *csum });
                tracing::debug!(path = lock_key, "Created new lock cache entry");
                false // Not expired (just created)
            }
        }
    }

    /// Acquire a lock on a path
    ///
    /// This creates a directory entry in the database at `priv/lock/<lockname>`
    /// and broadcasts the operation to the cluster via DFSM.
    pub fn acquire_lock(&self, path: &str, csum: &[u8; 32]) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let (lock_key, lock_path) = lock_paths(path);

        let locks = self.inner.locks.lock();

        // Check if there's an existing valid lock in cache
        if let Some(existing_lock) = locks.get(&lock_key) {
            // Use saturating_sub to handle backward clock jumps
            let lock_age = now.saturating_sub(existing_lock.ltime);
            if lock_age <= LOCK_TIMEOUT && existing_lock.csum != *csum {
                return Err(anyhow::anyhow!("Lock already held by another process"));
            }
        }

        // Extract lock name from path like "priv/lock/foo.lock" or "priv/lock/qemu-server/103.conf"
        let lock_prefix = format!("{LOCK_DIR_PATH}/");
        let lock_name = lock_key.strip_prefix(&lock_prefix).unwrap_or(&lock_key);

        if lock_key == LOCK_DIR_PATH || lock_name.is_empty() {
            return Err(anyhow::anyhow!("Invalid lock name (missing entry)"));
        }

        // Validate lock name to prevent path traversal
        if lock_name.contains("..") {
            return Err(anyhow::anyhow!("Invalid lock name (path traversal): {}", lock_name));
        }

        // Release locks mutex before database operations to avoid deadlock
        drop(locks);

        // Create or update lock directory in database
        // First check if it exists
        if self.exists(&lock_path)? {
            // Lock directory exists - update its mtime to refresh
            // In C this is implicit through the checksum, we'll update the entry
            tracing::debug!("Refreshing existing lock directory: {}", lock_path);
            // We don't need to do anything - the lock cache entry will be updated below
        } else {
            // Create lock directory in database
            let mode = libc::S_IFDIR | 0o755;
            let mtime = now as u32;

            // Ensure lock directory exists
            let lock_dir_full = format!("/{LOCK_DIR_PATH}");
            if !self.exists(&lock_dir_full)? {
                self.create(&lock_dir_full, libc::S_IFDIR | 0o755, 0, mtime)?;
            }

            self.create(&lock_path, mode, 0, mtime)?;
            tracing::debug!("Created lock directory in database: {}", lock_path);
        }

        // Update in-memory cache (use normalized path without leading slash)
        let mut locks = self.inner.locks.lock();
        locks.insert(lock_key, LockInfo { ltime: now, csum: *csum });

        tracing::debug!("Lock acquired on path: {}", lock_path);
        Ok(())
    }

    /// Release a lock on a path
    ///
    /// This deletes the directory entry from the database and broadcasts
    /// the delete operation to the cluster via DFSM.
    pub fn release_lock(&self, path: &str, csum: &[u8; 32]) -> Result<()> {
        let (lock_key, lock_path) = lock_paths(path);

        let locks = self.inner.locks.lock();

        if let Some(lock_info) = locks.get(&lock_key) {
            // Only release if checksum matches
            if lock_info.csum != *csum {
                return Err(anyhow::anyhow!("Cannot release lock: checksum mismatch"));
            }
        } else {
            return Err(anyhow::anyhow!("No lock found on path: {}", normalized_path));
        }

        // Release locks mutex before database operations
        drop(locks);

        // Delete lock directory from database
        if self.exists(&lock_path)? {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_secs() as u32;
            self.delete(&lock_path, 0, now)?;
            tracing::debug!("Deleted lock directory from database: {}", lock_path);
        }

        // Remove from in-memory cache
        let mut locks = self.inner.locks.lock();
        locks.remove(&lock_key);

        tracing::debug!("Lock released on path: {}", lock_path);
        Ok(())
    }

    /// Update lock cache by scanning the priv/lock directory in database
    ///
    /// This implements the C version's behavior (memdb.c:360-89):
    /// - Scans the `priv/lock` directory in the database
    /// - Rebuilds the entire lock hash table from database state
    /// - Preserves `ltime` from old entries if csum matches
    /// - Is called on database open and after synchronization
    ///
    /// This ensures locks are visible across C/Rust nodes and survive restarts.
    pub(crate) fn update_locks(&self) {
        // Check if lock directory exists
        let _lock_dir = match self.lookup_path(LOCK_DIR_PATH) {
            Some(entry) if entry.is_dir() => entry,
            _ => {
                tracing::debug!(
                    "{} directory does not exist, initializing empty lock table",
                    LOCK_DIR_PATH
                );
                self.inner.locks.lock().clear();
                return;
            }
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Get old locks table for preserving ltimes
        let old_locks = {
            let locks = self.inner.locks.lock();
            locks.clone()
        };

        // Build new locks table from database
        let mut new_locks = std::collections::HashMap::new();

        // Read all lock directories
        match self.readdir(LOCK_DIR_PATH) {
            Ok(entries) => {
                for entry in entries {
                    // Only process directories (locks are stored as directories)
                    if !entry.is_dir() {
                        continue;
                    }

                    let lock_path = format!("{}/{}", LOCK_DIR_PATH, entry.name);
                    let csum = entry.compute_checksum();

                    // Check if we have an old entry with matching checksum
                    let ltime = if let Some(old_lock) = old_locks.get(&lock_path) {
                        if old_lock.csum == csum {
                            // Checksum matches - preserve old ltime
                            old_lock.ltime
                        } else {
                            // Checksum changed - reset ltime
                            now
                        }
                    } else {
                        // New lock - set ltime to now
                        now
                    };

                    new_locks.insert(lock_path.clone(), LockInfo { ltime, csum });
                    tracing::debug!("Loaded lock from database: {}", lock_path);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to read {} directory: {}", LOCK_DIR_PATH, e);
                return;
            }
        }

        // Replace lock table
        *self.inner.locks.lock() = new_locks;

        tracing::debug!(
            "Updated lock table from database: {} locks",
            self.inner.locks.lock().len()
        );
    }

    /// Check if a path is locked
    pub fn is_locked(&self, path: &str) -> bool {
        let lock_key = lock_cache_key(path);

        let locks = self.inner.locks.lock();
        if let Some(lock_info) = locks.get(&lock_key) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            // Check if lock is still valid (not expired)
            // Use saturating_sub to handle backward clock jumps
            now.saturating_sub(lock_info.ltime) <= LOCK_TIMEOUT
        } else {
            false
        }
    }
}
