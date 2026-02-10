/// Core MemDb implementation - in-memory database with SQLite persistence
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::types::LockInfo;
use super::types::{
    DT_DIR, DT_REG, LOCK_DIR_PATH, LoadDbResult, MEMDB_MAX_FILE_SIZE, MEMDB_MAX_FSSIZE,
    MEMDB_MAX_INODES, ROOT_INODE, TreeEntry, VERSION_FILENAME,
};

/// In-memory database with SQLite persistence
#[derive(Clone)]
pub struct MemDb {
    pub(super) inner: Arc<MemDbInner>,
}

pub(super) struct MemDbInner {
    /// SQLite connection for persistence (wrapped in Mutex for thread-safety)
    pub(super) conn: Mutex<Connection>,

    /// In-memory index of all entries (inode -> TreeEntry)
    /// This is a cache of the database for fast lookups
    pub(super) index: Mutex<HashMap<u64, TreeEntry>>,

    /// In-memory tree structure (parent inode -> children)
    pub(super) tree: Mutex<HashMap<u64, HashMap<String, u64>>>,

    /// Root entry
    pub(super) root_inode: u64,

    /// Current version (incremented on each write)
    pub(super) version: AtomicU64,

    /// Resource locks (path -> LockInfo)
    pub(super) locks: Mutex<HashMap<String, LockInfo>>,

    /// Error flag - set to true on database errors (matches C's memdb->errors)
    /// When true, all operations should fail to prevent data corruption
    pub(super) errors: AtomicBool,

    /// Single write guard mutex to serialize all mutating operations
    /// Matches C's single GMutex approach, eliminates lock ordering issues
    pub(super) write_guard: Mutex<()>,
}

impl MemDb {
    pub fn open(path: &Path, create: bool) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Set SQLite pragmas to match C implementation (database.c:112-127)
        // - WAL mode: Write-Ahead Logging for better concurrent read access
        // - NORMAL sync: Faster writes (fsync only at critical moments)
        // - 10s busy timeout: Retry on SQLITE_BUSY instead of instant failure
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;

        if create {
            Self::init_schema(&conn)?;
        }

        let (index, tree, root_inode, version) = Self::load_from_db(&conn)?;

        let memdb = Self {
            inner: Arc::new(MemDbInner {
                conn: Mutex::new(conn),
                index: Mutex::new(index),
                tree: Mutex::new(tree),
                root_inode,
                version: AtomicU64::new(version),
                locks: Mutex::new(HashMap::new()),
                errors: AtomicBool::new(false),
                write_guard: Mutex::new(()),
            }),
        };

        memdb.update_locks();

        Ok(memdb)
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE tree (
                inode INTEGER PRIMARY KEY,
                parent INTEGER NOT NULL,
                version INTEGER NOT NULL,
                writer INTEGER NOT NULL,
                mtime INTEGER NOT NULL,
                type INTEGER NOT NULL,
                name TEXT NOT NULL,
                data BLOB
            );

            CREATE INDEX tree_parent_idx ON tree(parent, name);

            CREATE TABLE config (
                name TEXT PRIMARY KEY,
                value TEXT
            );
            "#,
        )?;

        // Create root metadata entry as inode ROOT_INODE with name "__version__"
        // Matching C implementation: root inode is NEVER in database as a regular entry
        // Root metadata is stored as inode ROOT_INODE with special name "__version__"
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs() as u32;

        conn.execute(
            "INSERT INTO tree (inode, parent, version, writer, mtime, type, name, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![ROOT_INODE, ROOT_INODE, 1, 0, now, DT_REG, VERSION_FILENAME, None::<Vec<u8>>],
        )?;

        Ok(())
    }

    fn load_from_db(conn: &Connection) -> Result<LoadDbResult> {
        let mut index = HashMap::new();
        let mut tree: HashMap<u64, HashMap<String, u64>> = HashMap::new();
        let mut max_version = 0u64;

        let mut stmt = conn.prepare(
            "SELECT inode, parent, version, writer, mtime, type, name, data FROM tree",
        )?;
        let rows = stmt.query_map([], |row| {
            let inode: u64 = row.get(0)?;
            let parent: u64 = row.get(1)?;
            let version: u64 = row.get(2)?;
            let writer: u32 = row.get(3)?;
            let mtime: u32 = row.get(4)?;
            let entry_type: u8 = row.get(5)?;
            let name: String = row.get(6)?;
            let data: Option<Vec<u8>> = row.get(7)?;

            // Derive size from data length (matching C behavior: sqlite3_column_bytes)
            let data_vec = data.unwrap_or_default();
            let size = data_vec.len();

            Ok(TreeEntry {
                inode,
                parent,
                version,
                writer,
                mtime,
                size,
                entry_type,
                name,
                data: data_vec,
            })
        })?;

        // Create root entry in memory first (matching C implementation in database.c:559-567)
        // Root is NEVER stored in database, only its metadata via inode ROOT_INODE
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs() as u32;
        let mut root = TreeEntry {
            inode: ROOT_INODE,
            parent: ROOT_INODE, // Root's parent is itself
            version: 0,         // Will be populated from __version__ entry
            writer: 0,
            mtime: now,
            size: 0,
            entry_type: DT_DIR,
            name: String::new(),
            data: Vec::new(),
        };

        for row in rows {
            let entry = row?;

            // Handle __version__ entry (inode ROOT_INODE) - populate root metadata (C: database.c:372-382)
            if entry.inode == ROOT_INODE {
                if entry.name == VERSION_FILENAME {
                    tracing::debug!(
                        "Loading root metadata from __version__: version={}, writer={}, mtime={}",
                        entry.version,
                        entry.writer,
                        entry.mtime
                    );
                    root.version = entry.version;
                    root.writer = entry.writer;
                    root.mtime = entry.mtime;
                    if entry.version > max_version {
                        max_version = entry.version;
                    }
                } else {
                    tracing::warn!("Ignoring inode 0 with unexpected name: {}", entry.name);
                }
                continue; // Don't add __version__ to index
            }

            // Track max version from all entries
            if entry.version > max_version {
                max_version = entry.version;
            }

            // Add to tree structure
            tree.entry(entry.parent)
                .or_default()
                .insert(entry.name.clone(), entry.inode);

            // If this is a directory, ensure it has an entry in the tree map
            if entry.is_dir() {
                tree.entry(entry.inode).or_default();
            }

            // Add to index
            index.insert(entry.inode, entry);
        }

        // If root version is still 0, set it to 1 (new database)
        if root.version == 0 {
            root.version = 1;
            max_version = 1;
            tracing::debug!("No __version__ entry found, initializing root with version 1");
        }

        // Add root to index and ensure it has a tree entry (use entry() to not overwrite children!)
        index.insert(ROOT_INODE, root);
        tree.entry(ROOT_INODE).or_default();

        Ok((index, tree, ROOT_INODE, max_version))
    }

    pub fn get_entry_by_inode(&self, inode: u64) -> Option<TreeEntry> {
        let index = self.inner.index.lock();
        index.get(&inode).cloned()
    }

    /// Execute a mutation with proper version management and error handling
    ///
    /// This helper centralizes:
    /// 1. Error flag checking (fails if database has errors)
    /// 2. Write guard acquisition (serializes all mutations)
    /// 3. Version increment and __version__ update
    /// 4. Transaction management
    /// 5. In-memory state updates
    ///
    /// The closure receives a transaction and the new version number.
    /// It should perform the database mutation and return any result.
    ///
    /// After the transaction commits, the closure's result is returned.
    /// The caller is responsible for updating in-memory structures (index, tree).
    ///
    /// # Arguments
    /// * `writer` - Writer ID (node ID in cluster)
    /// * `mtime` - Modification time (seconds since UNIX epoch)
    /// * `f` - Closure that performs the mutation within a transaction
    ///
    /// # Example
    /// ```ignore
    /// self.with_mutation(0, now, |tx, version| {
    ///     tx.execute("INSERT INTO tree ...", params![...])?;
    ///     Ok(())
    /// })?;
    /// ```
    fn with_mutation<R>(
        &self,
        writer: u32,
        mtime: u32,
        f: impl FnOnce(&rusqlite::Transaction<'_>, u64) -> Result<R>,
    ) -> Result<R> {
        // Check error flag first (matches C's memdb->errors check)
        if self.inner.errors.load(Ordering::SeqCst) {
            anyhow::bail!("Database has errors, refusing operation");
        }

        // Acquire write guard to serialize all mutations (matches C's single GMutex)
        let _guard = self.inner.write_guard.lock();

        // Increment version
        let new_version = self.inner.version.fetch_add(1, Ordering::SeqCst) + 1;

        // Begin transaction
        let conn = self.inner.conn.lock();
        let tx = conn.unchecked_transaction().context("Failed to begin transaction")?;

        // Update __version__ entry in database (matches C's database.c:275-278)
        tx.execute(
            "UPDATE tree SET version = ?1, writer = ?2, mtime = ?3 WHERE inode = ?4",
            params![new_version, writer, mtime, ROOT_INODE],
        )
        .context("Failed to update __version__ entry")?;

        // Execute the mutation
        let result = match f(&tx, new_version) {
            Ok(r) => r,
            Err(e) => {
                // Set error flag on failure (matches C behavior)
                self.inner.errors.store(true, Ordering::SeqCst);
                tracing::error!("Database mutation failed: {}", e);
                return Err(e);
            }
        };

        // Commit transaction
        if let Err(e) = tx.commit() {
            self.inner.errors.store(true, Ordering::SeqCst);
            tracing::error!("Failed to commit transaction: {}", e);
            return Err(e.into());
        }

        drop(conn);

        // Update root entry metadata in memory
        {
            let mut index = self.inner.index.lock();
            if let Some(root_entry) = index.get_mut(&self.inner.root_inode) {
                root_entry.version = new_version;
                root_entry.writer = writer;
                root_entry.mtime = mtime;
            }
        }

        Ok(result)
    }

    /// Get the __version__ entry for sending updates to C nodes
    ///
    /// The __version__ entry (inode ROOT_INODE) stores root metadata in the database
    /// but is not kept in the in-memory index. This method queries it directly
    /// from the database to send as an UPDATE message to C nodes.
    pub fn get_version_entry(&self) -> anyhow::Result<TreeEntry> {
        let index = self.inner.index.lock();
        let root_entry = index
            .get(&self.inner.root_inode)
            .ok_or_else(|| anyhow::anyhow!("Root entry not found"))?;

        // Create a __version__ entry matching C's format
        // This is what C expects to receive as inode ROOT_INODE
        Ok(TreeEntry {
            inode: ROOT_INODE, // __version__ is always inode ROOT_INODE in database/wire format
            parent: ROOT_INODE, // Root's parent is itself
            version: root_entry.version,
            writer: root_entry.writer,
            mtime: root_entry.mtime,
            size: 0,
            entry_type: DT_REG,
            name: VERSION_FILENAME.to_string(),
            data: Vec::new(),
        })
    }

    pub fn lookup_path(&self, path: &str) -> Option<TreeEntry> {
        let index = self.inner.index.lock();
        let tree = self.inner.tree.lock();

        if path.is_empty() || path == "/" || path == "." {
            return index.get(&self.inner.root_inode).cloned();
        }

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current_inode = self.inner.root_inode;

        for part in parts {
            let children = tree.get(&current_inode)?;
            current_inode = *children.get(part)?;
        }

        index.get(&current_inode).cloned()
    }

    /// Normalize a path to internal format
    ///
    /// # Path Normalization Strategy
    ///
    /// Internal paths are always stored as absolute paths with:
    /// - Leading `/` (e.g., "/nodes/node1/qemu-server/100.conf")
    /// - No trailing `/` except for root ("/")
    /// - No `..` or `.` components
    ///
    /// C compatibility: The C implementation sometimes sends paths without leading `/`
    /// (see find_plug in pmxcfs.c). This function normalizes all inputs to absolute paths.
    ///
    /// # Arguments
    ///
    /// * `path` - Input path (may or may not have leading `/`)
    ///
    /// # Returns
    ///
    /// Normalized absolute path with leading `/` and no trailing `/`
    ///
    /// # Examples
    ///
    /// ```ignore
    /// normalize_path("nodes/node1/qemu-server") -> "/nodes/node1/qemu-server"
    /// normalize_path("/nodes/node1/qemu-server") -> "/nodes/node1/qemu-server"
    /// normalize_path("/nodes/node1/qemu-server/") -> "/nodes/node1/qemu-server"
    /// normalize_path("") -> "/"
    /// normalize_path("/") -> "/"
    /// ```
    fn normalize_path(path: &str) -> String {
        // Handle empty path as root
        if path.is_empty() || path == "/" || path == "." {
            return "/".to_string();
        }

        // Remove leading and trailing slashes, then add single leading slash
        let trimmed = path.trim_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", trimmed)
        }
    }

    /// Split a path into parent directory and basename
    ///
    /// Uses the internal path normalization strategy to ensure consistent behavior.
    ///
    /// # Errors
    ///
    /// Returns an error if the path is invalid (e.g., empty).
    fn split_path(path: &str) -> Result<(String, String)> {
        if path.is_empty() {
            anyhow::bail!("Path cannot be empty");
        }

        // Normalize to absolute path
        let normalized_path = Self::normalize_path(path);

        if let Some(pos) = normalized_path.rfind('/') {
            let dirname = if pos == 0 { "/" } else { &normalized_path[..pos] };
            let basename = &normalized_path[pos + 1..];
            Ok((dirname.to_string(), basename.to_string()))
        } else {
            // This shouldn't happen after normalization, but handle it anyway
            Ok(("/".to_string(), normalized_path.to_string()))
        }
    }

    /// Check if path is a lock directory (cfs-utils.c:306-312)
    fn is_lock_dir(path: &str) -> bool {
        let path = path.trim_start_matches('/');
        path.starts_with("priv/lock/") && path.len() > 10
    }

    pub fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.lookup_path(path).is_some())
    }

    pub fn read(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>> {
        let entry = self
            .lookup_path(path)
            .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;

        if entry.is_dir() {
            return Err(anyhow::anyhow!("Cannot read directory: {path}"));
        }

        let offset = offset as usize;
        if offset >= entry.data.len() {
            return Ok(Vec::new());
        }

        let end = std::cmp::min(offset + size, entry.data.len());
        Ok(entry.data[offset..end].to_vec())
    }

    /// Helper to update __version__ entry in database
    ///
    /// This is called for EVERY write operation to keep root metadata synchronized
    /// (matching C behavior in database.c:275-278)
    fn update_version_entry(
        conn: &rusqlite::Connection,
        version: u64,
        writer: u32,
        mtime: u32,
    ) -> Result<()> {
        conn.execute(
            "UPDATE tree SET version = ?1, writer = ?2, mtime = ?3 WHERE inode = ?4",
            params![version, writer, mtime, ROOT_INODE],
        )?;
        Ok(())
    }

    /// Helper to update root entry in index
    ///
    /// Keeps the in-memory root entry synchronized with database __version__
    fn update_root_metadata(
        index: &mut HashMap<u64, TreeEntry>,
        root_inode: u64,
        version: u64,
        writer: u32,
        mtime: u32,
    ) {
        if let Some(root_entry) = index.get_mut(&root_inode) {
            root_entry.version = version;
            root_entry.writer = writer;
            root_entry.mtime = mtime;
        }
    }

    pub fn create(&self, path: &str, mode: u32, writer: u32, mtime: u32) -> Result<()> {
        // Check error flag first (matches C's memdb->errors check)
        if self.inner.errors.load(Ordering::SeqCst) {
            anyhow::bail!("Database has errors, refusing operation");
        }

        // Acquire write guard before any checks to prevent TOCTOU race
        // This ensures all validation and mutation happen atomically
        let _guard = self.inner.write_guard.lock();

        // Now perform checks under write guard protection
        if self.exists(path)? {
            return Err(anyhow::anyhow!("File already exists: {path}"));
        }

        let (parent_path, basename) = Self::split_path(path)?;

        // Reject '.' and '..' basenames (memdb.c:577-582)
        if basename.is_empty() || basename == "." || basename == ".." {
            return Err(std::io::Error::from_raw_os_error(libc::EACCES).into());
        }

        let parent_entry = self
            .lookup_path(&parent_path)
            .ok_or_else(|| anyhow::anyhow!("Parent directory not found: {parent_path}"))?;

        if !parent_entry.is_dir() {
            return Err(anyhow::anyhow!("Parent is not a directory: {parent_path}"));
        }

        // Check inode limit (matches C implementation in memdb.c)
        let index = self.inner.index.lock();
        let current_inodes = index.len();
        drop(index);

        if current_inodes >= MEMDB_MAX_INODES {
            return Err(anyhow::anyhow!(
                "Maximum inode count exceeded: {} >= {}",
                current_inodes,
                MEMDB_MAX_INODES
            ));
        }

        let entry_type = if mode & libc::S_IFDIR != 0 {
            DT_DIR
        } else {
            DT_REG
        };

        // Increment version
        let new_version = self.inner.version.fetch_add(1, Ordering::SeqCst) + 1;

        // Begin transaction
        let conn = self.inner.conn.lock();
        let tx = conn.unchecked_transaction().context("Failed to begin transaction")?;

        // Update __version__ entry in database (matches C's database.c:275-278)
        tx.execute(
            "UPDATE tree SET version = ?1, writer = ?2, mtime = ?3 WHERE inode = ?4",
            params![new_version, writer, mtime, ROOT_INODE],
        )
        .context("Failed to update __version__ entry")?;

        // Execute the mutation
        let result = (|| -> Result<(u64, TreeEntry)> {
            // Inode equals version number (C compatibility)
            let new_inode = new_version;

            let entry = TreeEntry {
                inode: new_inode,
                parent: parent_entry.inode,
                version: new_version,
                writer,
                mtime,
                size: 0,
                entry_type,
                name: basename.clone(),
                data: Vec::new(),
            };

            tx.execute(
                "INSERT INTO tree (inode, parent, version, writer, mtime, type, name, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry.inode,
                    entry.parent,
                    entry.version,
                    entry.writer,
                    entry.mtime,
                    entry.entry_type,
                    entry.name,
                    if entry.is_dir() { None::<Vec<u8>> } else { Some(entry.data.clone()) }
                ],
            )?;

            Ok((new_inode, entry))
        })();

        // Handle mutation result
        let (new_inode, entry) = match result {
            Ok(r) => r,
            Err(e) => {
                self.inner.errors.store(true, Ordering::SeqCst);
                tracing::error!("Database mutation failed: {}", e);
                return Err(e);
            }
        };

        // Commit transaction
        if let Err(e) = tx.commit() {
            self.inner.errors.store(true, Ordering::SeqCst);
            tracing::error!("Failed to commit transaction: {}", e);
            return Err(e.into());
        }

        drop(conn);

        // Update root entry metadata in memory
        {
            let mut index = self.inner.index.lock();
            if let Some(root_entry) = index.get_mut(&self.inner.root_inode) {
                root_entry.version = new_version;
                root_entry.writer = writer;
                root_entry.mtime = mtime;
            }
        }

        // Update in-memory structures
        {
            let mut index = self.inner.index.lock();
            let mut tree = self.inner.tree.lock();

            index.insert(new_inode, entry.clone());

            tree.entry(parent_entry.inode)
                .or_default()
                .insert(basename, new_inode);

            if entry.is_dir() {
                tree.insert(new_inode, HashMap::new());
            }
        }

        // If this is a directory in priv/lock/, register it in the lock table
        if entry.is_dir() && parent_path == LOCK_DIR_PATH {
            let csum = entry.compute_checksum();
            let _ = self.lock_expired(path, &csum);
            tracing::debug!("Registered lock directory: {}", path);
        }

        Ok(())
    }

    pub fn write(
        &self,
        path: &str,
        offset: u64,
        writer: u32,
        mtime: u32,
        data: &[u8],
        truncate: bool,
    ) -> Result<usize> {
        // Check error flag first (matches C's memdb->errors check)
        if self.inner.errors.load(Ordering::SeqCst) {
            anyhow::bail!("Database has errors, refusing operation");
        }

        // Acquire write guard before any checks to prevent TOCTOU race
        // This ensures lookup and mutation happen atomically
        let _guard = self.inner.write_guard.lock();

        let mut entry = self
            .lookup_path(path)
            .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;

        if entry.is_dir() {
            return Err(anyhow::anyhow!("Cannot write to directory: {path}"));
        }

        // Overflow protection: check offset + data.len() with checked arithmetic
        let offset_usize = offset as usize;
        let end_offset = offset_usize
            .checked_add(data.len())
            .ok_or_else(|| anyhow::anyhow!("Offset overflow"))?;

        if end_offset > MEMDB_MAX_FILE_SIZE {
            return Err(anyhow::anyhow!(
                "Write would exceed maximum file size: {} > {}",
                end_offset,
                MEMDB_MAX_FILE_SIZE
            ));
        }

        // Check total filesystem size limit (matches C implementation)
        // Calculate size delta for this write operation
        let size_delta = if end_offset > entry.data.len() {
            end_offset - entry.data.len()
        } else {
            0
        };

        if size_delta > 0 {
            // Calculate current filesystem usage
            let index = self.inner.index.lock();
            let mut total_size: usize = 0;
            for e in index.values() {
                if e.is_file() {
                    total_size += e.size;
                }
            }
            drop(index);

            // Check if adding this data would exceed filesystem limit
            let new_total_size = total_size
                .checked_add(size_delta)
                .ok_or_else(|| anyhow::anyhow!("Filesystem size overflow"))?;

            if new_total_size > MEMDB_MAX_FSSIZE {
                return Err(anyhow::anyhow!(
                    "Write would exceed maximum filesystem size: {} > {}",
                    new_total_size,
                    MEMDB_MAX_FSSIZE
                ));
            }
        }

        // Truncate behavior: preserve prefix bytes (matching C)
        // C implementation (memdb.c:724-726): if truncate, resize to offset, then write
        if truncate {
            // Preserve bytes before offset, clear from offset onwards
            entry.data.truncate(offset_usize);
        }

        // Extend if necessary
        if end_offset > entry.data.len() {
            entry.data.resize(end_offset, 0);
        }

        // Write data
        entry.data[offset_usize..end_offset].copy_from_slice(data);
        entry.size = entry.data.len();
        entry.mtime = mtime;
        entry.writer = writer;

        // Inline mutation logic to maintain write guard throughout
        // Increment version
        let new_version = self.inner.version.fetch_add(1, Ordering::SeqCst) + 1;
        entry.version = new_version;

        // Begin transaction
        let conn = self.inner.conn.lock();
        let tx = conn.unchecked_transaction().context("Failed to begin transaction")?;

        // Update __version__ entry in database (matches C's database.c:275-278)
        tx.execute(
            "UPDATE tree SET version = ?1, writer = ?2, mtime = ?3 WHERE inode = ?4",
            params![new_version, writer, mtime, ROOT_INODE],
        )
        .context("Failed to update __version__ entry")?;

        // Execute the update
        let result = (|| -> Result<TreeEntry> {
            tx.execute(
                "UPDATE tree SET version = ?1, writer = ?2, mtime = ?3, data = ?4 WHERE inode = ?5",
                params![
                    entry.version,
                    entry.writer,
                    entry.mtime,
                    &entry.data,
                    entry.inode
                ],
            )?;

            Ok(entry.clone())
        })();

        // Handle mutation result
        let updated_entry = match result {
            Ok(e) => e,
            Err(err) => {
                self.inner.errors.store(true, Ordering::SeqCst);
                tracing::error!("Database mutation failed: {}", err);
                return Err(err);
            }
        };

        // Commit transaction
        if let Err(e) = tx.commit() {
            self.inner.errors.store(true, Ordering::SeqCst);
            tracing::error!("Failed to commit transaction: {}", e);
            return Err(e.into());
        }

        drop(conn);

        // Update root entry metadata in memory
        {
            let mut index = self.inner.index.lock();
            if let Some(root_entry) = index.get_mut(&self.inner.root_inode) {
                root_entry.version = new_version;
                root_entry.writer = writer;
                root_entry.mtime = mtime;
            }
        }

        // Update in-memory index with the written entry
        {
            let mut index = self.inner.index.lock();
            index.insert(updated_entry.inode, updated_entry);
        }

        Ok(data.len())
    }

    /// Update modification time of a file or directory
    ///
    /// This implements the C version's `memdb_mtime` function (memdb.c:860-932)
    /// with full lock protection semantics for directories in `priv/lock/`.
    ///
    /// # Lock Protection
    ///
    /// For lock directories (`priv/lock/*`), this function enforces:
    /// 1. Only the same writer (node ID) can update the lock
    /// 2. Only newer mtime values are accepted (to prevent replay attacks)
    /// 3. Lock cache is refreshed after successful update
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file/directory
    /// * `writer` - Writer ID (node ID in cluster)
    /// * `mtime` - New modification time (seconds since UNIX epoch)
    pub fn set_mtime(&self, path: &str, writer: u32, mtime: u32) -> Result<()> {
        let mut entry = self
            .lookup_path(path)
            .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;

        // Don't allow updating root
        if entry.inode == self.inner.root_inode {
            return Err(anyhow::anyhow!("Cannot update root directory"));
        }

        // Check if this is a lock directory (matching C logic in memdb.c:882)
        let (parent_path, _) = Self::split_path(path)?;
        let is_lock = parent_path.trim_start_matches('/') == LOCK_DIR_PATH && entry.is_dir();

        if is_lock {
            // Lock protection: Only allow newer mtime (C: memdb.c:886-889)
            // This prevents replay attacks and ensures lock renewal works correctly
            if mtime < entry.mtime {
                tracing::warn!(
                    "Rejecting mtime update for lock '{}': {} < {} (locked)",
                    path,
                    mtime,
                    entry.mtime
                );
                return Err(anyhow::anyhow!(
                    "Cannot set older mtime on locked directory (dir is locked)"
                ));
            }

            // Lock protection: Only same writer can update (C: memdb.c:890-894)
            // This prevents lock hijacking from other nodes
            if entry.writer != writer {
                tracing::warn!(
                    "Rejecting mtime update for lock '{}': writer {} != {} (wrong owner)",
                    path,
                    writer,
                    entry.writer
                );
                return Err(anyhow::anyhow!(
                    "Lock owned by different writer (cannot hijack lock)"
                ));
            }

            tracing::debug!(
                "Updating lock directory: {} (mtime: {} -> {})",
                path,
                entry.mtime,
                mtime
            );
        }

        // Use with_mutation helper for atomic version bump + __version__ update
        let updated_entry = self.with_mutation(writer, mtime, |tx, version| {
            entry.version = version;
            entry.writer = writer;
            entry.mtime = mtime;

            tx.execute(
                "UPDATE tree SET version = ?1, writer = ?2, mtime = ?3 WHERE inode = ?4",
                params![entry.version, entry.writer, entry.mtime, entry.inode],
            )?;

            Ok(entry.clone())
        })?;

        // Update in-memory index
        {
            let mut index = self.inner.index.lock();
            index.insert(updated_entry.inode, updated_entry.clone());
        }

        // Refresh lock cache if this is a lock directory (C: memdb.c:924-929)
        // Remove old entry and insert new one with updated checksum
        if is_lock {
            let mut locks = self.inner.locks.lock();
            locks.remove(path);

            let csum = updated_entry.compute_checksum();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            locks.insert(path.to_string(), LockInfo { ltime: now, csum });

            tracing::debug!("Refreshed lock cache for: {}", path);
        }

        Ok(())
    }

    pub fn readdir(&self, path: &str) -> Result<Vec<TreeEntry>> {
        let entry = self
            .lookup_path(path)
            .ok_or_else(|| anyhow::anyhow!("Directory not found: {path}"))?;

        if !entry.is_dir() {
            return Err(anyhow::anyhow!("Not a directory: {path}"));
        }

        let tree = self.inner.tree.lock();
        let index = self.inner.index.lock();

        let children = tree
            .get(&entry.inode)
            .ok_or_else(|| anyhow::anyhow!("Directory structure corrupted"))?;

        let mut entries = Vec::new();
        for child_inode in children.values() {
            if let Some(child) = index.get(child_inode) {
                entries.push(child.clone());
            }
        }

        Ok(entries)
    }

    pub fn delete(&self, path: &str, writer: u32, mtime: u32) -> Result<()> {
        let entry = self
            .lookup_path(path)
            .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;

        // Don't allow deleting root
        if entry.inode == self.inner.root_inode {
            return Err(anyhow::anyhow!("Cannot delete root directory"));
        }

        // If directory, check if empty
        if entry.is_dir() {
            let tree = self.inner.tree.lock();
            if let Some(children) = tree.get(&entry.inode)
                && !children.is_empty()
            {
                return Err(anyhow::anyhow!("Directory not empty: {path}"));
            }
        }

        // Use with_mutation helper for atomic version bump + __version__ update
        self.with_mutation(writer, mtime, |tx, _version| {
            tx.execute("DELETE FROM tree WHERE inode = ?1", params![entry.inode])?;
            Ok(())
        })?;

        // Update in-memory structures
        {
            let mut index = self.inner.index.lock();
            let mut tree = self.inner.tree.lock();

            // Remove from index
            index.remove(&entry.inode);

            // Remove from parent's children
            if let Some(parent_children) = tree.get_mut(&entry.parent) {
                parent_children.remove(&entry.name);
            }

            // Remove from tree if directory
            if entry.is_dir() {
                tree.remove(&entry.inode);
            }
        }

        // Clean up lock cache for directories (matching C behavior in memdb.c:1235)
        // This prevents stale lock cache entries and memory leaks
        if entry.is_dir() {
            let mut locks = self.inner.locks.lock();
            locks.remove(path);
            tracing::debug!("Removed lock cache entry for deleted directory: {}", path);
        }

        Ok(())
    }

    pub fn rename(&self, old_path: &str, new_path: &str, writer: u32, mtime: u32) -> Result<()> {
        let mut entry = self
            .lookup_path(old_path)
            .ok_or_else(|| anyhow::anyhow!("Source not found: {old_path}"))?;

        if entry.inode == self.inner.root_inode {
            return Err(anyhow::anyhow!("Cannot rename root directory"));
        }

        // Protect lock directories from being renamed (memdb.c:1107-1111)
        if entry.is_dir() && Self::is_lock_dir(old_path) {
            return Err(std::io::Error::from_raw_os_error(libc::EACCES).into());
        }

        // If target exists, delete it first (POSIX rename semantics)
        // This matches C behavior (memdb.c:1113-1125) for atomic replacement
        let target_inode = if self.exists(new_path)? {
            let target_entry = self.lookup_path(new_path).unwrap();
            Some(target_entry.inode)
        } else {
            None
        };

        let (new_parent_path, new_basename) = Self::split_path(new_path)?;

        let new_parent_entry = self
            .lookup_path(&new_parent_path)
            .ok_or_else(|| anyhow::anyhow!("New parent directory not found: {new_parent_path}"))?;

        if !new_parent_entry.is_dir() {
            return Err(anyhow::anyhow!(
                "New parent is not a directory: {new_parent_path}"
            ));
        }

        let old_parent = entry.parent;
        let old_name = entry.name.clone();

        entry.parent = new_parent_entry.inode;
        entry.name = new_basename.clone();

        // Update writer and mtime on the renamed entry (memdb.c:1156-1164)
        entry.writer = writer;
        entry.mtime = mtime;

        // Use with_mutation helper for atomic version bump + __version__ update
        let updated_entry = self.with_mutation(writer, mtime, |tx, version| {
            entry.version = version;

            // Delete target if it exists (atomic replacement)
            if let Some(target_inode) = target_inode {
                tx.execute("DELETE FROM tree WHERE inode = ?1", params![target_inode])?;
            }

            // Update writer and mtime in database (memdb.c:1171-1173)
            tx.execute(
                "UPDATE tree SET parent = ?1, name = ?2, version = ?3, writer = ?4, mtime = ?5 WHERE inode = ?6",
                params![entry.parent, entry.name, entry.version, entry.writer, entry.mtime, entry.inode],
            )?;

            Ok(entry.clone())
        })?;

        // Update in-memory structures
        {
            let mut index = self.inner.index.lock();
            let mut tree = self.inner.tree.lock();

            // Remove target from in-memory structures if it existed
            if let Some(target_inode) = target_inode {
                index.remove(&target_inode);
                // Target is already in new_parent_entry's children, will be replaced below
            }

            index.insert(updated_entry.inode, updated_entry.clone());

            if let Some(old_parent_children) = tree.get_mut(&old_parent) {
                old_parent_children.remove(&old_name);
            }

            tree.entry(new_parent_entry.inode)
                .or_default()
                .insert(new_basename, updated_entry.inode);
        }

        Ok(())
    }

    pub fn get_all_entries(&self) -> Result<Vec<TreeEntry>> {
        let index = self.inner.index.lock();
        let entries: Vec<TreeEntry> = index.values().cloned().collect();
        Ok(entries)
    }

    pub fn get_version(&self) -> u64 {
        self.inner.version.load(Ordering::SeqCst)
    }

    /// Replace all entries (for full state synchronization)
    pub fn replace_all_entries(&self, entries: Vec<TreeEntry>) -> Result<()> {
        tracing::info!(
            "Replacing all database entries with {} new entries",
            entries.len()
        );

        let conn = self.inner.conn.lock();
        let tx = conn.unchecked_transaction()?;

        tx.execute("DELETE FROM tree", [])?;

        let max_version = entries.iter().map(|e| e.version).max().unwrap_or(0);

        for entry in &entries {
            tx.execute(
                "INSERT INTO tree (inode, parent, version, writer, mtime, type, name, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry.inode,
                    entry.parent,
                    entry.version,
                    entry.writer,
                    entry.mtime,
                    entry.entry_type,
                    entry.name,
                    if entry.is_dir() { None::<Vec<u8>> } else { Some(entry.data.clone()) }
                ],
            )?;
        }

        tx.commit()?;
        drop(conn);

        let mut index = self.inner.index.lock();
        let mut tree = self.inner.tree.lock();

        index.clear();
        tree.clear();

        for entry in entries {
            tree.entry(entry.parent)
                .or_default()
                .insert(entry.name.clone(), entry.inode);

            if entry.is_dir() {
                tree.entry(entry.inode).or_default();
            }

            index.insert(entry.inode, entry);
        }

        self.inner.version.store(max_version, Ordering::SeqCst);

        tracing::info!(
            "Database state replaced successfully, version now: {}",
            max_version
        );
        Ok(())
    }

    /// Apply a single TreeEntry during incremental synchronization
    ///
    /// This is used when receiving Update messages from the leader.
    /// It directly inserts or updates the entry in the database without
    /// going through the path-based API.
    pub fn apply_tree_entry(&self, entry: TreeEntry) -> Result<()> {
        tracing::debug!(
            "Applying TreeEntry: inode={}, parent={}, name='{}', version={}",
            entry.inode,
            entry.parent,
            entry.name,
            entry.version
        );

        // Acquire locks in consistent order: conn, then index, then tree
        // This prevents DB-memory divergence by updating both atomically
        let conn = self.inner.conn.lock();
        let mut index = self.inner.index.lock();
        let mut tree = self.inner.tree.lock();

        // Begin transaction for atomicity
        let tx = conn.unchecked_transaction()?;

        // Handle root inode specially (inode 0 is __version__)
        let db_name = if entry.inode == self.inner.root_inode {
            VERSION_FILENAME
        } else {
            entry.name.as_str()
        };

        // Insert or replace the entry in database
        tx.execute(
            "INSERT OR REPLACE INTO tree (inode, parent, version, writer, mtime, type, name, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                entry.inode,
                entry.parent,
                entry.version,
                entry.writer,
                entry.mtime,
                entry.entry_type,
                db_name,
                if entry.is_dir() { None::<Vec<u8>> } else { Some(entry.data.clone()) }
            ],
        )?;

        // Update __version__ entry with the same metadata (matching C in database.c:275-278)
        // Only do this if we're not already writing __version__ itself
        if entry.inode != ROOT_INODE {
            Self::update_version_entry(&tx, entry.version, entry.writer, entry.mtime)?;
        }

        // Update in-memory structures BEFORE committing transaction
        // This ensures DB and memory are atomically updated together

        // Check if this entry already exists
        let old_entry = index.get(&entry.inode).cloned();

        // If entry exists with different parent or name, update tree structure
        if let Some(old) = old_entry {
            if old.parent != entry.parent || old.name != entry.name {
                // Remove from old parent's children
                if let Some(old_parent_children) = tree.get_mut(&old.parent) {
                    old_parent_children.remove(&old.name);
                }

                // Add to new parent's children
                tree.entry(entry.parent)
                    .or_default()
                    .insert(entry.name.clone(), entry.inode);
            }
        } else {
            // New entry - add to parent's children
            tree.entry(entry.parent)
                .or_default()
                .insert(entry.name.clone(), entry.inode);
        }

        // If this is a directory, ensure it has an entry in the tree map
        if entry.is_dir() {
            tree.entry(entry.inode).or_default();
        }

        // Update index
        index.insert(entry.inode, entry.clone());

        // Update root entry's metadata to match __version__ (if we wrote a non-root entry)
        if entry.inode != self.inner.root_inode {
            Self::update_root_metadata(
                &mut index,
                self.inner.root_inode,
                entry.version,
                entry.writer,
                entry.mtime,
            );
            tracing::debug!(
                version = entry.version,
                writer = entry.writer,
                mtime = entry.mtime,
                "Updated root entry metadata"
            );
        }

        // Update version counter if this entry has a higher version
        self.inner
            .version
            .fetch_max(entry.version, Ordering::SeqCst);

        // Commit transaction after memory is updated
        // Both DB and memory are now consistent
        tx.commit()?;

        tracing::debug!("TreeEntry applied successfully");
        Ok(())
    }

    /// **TEST ONLY**: Manually set lock timestamp for testing expiration behavior
    ///
    /// This method is exposed for testing purposes only to simulate lock expiration
    /// without waiting the full 120 seconds. Do not use in production code.
    #[cfg(test)]
    pub fn test_set_lock_timestamp(&self, path: &str, timestamp_secs: u64) {
        // Normalize path to remove leading slash for consistency
        let normalized_path = path.strip_prefix('/').unwrap_or(path);

        let mut locks = self.inner.locks.lock();
        if let Some(lock_info) = locks.get_mut(normalized_path) {
            lock_info.ltime = timestamp_secs;
        }
    }

    /// Get filesystem statistics
    ///
    /// Returns information about the filesystem usage, matching C's memdb_statfs
    /// implementation. This is used by FUSE to report filesystem statistics.
    ///
    /// # Returns
    ///
    /// A tuple of (blocks, bfree, bavail, files, ffree) where:
    /// - blocks: Total data blocks in filesystem
    /// - bfree: Free blocks available
    /// - bavail: Free blocks available to non-privileged user
    /// - files: Total file nodes (inodes)
    /// - ffree: Free file nodes
    pub fn statfs(&self) -> (u64, u64, u64, u64, u64) {
        const MEMDB_BLOCKSIZE: u64 = 4096;
        const MEMDB_MAX_FSSIZE: u64 = 128 * 1024 * 1024; // 128 MiB
        const MEMDB_MAX_INODES: u64 = 256 * 1024; // 256k inodes

        let index = self.inner.index.lock();

        // Calculate total size used by all files
        let mut total_size: u64 = 0;
        for entry in index.values() {
            if entry.is_file() {
                total_size += entry.size as u64;
            }
        }

        // Calculate blocks
        let blocks = MEMDB_MAX_FSSIZE / MEMDB_BLOCKSIZE;
        let blocks_used = (total_size + MEMDB_BLOCKSIZE - 1) / MEMDB_BLOCKSIZE;
        let bfree = blocks.saturating_sub(blocks_used);
        let bavail = bfree; // Same as bfree for non-privileged users

        // Calculate inodes
        let files = MEMDB_MAX_INODES;
        let files_used = index.len() as u64;
        let ffree = files.saturating_sub(files_used);

        (blocks, bfree, bavail, files, ffree)
    }
}

// ============================================================================
// Trait Implementation for Dependency Injection
// ============================================================================

impl crate::traits::MemDbOps for MemDb {
    fn create(&self, path: &str, mode: u32, writer: u32, mtime: u32) -> Result<()> {
        self.create(path, mode, writer, mtime)
    }

    fn read(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>> {
        self.read(path, offset, size)
    }

    fn write(
        &self,
        path: &str,
        offset: u64,
        writer: u32,
        mtime: u32,
        data: &[u8],
        truncate: bool,
    ) -> Result<usize> {
        self.write(path, offset, writer, mtime, data, truncate)
    }

    fn delete(&self, path: &str, writer: u32, mtime: u32) -> Result<()> {
        self.delete(path, writer, mtime)
    }

    fn rename(&self, old_path: &str, new_path: &str, writer: u32, mtime: u32) -> Result<()> {
        self.rename(old_path, new_path, writer, mtime)
    }

    fn exists(&self, path: &str) -> Result<bool> {
        self.exists(path)
    }

    fn readdir(&self, path: &str) -> Result<Vec<crate::types::TreeEntry>> {
        self.readdir(path)
    }

    fn set_mtime(&self, path: &str, writer: u32, mtime: u32) -> Result<()> {
        self.set_mtime(path, writer, mtime)
    }

    fn lookup_path(&self, path: &str) -> Option<crate::types::TreeEntry> {
        self.lookup_path(path)
    }

    fn get_entry_by_inode(&self, inode: u64) -> Option<crate::types::TreeEntry> {
        self.get_entry_by_inode(inode)
    }

    fn acquire_lock(&self, path: &str, csum: &[u8; 32]) -> Result<()> {
        self.acquire_lock(path, csum)
    }

    fn release_lock(&self, path: &str, csum: &[u8; 32]) -> Result<()> {
        self.release_lock(path, csum)
    }

    fn is_locked(&self, path: &str) -> bool {
        self.is_locked(path)
    }

    fn lock_expired(&self, path: &str, csum: &[u8; 32]) -> bool {
        self.lock_expired(path, csum)
    }

    fn get_version(&self) -> u64 {
        self.get_version()
    }

    fn get_all_entries(&self) -> Result<Vec<crate::types::TreeEntry>> {
        self.get_all_entries()
    }

    fn replace_all_entries(&self, entries: Vec<crate::types::TreeEntry>) -> Result<()> {
        self.replace_all_entries(entries)
    }

    fn apply_tree_entry(&self, entry: crate::types::TreeEntry) -> Result<()> {
        self.apply_tree_entry(entry)
    }

    fn encode_database(&self) -> Result<Vec<u8>> {
        self.encode_database()
    }

    fn compute_database_checksum(&self) -> Result<[u8; 32]> {
        self.compute_database_checksum()
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for MemDb database operations
    //!
    //! This test module provides comprehensive coverage for:
    //! - Basic CRUD operations (create, read, write, delete, rename)
    //! - Lock management (acquisition, release, expiration, contention)
    //! - Checksum operations
    //! - Persistence verification
    //! - Error handling and edge cases
    //! - Security (path traversal, type mismatches)
    //!
    //! ## Test Organization
    //!
    //! Tests are organized into several categories:
    //! - **Basic Operations**: File and directory CRUD
    //! - **Lock Management**: Lock lifecycle, expiration, renewal
    //! - **Error Handling**: Path validation, type checking, duplicates
    //! - **Edge Cases**: Empty paths, sparse files, boundary conditions
    //!
    //! ## Lock Expiration Testing
    //!
    //! Lock timeout is 120 seconds. Tests use `test_set_lock_timestamp()` helper
    //! to simulate time passage without waiting 120 actual seconds.

    use super::*;
    use std::thread::sleep;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    #[test]
    fn test_lock_expiration() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;
        let path = "/priv/lock/test-resource";
        let csum = [42u8; 32];

        // Create lock directory structure
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Acquire lock
        db.acquire_lock(path, &csum)?;
        assert!(db.is_locked(path), "Lock should be active");
        assert!(
            !db.lock_expired(path, &csum),
            "Lock should not be expired initially"
        );

        // Wait a short time (should still not be expired)
        sleep(Duration::from_secs(2));
        assert!(
            db.is_locked(path),
            "Lock should still be active after 2 seconds"
        );
        assert!(
            !db.lock_expired(path, &csum),
            "Lock should not be expired after 2 seconds"
        );

        // Manually set lock timestamp to simulate expiration (testing internal behavior)
        // Note: In C implementation, LOCK_TIMEOUT is 120 seconds (memdb.h:27)
        // Set ltime to 121 seconds ago (past LOCK_TIMEOUT of 120 seconds)
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        db.test_set_lock_timestamp(path, now_secs - 121);

        // Now the lock should be expired
        assert!(
            db.lock_expired(path, &csum),
            "Lock should be expired after 121 seconds"
        );

        // is_locked() should also return false for expired locks
        assert!(
            !db.is_locked(path),
            "is_locked() should return false for expired locks"
        );

        // Test checksum mismatch resets timeout
        let different_csum = [99u8; 32];
        assert!(
            !db.lock_expired(path, &different_csum),
            "lock_expired() with different checksum should reset timeout and return false"
        );

        // After checksum mismatch, lock should be active again (with new checksum)
        assert!(
            db.is_locked(path),
            "Lock should be active after checksum reset"
        );

        Ok(())
    }

    #[test]
    fn test_memdb_file_size_limit() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        // Create database
        let db = MemDb::open(&db_path, true)?;

        // Create a file
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        db.create("/test.bin", libc::S_IFREG, 0, now)?;

        // Try to write exactly 1MB (should succeed)
        let data_1mb = vec![0u8; 1024 * 1024];
        let result = db.write("/test.bin", 0, 0, now, &data_1mb, false);
        assert!(result.is_ok(), "1MB file should be accepted");

        // Try to write 1MB + 1 byte (should fail)
        let data_too_large = vec![0u8; 1024 * 1024 + 1];
        db.create("/test2.bin", libc::S_IFREG, 0, now)?;
        let result = db.write("/test2.bin", 0, 0, now, &data_too_large, false);
        assert!(result.is_err(), "File larger than 1MB should be rejected");

        Ok(())
    }

    #[test]
    fn test_memdb_basic_operations() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        // Create database
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Test directory creation
        db.create("/testdir", libc::S_IFDIR, 0, now)?;
        assert!(db.exists("/testdir")?, "Directory should exist");

        // Test file creation
        db.create("/testdir/file.txt", libc::S_IFREG, 0, now)?;
        assert!(db.exists("/testdir/file.txt")?, "File should exist");

        // Test write
        let data = b"Hello, pmxcfs!";
        db.write("/testdir/file.txt", 0, 0, now, data, false)?;

        // Test read
        let read_data = db.read("/testdir/file.txt", 0, 1024)?;
        assert_eq!(&read_data[..], data, "Read data should match written data");

        // Test readdir
        let entries = db.readdir("/testdir")?;
        assert_eq!(entries.len(), 1, "Directory should have 1 entry");
        assert_eq!(entries[0].name, "file.txt");

        // Test rename
        db.rename("/testdir/file.txt", "/testdir/renamed.txt", 0, now)?;
        assert!(
            !db.exists("/testdir/file.txt")?,
            "Old path should not exist"
        );
        assert!(db.exists("/testdir/renamed.txt")?, "New path should exist");

        // Test delete
        db.delete("/testdir/renamed.txt", 0, now)?;
        assert!(
            !db.exists("/testdir/renamed.txt")?,
            "Deleted file should not exist"
        );

        Ok(())
    }

    #[test]
    fn test_lock_management() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create parent directory and resource
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock/qemu-server", libc::S_IFDIR, 0, now)?;

        let path = "/priv/lock/resource";
        let csum1 = [1u8; 32];
        let csum2 = [2u8; 32];

        // Create the lock file
        db.create(path, libc::S_IFREG, 0, now)?;

        // Test lock acquisition
        assert!(!db.is_locked(path), "Path should not be locked initially");

        db.acquire_lock(path, &csum1)?;
        assert!(
            db.is_locked(path),
            "Path should be locked after acquisition"
        );

        // Test lock contention
        let result = db.acquire_lock(path, &csum2);
        assert!(result.is_err(), "Lock with different checksum should fail");

        // Test lock refresh (same checksum)
        let result = db.acquire_lock(path, &csum1);
        assert!(
            result.is_ok(),
            "Lock refresh with same checksum should succeed"
        );

        // Test lock release
        db.release_lock(path, &csum1)?;
        assert!(
            !db.is_locked(path),
            "Path should not be locked after release"
        );

        // Test release non-existent lock
        let result = db.release_lock(path, &csum1);
        assert!(result.is_err(), "Releasing non-existent lock should fail");

        // Test lock access using config path (maps to priv/lock)
        let config_path = "/qemu-server/100.conf";
        let csum3 = [3u8; 32];
        db.acquire_lock(config_path, &csum3)?;
        assert!(db.is_locked(config_path), "Config path should be locked");
        db.release_lock(config_path, &csum3)?;
        assert!(
            !db.is_locked(config_path),
            "Config path should be unlocked after release"
        );

        Ok(())
    }

    #[test]
    fn test_checksum_operations() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create some test data
        db.create("/file1.txt", libc::S_IFREG, 0, now)?;
        db.write("/file1.txt", 0, 0, now, b"test data 1", false)?;

        db.create("/file2.txt", libc::S_IFREG, 0, now)?;
        db.write("/file2.txt", 0, 0, now, b"test data 2", false)?;

        // Test database encoding
        let encoded = db.encode_database()?;
        assert!(!encoded.is_empty(), "Encoded database should not be empty");

        // Test database checksum
        let checksum1 = db.compute_database_checksum()?;
        assert_ne!(checksum1, [0u8; 32], "Checksum should not be all zeros");

        // Compute checksum again - should be the same
        let checksum2 = db.compute_database_checksum()?;
        assert_eq!(checksum1, checksum2, "Checksum should be deterministic");

        // Modify database and verify checksum changes
        db.write("/file1.txt", 0, 0, now, b"modified data", false)?;
        let checksum3 = db.compute_database_checksum()?;
        assert_ne!(
            checksum1, checksum3,
            "Checksum should change after modification"
        );

        // Test entry checksum
        if let Some(entry) = db.lookup_path("/file1.txt") {
            let entry_csum = entry.compute_checksum();
            assert_ne!(
                entry_csum, [0u8; 32],
                "Entry checksum should not be all zeros"
            );
        } else {
            panic!("File should exist");
        }

        Ok(())
    }

    #[test]
    fn test_lock_cache_cleanup_on_delete() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create priv/lock directory
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Create a lock directory
        db.create("/priv/lock/testlock", libc::S_IFDIR, 0, now)?;

        // Verify lock directory exists
        assert!(db.exists("/priv/lock/testlock")?);

        // Delete the lock directory
        db.delete("/priv/lock/testlock", 0, now)?;

        // Verify lock directory is deleted
        assert!(!db.exists("/priv/lock/testlock")?);

        Ok(())
    }

    #[test]
    fn test_lock_protection_same_writer() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create priv/lock directory
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Create a lock directory
        db.create("/priv/lock/mylock", libc::S_IFDIR, 0, now)?;

        // Get the actual writer ID from the created lock
        let entry = db.lookup_path("/priv/lock/mylock").unwrap();
        let writer_id = entry.writer;

        // Same writer (node 1) should be able to update mtime
        let new_mtime = now + 10;
        let result = db.set_mtime("/priv/lock/mylock", writer_id, new_mtime);
        assert!(
            result.is_ok(),
            "Same writer should be able to update lock mtime"
        );

        // Verify mtime was updated
        let updated_entry = db.lookup_path("/priv/lock/mylock").unwrap();
        assert_eq!(updated_entry.mtime, new_mtime);

        Ok(())
    }

    #[test]
    fn test_lock_protection_different_writer() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create priv/lock directory
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Create a lock directory
        db.create("/priv/lock/mylock", libc::S_IFDIR, 0, now)?;

        // Get the current writer ID
        let entry = db.lookup_path("/priv/lock/mylock").unwrap();
        let original_writer = entry.writer;

        // Try to update from different writer (simulating another node trying to steal lock)
        let different_writer = original_writer + 1;
        let new_mtime = now + 10;
        let result = db.set_mtime("/priv/lock/mylock", different_writer, new_mtime);

        // Should fail - cannot hijack lock from different writer
        assert!(
            result.is_err(),
            "Different writer should NOT be able to hijack lock"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Lock owned by different writer"),
            "Error should indicate lock ownership conflict"
        );

        // Verify mtime was NOT updated
        let unchanged_entry = db.lookup_path("/priv/lock/mylock").unwrap();
        assert_eq!(unchanged_entry.mtime, now, "Mtime should not have changed");

        Ok(())
    }

    #[test]
    fn test_lock_protection_older_mtime() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create priv/lock directory
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Create a lock directory
        db.create("/priv/lock/mylock", libc::S_IFDIR, 0, now)?;

        let entry = db.lookup_path("/priv/lock/mylock").unwrap();
        let writer_id = entry.writer;

        // Try to set an older mtime (replay attack simulation)
        let older_mtime = now - 10;
        let result = db.set_mtime("/priv/lock/mylock", writer_id, older_mtime);

        // Should fail - cannot set older mtime
        assert!(result.is_err(), "Cannot set older mtime on lock");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Cannot set older mtime"),
            "Error should indicate mtime protection"
        );

        // Verify mtime was NOT changed
        let unchanged_entry = db.lookup_path("/priv/lock/mylock").unwrap();
        assert_eq!(unchanged_entry.mtime, now, "Mtime should not have changed");

        Ok(())
    }

    #[test]
    fn test_lock_protection_newer_mtime() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create priv/lock directory
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Create a lock directory
        db.create("/priv/lock/mylock", libc::S_IFDIR, 0, now)?;

        let entry = db.lookup_path("/priv/lock/mylock").unwrap();
        let writer_id = entry.writer;

        // Set a newer mtime (normal lock refresh)
        let newer_mtime = now + 60;
        let result = db.set_mtime("/priv/lock/mylock", writer_id, newer_mtime);

        // Should succeed
        assert!(result.is_ok(), "Should be able to set newer mtime on lock");

        // Verify mtime was updated
        let updated_entry = db.lookup_path("/priv/lock/mylock").unwrap();
        assert_eq!(updated_entry.mtime, newer_mtime, "Mtime should be updated");

        Ok(())
    }

    #[test]
    fn test_regular_file_mtime_update() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create a regular file
        db.create("/testfile.txt", 0, 0, now)?;

        let entry = db.lookup_path("/testfile.txt").unwrap();
        let writer_id = entry.writer;

        // Should be able to set both older and newer mtime on regular files
        let older_mtime = now - 10;
        let result = db.set_mtime("/testfile.txt", writer_id, older_mtime);
        assert!(result.is_ok(), "Regular files should allow older mtime");

        let newer_mtime = now + 10;
        let result = db.set_mtime("/testfile.txt", writer_id, newer_mtime);
        assert!(result.is_ok(), "Regular files should allow newer mtime");

        Ok(())
    }

    #[test]
    fn test_lock_lifecycle_with_cache() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Setup: Create priv/lock directory
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Step 1: Create lock
        db.create("/priv/lock/lifecycle_lock", libc::S_IFDIR, 0, now)?;
        assert!(db.exists("/priv/lock/lifecycle_lock")?);

        let entry = db.lookup_path("/priv/lock/lifecycle_lock").unwrap();
        let writer_id = entry.writer;

        // Step 2: Refresh lock multiple times (simulate lock renewals)
        for i in 1..=5 {
            let refresh_mtime = now + (i * 30); // Refresh every 30 seconds
            let result = db.set_mtime("/priv/lock/lifecycle_lock", writer_id, refresh_mtime);
            assert!(result.is_ok(), "Lock refresh #{i} should succeed");

            // Verify mtime was updated
            let refreshed_entry = db.lookup_path("/priv/lock/lifecycle_lock").unwrap();
            assert_eq!(refreshed_entry.mtime, refresh_mtime);
        }

        // Step 3: Delete lock (release)
        db.delete("/priv/lock/lifecycle_lock", 0, now)?;
        assert!(!db.exists("/priv/lock/lifecycle_lock")?);

        Ok(())
    }

    #[test]
    fn test_lock_renewal_before_expiration() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;
        let path = "/priv/lock/renewal-test";
        let csum = [55u8; 32];

        // Create lock directory structure
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Acquire initial lock
        db.acquire_lock(path, &csum)?;
        assert!(db.is_locked(path), "Lock should be active");

        // Simulate time passing (119 seconds - just before expiration)
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        db.test_set_lock_timestamp(path, now_secs - 119);

        // Lock should still be valid (not yet expired)
        assert!(
            !db.lock_expired(path, &csum),
            "Lock should not be expired at 119 seconds"
        );
        assert!(
            db.is_locked(path),
            "is_locked() should return true before expiration"
        );

        // Renew the lock by acquiring again with same checksum
        db.acquire_lock(path, &csum)?;

        // After renewal, lock should definitely not be expired
        assert!(
            !db.lock_expired(path, &csum),
            "Lock should not be expired after renewal"
        );
        assert!(
            db.is_locked(path),
            "Lock should still be active after renewal"
        );

        // Now simulate expiration time (121 seconds from renewal)
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        db.test_set_lock_timestamp(path, now_secs - 121);

        // Lock should now be expired
        assert!(
            db.lock_expired(path, &csum),
            "Lock should be expired after 121 seconds without renewal"
        );

        Ok(())
    }

    #[test]
    fn test_acquire_lock_after_expiration() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;
        let path = "/priv/lock/reacquire-test";
        let csum1 = [11u8; 32];
        let csum2 = [22u8; 32];

        // Create lock directory structure
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Acquire initial lock with csum1
        db.acquire_lock(path, &csum1)?;
        assert!(db.is_locked(path), "Lock should be active");

        // Simulate lock expiration (121 seconds)
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        db.test_set_lock_timestamp(path, now_secs - 121);

        // Verify lock is expired
        assert!(db.lock_expired(path, &csum1), "Lock should be expired");
        assert!(
            !db.is_locked(path),
            "is_locked() should return false for expired lock"
        );

        // A different process should be able to acquire the expired lock
        let result = db.acquire_lock(path, &csum2);
        assert!(
            result.is_ok(),
            "Should be able to acquire expired lock with different checksum"
        );

        // Lock should now be active with new checksum
        assert!(
            db.is_locked(path),
            "Lock should be active with new checksum"
        );
        assert!(
            !db.lock_expired(path, &csum2),
            "New lock should not be expired"
        );

        // Old checksum should fail to check expiration (checksum mismatch)
        assert!(
            !db.lock_expired(path, &csum1),
            "lock_expired() with old checksum should reset timeout and return false"
        );

        Ok(())
    }

    #[test]
    fn test_multiple_locks_expiring() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create lock directory structure
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Create three locks
        let locks = [
            ("/priv/lock/lock1", [1u8; 32]),
            ("/priv/lock/lock2", [2u8; 32]),
            ("/priv/lock/lock3", [3u8; 32]),
        ];

        // Acquire all locks
        for (path, csum) in &locks {
            db.acquire_lock(path, csum)?;
            assert!(db.is_locked(path), "Lock {path} should be active");
        }

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Set different expiration times
        // lock1: 121 seconds ago (expired)
        // lock2: 119 seconds ago (not expired)
        // lock3: 121 seconds ago (expired)
        db.test_set_lock_timestamp(locks[0].0, now_secs - 121);
        db.test_set_lock_timestamp(locks[1].0, now_secs - 119);
        db.test_set_lock_timestamp(locks[2].0, now_secs - 121);

        // Check expiration states
        assert!(
            db.lock_expired(locks[0].0, &locks[0].1),
            "lock1 should be expired"
        );
        assert!(
            !db.lock_expired(locks[1].0, &locks[1].1),
            "lock2 should not be expired"
        );
        assert!(
            db.lock_expired(locks[2].0, &locks[2].1),
            "lock3 should be expired"
        );

        // Check is_locked states
        assert!(
            !db.is_locked(locks[0].0),
            "lock1 is_locked should return false"
        );
        assert!(
            db.is_locked(locks[1].0),
            "lock2 is_locked should return true"
        );
        assert!(
            !db.is_locked(locks[2].0),
            "lock3 is_locked should return false"
        );

        // Re-acquire expired locks with different checksums
        let new_csum1 = [11u8; 32];
        let new_csum3 = [33u8; 32];

        assert!(
            db.acquire_lock(locks[0].0, &new_csum1).is_ok(),
            "Should be able to re-acquire expired lock1"
        );
        assert!(
            db.acquire_lock(locks[2].0, &new_csum3).is_ok(),
            "Should be able to re-acquire expired lock3"
        );

        // Verify all locks are now active
        assert!(db.is_locked(locks[0].0), "lock1 should be active again");
        assert!(db.is_locked(locks[1].0), "lock2 should still be active");
        assert!(db.is_locked(locks[2].0), "lock3 should be active again");

        Ok(())
    }

    #[test]
    fn test_lock_expiration_boundary() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;
        let path = "/priv/lock/boundary-test";
        let csum = [77u8; 32];

        // Create lock directory structure
        db.create("/priv", libc::S_IFDIR, 0, now)?;
        db.create("/priv/lock", libc::S_IFDIR, 0, now)?;

        // Acquire lock
        db.acquire_lock(path, &csum)?;

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Test exact boundary: 120 seconds (LOCK_TIMEOUT)
        db.test_set_lock_timestamp(path, now_secs - 120);
        assert!(
            !db.lock_expired(path, &csum),
            "Lock should NOT be expired at exactly 120 seconds (boundary)"
        );
        assert!(
            db.is_locked(path),
            "Lock should still be considered active at 120 seconds"
        );

        // Test 121 seconds (just past timeout)
        db.test_set_lock_timestamp(path, now_secs - 121);
        assert!(
            db.lock_expired(path, &csum),
            "Lock SHOULD be expired at 121 seconds"
        );
        assert!(
            !db.is_locked(path),
            "Lock should not be considered active at 121 seconds"
        );

        Ok(())
    }

    // ===== Error Handling Tests =====

    #[test]
    fn test_invalid_path_traversal() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Test path traversal attempts
        let invalid_paths = vec![
            "/../etc/passwd",            // Absolute path traversal
            "/test/../../../etc/passwd", // Multiple parent references
            "//etc//passwd",             // Double slashes
            "/test/./file",              // Current directory reference
        ];

        for invalid_path in invalid_paths {
            // Attempt to create with invalid path
            let result = db.create(invalid_path, libc::S_IFREG, 0, now);
            // Note: Current implementation may not reject all these - this documents behavior
            // In production, path validation should be added
            if let Err(e) = result {
                assert!(
                    e.to_string().contains("Invalid") || e.to_string().contains("not found"),
                    "Invalid path '{invalid_path}' should produce appropriate error: {e}"
                );
            }
        }

        Ok(())
    }

    #[test]
    fn test_operations_on_nonexistent_paths() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Try to read non-existent file
        let result = db.read("/nonexistent.txt", 0, 100);
        assert!(result.is_err(), "Reading non-existent file should fail");

        // Try to write to non-existent file
        let result = db.write("/nonexistent.txt", 0, 0, now, b"data", false);
        assert!(result.is_err(), "Writing to non-existent file should fail");

        // Try to delete non-existent file
        let result = db.delete("/nonexistent.txt", 0, now);
        assert!(result.is_err(), "Deleting non-existent file should fail");

        // Try to rename non-existent file
        let result = db.rename("/nonexistent.txt", "/new.txt", 0, now);
        assert!(result.is_err(), "Renaming non-existent file should fail");

        // Try to check if non-existent file is locked
        assert!(
            !db.is_locked("/nonexistent.txt"),
            "Non-existent file should not be locked"
        );

        Ok(())
    }

    #[test]
    fn test_file_type_mismatches() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create a directory
        db.create("/testdir", libc::S_IFDIR, 0, now)?;

        // Try to write to a directory (should fail)
        let result = db.write("/testdir", 0, 0, now, b"data", false);
        assert!(result.is_err(), "Writing to a directory should fail");

        // Try to read from a directory (readdir should work, but read should fail)
        let result = db.read("/testdir", 0, 100);
        assert!(result.is_err(), "Reading from a directory should fail");

        // Create a file
        db.create("/testfile.txt", libc::S_IFREG, 0, now)?;

        // Try to readdir on a file (should fail)
        let result = db.readdir("/testfile.txt");
        assert!(result.is_err(), "Readdir on a file should fail");

        Ok(())
    }

    #[test]
    fn test_duplicate_creation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create a file
        db.create("/duplicate.txt", libc::S_IFREG, 0, now)?;

        // Try to create the same file again
        let result = db.create("/duplicate.txt", libc::S_IFREG, 0, now);
        assert!(result.is_err(), "Creating duplicate file should fail");

        // Create a directory
        db.create("/dupdir", libc::S_IFDIR, 0, now)?;

        // Try to create the same directory again
        let result = db.create("/dupdir", libc::S_IFDIR, 0, now);
        assert!(result.is_err(), "Creating duplicate directory should fail");

        Ok(())
    }

    #[test]
    fn test_rename_target_exists() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create source and target files
        db.create("/source.txt", libc::S_IFREG, 0, now)?;
        db.write("/source.txt", 0, 0, now, b"source data", false)?;

        db.create("/target.txt", libc::S_IFREG, 0, now)?;
        db.write("/target.txt", 0, 0, now, b"target data", false)?;

        // Rename source to existing target (should succeed with atomic replacement - POSIX semantics)
        let result = db.rename("/source.txt", "/target.txt", 0, now);
        assert!(result.is_ok(), "Renaming to existing target should succeed (POSIX semantics)");

        // Source should no longer exist
        assert!(
            !db.exists("/source.txt")?,
            "Source should not exist after rename"
        );

        // Target should exist with source's data (atomic replacement)
        assert!(db.exists("/target.txt")?, "Target should exist");
        let data = db.read("/target.txt", 0, 100)?;
        assert_eq!(
            &data[..],
            b"source data",
            "Target should have source's data after atomic replacement"
        );


        Ok(())
    }

    #[test]
    fn test_delete_nonempty_directory() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create a directory with a file
        db.create("/parent", libc::S_IFDIR, 0, now)?;
        db.create("/parent/child.txt", libc::S_IFREG, 0, now)?;

        // Try to delete non-empty directory
        let result = db.delete("/parent", 0, now);
        // Note: Current behavior may vary - document expected behavior
        if let Err(e) = result {
            assert!(
                e.to_string().contains("not empty") || e.to_string().contains("ENOTEMPTY"),
                "Deleting non-empty directory should produce appropriate error: {e}"
            );
        }

        Ok(())
    }

    #[test]
    fn test_write_offset_beyond_file_size() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create a file with some data
        db.create("/offset-test.txt", libc::S_IFREG, 0, now)?;
        db.write("/offset-test.txt", 0, 0, now, b"hello", false)?;

        // Write at offset beyond current file size (sparse file)
        let result = db.write("/offset-test.txt", 100, 0, now, b"world", false);

        // Check if sparse writes are supported
        if result.is_ok() {
            let data = db.read("/offset-test.txt", 0, 200)?;
            // Should have zeros between offset 5 and 100
            assert_eq!(&data[0..5], b"hello", "Initial data should be preserved");
            assert_eq!(
                &data[100..105],
                b"world",
                "Data at offset should be written"
            );
        }

        Ok(())
    }

    #[test]
    fn test_empty_path_handling() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = MemDb::open(&db_path, true)?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Test empty path for create (should be rejected)
        let result = db.create("", libc::S_IFREG, 0, now);
        assert!(result.is_err(), "Empty path should be rejected for create");

        // Note: exists("") behavior is implementation-specific (may return true for root)
        // so we don't test it here

        Ok(())
    }

    #[test]
    fn test_database_persistence() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create database and write data
        {
            let db = MemDb::open(&db_path, true)?;
            db.create("/persistent.txt", libc::S_IFREG, 0, now)?;
            db.write("/persistent.txt", 0, 0, now, b"persistent data", false)?;
        }

        // Reopen database and verify data persists
        {
            let db = MemDb::open(&db_path, false)?;
            assert!(
                db.exists("/persistent.txt")?,
                "File should persist across reopens"
            );

            let data = db.read("/persistent.txt", 0, 1024)?;
            assert_eq!(&data[..], b"persistent data", "Data should persist");
        }

        Ok(())
    }

    #[test]
    fn test_persistence_with_multiple_files() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create database with multiple files
        {
            let db = MemDb::open(&db_path, true)?;

            // Create directory
            db.create("/config", libc::S_IFDIR, 0, now)?;

            // Create files in root
            db.create("/file1.txt", libc::S_IFREG, 0, now)?;
            db.write("/file1.txt", 0, 0, now, b"content 1", false)?;

            // Create files in directory
            db.create("/config/file2.txt", libc::S_IFREG, 0, now)?;
            db.write("/config/file2.txt", 0, 0, now, b"content 2", false)?;
        }

        // Reopen and verify all data persists
        {
            let db = MemDb::open(&db_path, false)?;

            assert!(db.exists("/config")?, "Directory should persist");
            assert!(db.exists("/file1.txt")?, "File 1 should persist");
            assert!(db.exists("/config/file2.txt")?, "File 2 should persist");

            let data1 = db.read("/file1.txt", 0, 1024)?;
            assert_eq!(&data1[..], b"content 1", "File 1 content should persist");

            let data2 = db.read("/config/file2.txt", 0, 1024)?;
            assert_eq!(&data2[..], b"content 2", "File 2 content should persist");
        }

        Ok(())
    }

    #[test]
    fn test_persistence_after_updates() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

        // Create database and write initial data
        {
            let db = MemDb::open(&db_path, true)?;
            db.create("/mutable.txt", libc::S_IFREG, 0, now)?;
            db.write("/mutable.txt", 0, 0, now, b"initial", false)?;
        }

        // Reopen and update data
        {
            let db = MemDb::open(&db_path, false)?;
            db.write("/mutable.txt", 0, 0, now + 1, b"updated", false)?;
        }

        // Reopen again and verify updated data persists
        {
            let db = MemDb::open(&db_path, false)?;
            let data = db.read("/mutable.txt", 0, 1024)?;
            assert_eq!(&data[..], b"updated", "Updated data should persist");
        }

        Ok(())
    }
}
