//! Traits for MemDb operations
//!
//! This module provides the `MemDbOps` trait which abstracts MemDb operations
//! for dependency injection and testing. Similar to `StatusOps` in pmxcfs-status.

use crate::types::TreeEntry;
use anyhow::Result;

/// Trait abstracting MemDb operations for dependency injection and mocking
///
/// This trait enables:
/// - Dependency injection of MemDb into components
/// - Testing with MockMemDb instead of real database
/// - Trait objects for runtime polymorphism
///
/// # Example
/// ```no_run
/// use pmxcfs_memdb::{MemDb, MemDbOps};
/// use std::sync::Arc;
///
/// fn use_database(db: Arc<dyn MemDbOps>) {
///     // Can work with real MemDb or MockMemDb
///     let exists = db.exists("/test").unwrap();
/// }
/// ```
pub trait MemDbOps: Send + Sync {
    // ===== Basic File Operations =====

    /// Create a new file or directory
    fn create(&self, path: &str, mode: u32, writer: u32, mtime: u32) -> Result<()>;

    /// Read data from a file
    fn read(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>>;

    /// Write data to a file
    fn write(
        &self,
        path: &str,
        offset: u64,
        writer: u32,
        mtime: u32,
        data: &[u8],
        truncate: bool,
    ) -> Result<usize>;

    /// Delete a file or directory
    fn delete(&self, path: &str, writer: u32, mtime: u32) -> Result<()>;

    /// Rename a file or directory
    fn rename(&self, old_path: &str, new_path: &str, writer: u32, mtime: u32) -> Result<()>;

    /// Check if a path exists
    fn exists(&self, path: &str) -> Result<bool>;

    /// List directory contents
    fn readdir(&self, path: &str) -> Result<Vec<TreeEntry>>;

    /// Set modification time
    fn set_mtime(&self, path: &str, writer: u32, mtime: u32) -> Result<()>;

    // ===== Path Lookup =====

    /// Look up a path and return its entry
    fn lookup_path(&self, path: &str) -> Option<TreeEntry>;

    /// Get entry by inode number
    fn get_entry_by_inode(&self, inode: u64) -> Option<TreeEntry>;

    // ===== Lock Operations =====

    /// Acquire a lock on a path
    fn acquire_lock(&self, path: &str, csum: &[u8; 32]) -> Result<()>;

    /// Release a lock on a path
    fn release_lock(&self, path: &str, csum: &[u8; 32]) -> Result<()>;

    /// Check if a path is locked
    fn is_locked(&self, path: &str) -> bool;

    /// Check if a lock has expired
    fn lock_expired(&self, path: &str, csum: &[u8; 32]) -> bool;

    // ===== Database Operations =====

    /// Get the current database version
    fn get_version(&self) -> u64;

    /// Get all entries in the database
    fn get_all_entries(&self) -> Result<Vec<TreeEntry>>;

    /// Replace all entries (for synchronization)
    fn replace_all_entries(&self, entries: Vec<TreeEntry>) -> Result<()>;

    /// Apply a single tree entry update
    fn apply_tree_entry(&self, entry: TreeEntry) -> Result<()>;

    /// Encode the entire database for network transmission
    fn encode_database(&self) -> Result<Vec<u8>>;

    /// Compute database checksum
    fn compute_database_checksum(&self) -> Result<[u8; 32]>;
}
