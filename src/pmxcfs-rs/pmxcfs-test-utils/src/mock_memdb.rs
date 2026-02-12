//! Mock in-memory database implementation for testing
//!
//! This module provides `MockMemDb`, a lightweight in-memory implementation
//! of the `MemDbOps` trait for use in unit tests.

use anyhow::{Result, bail};
use parking_lot::RwLock;
use pmxcfs_memdb::{MemDbOps, LOCK_DIR_PATH, ROOT_INODE, TreeEntry};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// Directory and file type constants from dirent.h
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;

// Lock timeout in seconds (matches C implementation)
const LOCK_TIMEOUT_SECS: u64 = 120;

/// Normalize a lock identifier into the cache key used by the lock map.
///
/// This mirrors the behavior in the production MemDb by ensuring the key is
/// a relative path starting with the `priv/lock` prefix.
fn lock_cache_key(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.starts_with(LOCK_DIR_PATH) {
        trimmed.to_string()
    } else {
        format!("{}/{}", LOCK_DIR_PATH, trimmed)
    }
}

/// Mock in-memory database for testing
///
/// Unlike the real `MemDb` which uses SQLite persistence, `MockMemDb` stores
/// everything in memory using HashMap. This makes it:
/// - Faster for unit tests (no disk I/O)
/// - Easier to inject failures for error testing
/// - Completely isolated (no shared state between tests)
///
/// # Example
/// ```
/// use pmxcfs_test_utils::MockMemDb;
/// use pmxcfs_memdb::MemDbOps;
/// use std::sync::Arc;
///
/// let db: Arc<dyn MemDbOps> = Arc::new(MockMemDb::new());
/// db.create("/test.txt", 0, 0, 1234).unwrap();
/// assert!(db.exists("/test.txt").unwrap());
/// ```
pub struct MockMemDb {
    /// Files and directories stored as path -> data
    files: RwLock<HashMap<String, Vec<u8>>>,
    /// Directory entries stored as path -> Vec<child_names>
    directories: RwLock<HashMap<String, Vec<String>>>,
    /// Metadata stored as path -> TreeEntry
    entries: RwLock<HashMap<String, TreeEntry>>,
    /// Lock state stored as path -> (timestamp, checksum)
    locks: RwLock<HashMap<String, (u64, [u8; 32])>>,
    /// Version counter
    version: AtomicU64,
    /// Inode counter
    next_inode: AtomicU64,
}

impl MockMemDb {
    /// Create a new empty mock database
    pub fn new() -> Self {
        let mut directories = HashMap::new();
        directories.insert("/".to_string(), Vec::new());

        let mut entries = HashMap::new();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;

        // Create root entry
        entries.insert(
            "/".to_string(),
            TreeEntry {
                inode: ROOT_INODE,
                parent: 0,
                version: 0,
                writer: 1,
                mtime: now,
                size: 0,
                entry_type: DT_DIR,
                data: Vec::new(),
                name: String::new(),
            },
        );

        Self {
            files: RwLock::new(HashMap::new()),
            directories: RwLock::new(directories),
            entries: RwLock::new(entries),
            locks: RwLock::new(HashMap::new()),
            version: AtomicU64::new(1),
            next_inode: AtomicU64::new(ROOT_INODE + 1),
        }
    }

    /// Helper to check if path is a directory
    fn is_directory(&self, path: &str) -> bool {
        self.directories.read().contains_key(path)
    }

    /// Helper to get parent path
    fn parent_path(path: &str) -> Option<String> {
        if path == "/" {
            return None;
        }
        let parent = path.rsplit_once('/')?.0;
        if parent.is_empty() {
            Some("/".to_string())
        } else {
            Some(parent.to_string())
        }
    }

    /// Helper to get file name from path
    fn file_name(path: &str) -> String {
        if path == "/" {
            return String::new();
        }
        path.rsplit('/').next().unwrap_or("").to_string()
    }
}

impl Default for MockMemDb {
    fn default() -> Self {
        Self::new()
    }
}

impl MemDbOps for MockMemDb {
    fn create(&self, path: &str, mode: u32, _writer: u32, mtime: u32) -> Result<()> {
        if path.is_empty() {
            bail!("Empty path");
        }

        if self.entries.read().contains_key(path) {
            bail!("File exists: {}", path);
        }

        let is_dir = (mode & libc::S_IFMT) == libc::S_IFDIR;
        let entry_type = if is_dir { DT_DIR } else { DT_REG };
        let inode = self.next_inode.fetch_add(1, Ordering::SeqCst);

        // Add to parent directory
        if let Some(parent) = Self::parent_path(path) {
            if !self.is_directory(&parent) {
                bail!("Parent is not a directory: {}", parent);
            }
            let mut dirs = self.directories.write();
            if let Some(children) = dirs.get_mut(&parent) {
                children.push(Self::file_name(path));
            }
        }

        // Create entry
        let entry = TreeEntry {
            inode,
            parent: 0, // Simplified
            version: self.version.load(Ordering::SeqCst),
            writer: 1,
            mtime,
            size: 0,
            entry_type,
            data: Vec::new(),
            name: Self::file_name(path),
        };

        self.entries.write().insert(path.to_string(), entry);

        if is_dir {
            self.directories
                .write()
                .insert(path.to_string(), Vec::new());
        } else {
            self.files.write().insert(path.to_string(), Vec::new());
        }

        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn read(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>> {
        let files = self.files.read();
        let data = files
            .get(path)
            .ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

        let offset = offset as usize;
        if offset >= data.len() {
            return Ok(Vec::new());
        }

        let end = std::cmp::min(offset + size, data.len());
        Ok(data[offset..end].to_vec())
    }

    fn write(
        &self,
        path: &str,
        offset: u64,
        _writer: u32,
        mtime: u32,
        data: &[u8],
        truncate: bool,
    ) -> Result<usize> {
        let mut files = self.files.write();
        let file_data = files
            .get_mut(path)
            .ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

        let offset = offset as usize;

        if truncate {
            file_data.clear();
        }

        // Expand if needed
        if offset + data.len() > file_data.len() {
            file_data.resize(offset + data.len(), 0);
        }

        file_data[offset..offset + data.len()].copy_from_slice(data);

        // Update entry
        if let Some(entry) = self.entries.write().get_mut(path) {
            entry.mtime = mtime;
            entry.size = file_data.len();
        }

        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(data.len())
    }

    fn delete(&self, path: &str, _writer: u32, _mtime: u32) -> Result<()> {
        if !self.entries.read().contains_key(path) {
            bail!("File not found: {}", path);
        }

        // Check if directory is empty
        if let Some(children) = self.directories.read().get(path) {
            if !children.is_empty() {
                bail!("Directory not empty: {}", path);
            }
        }

        self.entries.write().remove(path);
        self.files.write().remove(path);
        self.directories.write().remove(path);

        // Remove from parent
        if let Some(parent) = Self::parent_path(path) {
            if let Some(children) = self.directories.write().get_mut(&parent) {
                children.retain(|name| name != &Self::file_name(path));
            }
        }

        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn rename(&self, old_path: &str, new_path: &str, _writer: u32, _mtime: u32) -> Result<()> {
        // Hold write locks for entire operation to avoid TOCTOU race condition
        let mut entries = self.entries.write();
        let mut files = self.files.write();
        let mut directories = self.directories.write();

        // Check existence
        if !entries.contains_key(old_path) {
            bail!("Source not found: {}", old_path);
        }
        if entries.contains_key(new_path) {
            bail!("Destination already exists: {}", new_path);
        }

        let is_dir = directories.contains_key(old_path);

        // Update parent directory children lists
        if let Some(old_parent) = Self::parent_path(old_path) {
            if let Some(children) = directories.get_mut(&old_parent) {
                children.retain(|name| name != &Self::file_name(old_path));
            }
        }
        if let Some(new_parent) = Self::parent_path(new_path) {
            if let Some(children) = directories.get_mut(&new_parent) {
                children.push(Self::file_name(new_path));
            }
        }

        // If renaming a directory, update all descendant paths
        if is_dir {
            let old_prefix = if old_path == "/" {
                "/".to_string()
            } else {
                format!("{}/", old_path)
            };
            let new_prefix = if new_path == "/" {
                "/".to_string()
            } else {
                format!("{}/", new_path)
            };

            // Collect all paths that need to be updated
            let paths_to_update: Vec<String> = entries
                .keys()
                .filter(|p| p.starts_with(&old_prefix))
                .cloned()
                .collect();

            // Update each descendant path
            for old_descendant in paths_to_update {
                let new_descendant = old_descendant.replacen(&old_prefix, &new_prefix, 1);

                // Move entry
                if let Some(mut entry) = entries.remove(&old_descendant) {
                    entry.name = Self::file_name(&new_descendant);
                    entries.insert(new_descendant.clone(), entry);
                }

                // Move file data
                if let Some(data) = files.remove(&old_descendant) {
                    files.insert(new_descendant.clone(), data);
                }

                // Move directory
                if let Some(children) = directories.remove(&old_descendant) {
                    directories.insert(new_descendant, children);
                }
            }
        }

        // Move the entry itself
        if let Some(mut entry) = entries.remove(old_path) {
            entry.name = Self::file_name(new_path);
            entries.insert(new_path.to_string(), entry);
        }

        // Move file data
        if let Some(data) = files.remove(old_path) {
            files.insert(new_path.to_string(), data);
        }

        // Move directory
        if let Some(children) = directories.remove(old_path) {
            directories.insert(new_path.to_string(), children);
        }

        drop(entries);
        drop(files);
        drop(directories);

        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.entries.read().contains_key(path))
    }

    fn readdir(&self, path: &str) -> Result<Vec<TreeEntry>> {
        let directories = self.directories.read();
        let children = directories
            .get(path)
            .ok_or_else(|| anyhow::anyhow!("Not a directory: {}", path))?;

        let entries = self.entries.read();
        let mut result = Vec::new();

        for child_name in children {
            let child_path = if path == "/" {
                format!("/{}", child_name)
            } else {
                format!("{}/{}", path, child_name)
            };

            if let Some(entry) = entries.get(&child_path) {
                result.push(entry.clone());
            }
        }

        Ok(result)
    }

    fn set_mtime(&self, path: &str, _writer: u32, mtime: u32) -> Result<()> {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(path)
            .ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;
        entry.mtime = mtime;
        Ok(())
    }

    fn lookup_path(&self, path: &str) -> Option<TreeEntry> {
        self.entries.read().get(path).cloned()
    }

    fn get_entry_by_inode(&self, inode: u64) -> Option<TreeEntry> {
        self.entries
            .read()
            .values()
            .find(|e| e.inode == inode)
            .cloned()
    }

    fn acquire_lock(&self, path: &str, csum: &[u8; 32]) -> Result<()> {
        let mut locks = self.locks.write();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let key = lock_cache_key(path);

        if let Some((timestamp, existing_csum)) = locks.get(&key) {
            // Check if expired
            if now - timestamp > LOCK_TIMEOUT_SECS {
                // Expired, can acquire
                locks.insert(key, (now, *csum));
                return Ok(());
            }

            // Not expired, check if same checksum (refresh)
            if existing_csum == csum {
                locks.insert(key, (now, *csum));
                return Ok(());
            }

            bail!("Lock already held with different checksum");
        }

        locks.insert(key, (now, *csum));
        Ok(())
    }

    fn release_lock(&self, path: &str, csum: &[u8; 32]) -> Result<()> {
        let mut locks = self.locks.write();
        let key = lock_cache_key(path);
        if let Some((_, existing_csum)) = locks.get(&key) {
            if existing_csum == csum {
                locks.remove(&key);
                return Ok(());
            }
            bail!("Lock checksum mismatch");
        }
        bail!("No lock found");
    }

    fn is_locked(&self, path: &str) -> bool {
        let key = lock_cache_key(path);
        if let Some((timestamp, _)) = self.locks.read().get(&key) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            now - timestamp <= LOCK_TIMEOUT_SECS
        } else {
            false
        }
    }

    fn lock_expired(&self, path: &str, csum: &[u8; 32]) -> bool {
        let key = lock_cache_key(path);
        if let Some((timestamp, existing_csum)) = self.locks.read().get(&key).cloned() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            // If checksum mismatches, this is a different lock holder attempting
            // to check expiration. Reset the timeout to prevent premature expiration
            // while the current holder still has the lock. This matches the C
            // implementation's behavior where lock_expired() with wrong checksum
            // extends the lock timeout.
            if &existing_csum != csum {
                self.locks.write().insert(key, (now, *csum));
                return false;
            }

            // Check expiration
            now - timestamp > LOCK_TIMEOUT_SECS
        } else {
            false
        }
    }

    fn get_version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    fn get_all_entries(&self) -> Result<Vec<TreeEntry>> {
        Ok(self.entries.read().values().cloned().collect())
    }

    fn replace_all_entries(&self, entries: Vec<TreeEntry>) -> Result<()> {
        // Preserve root entry before clearing
        let root_entry = self.entries.read().get("/").cloned();

        // Acquire all write locks once (in correct order to avoid deadlocks)
        let mut entries_map = self.entries.write();
        let mut files_map = self.files.write();
        let mut dirs_map = self.directories.write();

        // Clear all data
        entries_map.clear();
        files_map.clear();
        dirs_map.clear();

        // Restore root entry to preserve invariant
        if let Some(root) = root_entry {
            entries_map.insert("/".to_string(), root);
            dirs_map.insert("/".to_string(), Vec::new());
        }

        // Insert all entries
        for entry in entries {
            let path = format!("/{}", entry.name); // Simplified
            entries_map.insert(path.clone(), entry.clone());

            // Use entry_type to distinguish files from directories
            if entry.entry_type == DT_REG {
                files_map.insert(path, entry.data.clone());
            } else if entry.entry_type == DT_DIR {
                dirs_map.insert(path, Vec::new());
            }
        }

        // Rebuild parent-child relationships
        let paths: Vec<String> = entries_map.keys().cloned().collect();
        for path in paths {
            if let Some(entry) = entries_map.get(&path) {
                if let Some(parent) = Self::parent_path(&path) {
                    if let Some(children) = dirs_map.get_mut(&parent) {
                        if !children.contains(&entry.name) {
                            children.push(entry.name.clone());
                        }
                    }
                }
            }
        }

        drop(entries_map);
        drop(files_map);
        drop(dirs_map);

        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn apply_tree_entry(&self, entry: TreeEntry) -> Result<()> {
        let path = format!("/{}", entry.name); // Simplified

        // Acquire locks once
        let mut entries_map = self.entries.write();
        let mut files_map = self.files.write();
        let mut dirs_map = self.directories.write();

        entries_map.insert(path.clone(), entry.clone());

        // Use entry_type to distinguish files from directories
        if entry.entry_type == DT_REG {
            files_map.insert(path.clone(), entry.data.clone());
        } else if entry.entry_type == DT_DIR {
            dirs_map.insert(path.clone(), Vec::new());
        }

        // Update parent-child relationship
        if let Some(parent) = Self::parent_path(&path) {
            if let Some(children) = dirs_map.get_mut(&parent) {
                if !children.contains(&entry.name) {
                    children.push(entry.name.clone());
                }
            }
        }

        drop(entries_map);
        drop(files_map);
        drop(dirs_map);

        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn encode_database(&self) -> Result<Vec<u8>> {
        // Simplified - just return empty vec
        Ok(Vec::new())
    }

    fn compute_database_checksum(&self) -> Result<[u8; 32]> {
        // Simplified - return deterministic checksum based on version
        let version = self.version.load(Ordering::SeqCst);
        let mut checksum = [0u8; 32];
        checksum[0..8].copy_from_slice(&version.to_le_bytes());
        Ok(checksum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_mock_memdb_basic_operations() {
        let db = MockMemDb::new();

        // Create file
        db.create("/test.txt", libc::S_IFREG, 0, 1234).unwrap();
        assert!(db.exists("/test.txt").unwrap());

        // Write data
        let data = b"Hello, MockMemDb!";
        db.write("/test.txt", 0, 0, 1235, data, false).unwrap();

        // Read data
        let read_data = db.read("/test.txt", 0, 100).unwrap();
        assert_eq!(&read_data[..], data);

        // Check entry
        let entry = db.lookup_path("/test.txt").unwrap();
        assert_eq!(entry.size, data.len());
        assert_eq!(entry.mtime, 1235);
    }

    #[test]
    fn test_mock_memdb_directory_operations() {
        let db = MockMemDb::new();

        // Create directory
        db.create("/mydir", libc::S_IFDIR, 0, 1000).unwrap();
        assert!(db.exists("/mydir").unwrap());

        // Create file in directory
        db.create("/mydir/file.txt", libc::S_IFREG, 0, 1001).unwrap();

        // Read directory
        let entries = db.readdir("/mydir").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "file.txt");
    }

    #[test]
    fn test_mock_memdb_lock_operations() {
        let db = MockMemDb::new();
        let csum1 = [1u8; 32];
        let csum2 = [2u8; 32];

        // Acquire lock
        db.acquire_lock("/priv/lock/resource", &csum1).unwrap();
        assert!(db.is_locked("/priv/lock/resource"));

        // Lock with same checksum should succeed (refresh)
        assert!(db.acquire_lock("/priv/lock/resource", &csum1).is_ok());

        // Lock with different checksum should fail
        assert!(db.acquire_lock("/priv/lock/resource", &csum2).is_err());

        // Release lock
        db.release_lock("/priv/lock/resource", &csum1).unwrap();
        assert!(!db.is_locked("/priv/lock/resource"));

        // Can acquire with different checksum now
        db.acquire_lock("/priv/lock/resource", &csum2).unwrap();
        assert!(db.is_locked("/priv/lock/resource"));
    }

    #[test]
    fn test_mock_memdb_rename() {
        let db = MockMemDb::new();

        // Create file
        db.create("/old.txt", libc::S_IFREG, 0, 1000).unwrap();
        db.write("/old.txt", 0, 0, 1001, b"content", false).unwrap();

        // Rename
        db.rename("/old.txt", "/new.txt", 0, 1000).unwrap();

        // Old path should not exist
        assert!(!db.exists("/old.txt").unwrap());

        // New path should exist with same content
        assert!(db.exists("/new.txt").unwrap());
        let data = db.read("/new.txt", 0, 100).unwrap();
        assert_eq!(&data[..], b"content");
    }

    #[test]
    fn test_mock_memdb_delete() {
        let db = MockMemDb::new();

        // Create and delete file
        db.create("/delete-me.txt", libc::S_IFREG, 0, 1000).unwrap();
        assert!(db.exists("/delete-me.txt").unwrap());

        db.delete("/delete-me.txt", 0, 1000).unwrap();
        assert!(!db.exists("/delete-me.txt").unwrap());

        // Delete non-existent file should fail
        assert!(db.delete("/nonexistent.txt", 0, 1000).is_err());
    }

    #[test]
    fn test_mock_memdb_version_tracking() {
        let db = MockMemDb::new();
        let initial_version = db.get_version();

        // Version should increment on modifications
        db.create("/file1.txt", libc::S_IFREG, 0, 1000).unwrap();
        assert!(db.get_version() > initial_version);

        let v1 = db.get_version();
        db.write("/file1.txt", 0, 0, 1001, b"data", false).unwrap();
        assert!(db.get_version() > v1);

        let v2 = db.get_version();
        db.delete("/file1.txt", 0, 1000).unwrap();
        assert!(db.get_version() > v2);
    }

    #[test]
    fn test_mock_memdb_isolation() {
        // Each MockMemDb instance is completely isolated
        let db1 = MockMemDb::new();
        let db2 = MockMemDb::new();

        db1.create("/test.txt", libc::S_IFREG, 0, 1000).unwrap();

        // db2 should not see db1's files
        assert!(db1.exists("/test.txt").unwrap());
        assert!(!db2.exists("/test.txt").unwrap());
    }

    #[test]
    fn test_mock_memdb_as_trait_object() {
        // Demonstrate using MockMemDb through trait object
        let db: Arc<dyn MemDbOps> = Arc::new(MockMemDb::new());

        db.create("/trait-test.txt", libc::S_IFREG, 0, 2000).unwrap();
        assert!(db.exists("/trait-test.txt").unwrap());

        db.write("/trait-test.txt", 0, 0, 2001, b"via trait", false)
            .unwrap();
        let data = db.read("/trait-test.txt", 0, 100).unwrap();
        assert_eq!(&data[..], b"via trait");
    }

    #[test]
    fn test_mock_memdb_error_cases() {
        let db = MockMemDb::new();

        // Create duplicate should fail
        db.create("/dup.txt", libc::S_IFREG, 0, 1000).unwrap();
        assert!(db.create("/dup.txt", libc::S_IFREG, 0, 1000).is_err());

        // Read non-existent file should fail
        assert!(db.read("/nonexistent.txt", 0, 100).is_err());

        // Write to non-existent file should fail
        assert!(
            db.write("/nonexistent.txt", 0, 0, 1000, b"data", false)
                .is_err()
        );

        // Empty path should fail
        assert!(db.create("", libc::S_IFREG, 0, 1000).is_err());
    }
}
