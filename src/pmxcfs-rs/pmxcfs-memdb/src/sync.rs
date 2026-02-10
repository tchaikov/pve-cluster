/// State synchronization and serialization for memdb
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;

use super::database::MemDb;
use super::index::{IndexEntry, MemDbIndex};
use super::types::TreeEntry;

impl MemDb {
    /// Encode database index for C-compatible state synchronization
    ///
    /// This creates a memdb_index_t structure matching the C implementation,
    /// containing metadata and a sorted list of (inode, digest) pairs.
    /// This is sent as the "state" during DFSM synchronization.
    pub fn encode_index(&self) -> Result<MemDbIndex> {
        // Acquire locks in consistent order: conn, then index
        // This prevents races where version changes between read and root update
        let conn = self.inner.conn.lock();
        let mut index = self.inner.index.lock();

        // Read global version once under both locks to ensure consistency
        // No other operation can modify version counter while we hold both locks
        let global_version = self.inner.version.load(Ordering::SeqCst);

        let root_inode = self.inner.root_inode;
        let mut root_version_updated = false;
        if let Some(root_entry) = index.get_mut(&root_inode) {
            if root_entry.version != global_version {
                root_entry.version = global_version;
                root_version_updated = true;
            }
        } else {
            anyhow::bail!("Root entry not found in index");
        }

        // If root version was updated, persist to database atomically
        // Both DB and memory are updated under locks for consistency
        if root_version_updated {
            let root_entry = index.get(&root_inode).unwrap();  // Safe: we just checked it exists

            // Begin transaction for atomic update
            let tx = conn.unchecked_transaction()
                .context("Failed to begin transaction for root version update")?;

            tx.execute(
                "UPDATE tree SET version = ? WHERE inode = ?",
                rusqlite::params![root_entry.version as i64, root_inode as i64],
            )
            .context("Failed to update root version in database")?;

            tx.commit().context("Failed to commit root version update")?;
        }

        drop(conn);

        // Collect ALL entries including root, sorted by inode
        let mut entries: Vec<&TreeEntry> = index.values().collect();
        entries.sort_by_key(|e| e.inode);

        tracing::info!("=== encode_index: Encoding {} entries ===", entries.len());
        for te in entries.iter() {
            tracing::info!(
                "  Entry: inode={:#018x}, parent={:#018x}, name='{}', type={}, version={}, writer={}, mtime={}, size={}",
                te.inode, te.parent, te.name, te.entry_type, te.version, te.writer, te.mtime, te.size
            );
        }

        // Create index entries with digests
        let index_entries: Vec<IndexEntry> = entries
            .iter()
            .map(|te| {
                let digest = MemDbIndex::compute_entry_digest(
                    te.inode,
                    te.parent,
                    te.version,
                    te.writer,
                    te.mtime,
                    te.size,
                    te.entry_type,
                    &te.name,
                    &te.data,
                );
                tracing::debug!(
                    "  Digest for inode {:#018x}: {:02x}{:02x}{:02x}{:02x}...{:02x}{:02x}{:02x}{:02x}",
                    te.inode,
                    digest[0], digest[1], digest[2], digest[3],
                    digest[28], digest[29], digest[30], digest[31]
                );
                IndexEntry { inode: te.inode, digest }
            })
            .collect();

        // Get root entry for mtime and writer_id (now updated with global version)
        let root_entry = index
            .get(&self.inner.root_inode)
            .ok_or_else(|| anyhow::anyhow!("Root entry not found in index"))?;

        let version = global_version;  // Already synchronized above
        let last_inode = index.keys().max().copied().unwrap_or(1);
        let writer = root_entry.writer;
        let mtime = root_entry.mtime;

        drop(index);

        Ok(MemDbIndex::new(
            version,
            last_inode,
            writer,
            mtime,
            index_entries,
        ))
    }

    /// Encode the entire database state into a byte array
    /// Matches C version's memdb_encode() function
    pub fn encode_database(&self) -> Result<Vec<u8>> {
        let index = self.inner.index.lock();

        // Collect all entries sorted by inode for consistent ordering
        // This matches the C implementation's memdb_tree_compare function
        let mut entries: Vec<&TreeEntry> = index.values().collect();
        entries.sort_by_key(|e| e.inode);

        // Log all entries for debugging
        tracing::info!(
            "Encoding database: {} entries",
            entries.len()
        );
        for entry in entries.iter() {
            tracing::info!(
                "  Entry: inode={}, name='{}', parent={}, type={}, size={}, version={}",
                entry.inode,
                entry.name,
                entry.parent,
                entry.entry_type,
                entry.size,
                entry.version
            );
        }

        // Serialize using bincode (compatible with C struct layout)
        let encoded = bincode::serialize(&entries)
            .map_err(|e| anyhow::anyhow!("Failed to encode database: {e}"))?;

        tracing::debug!(
            "Encoded database: {} entries, {} bytes",
            entries.len(),
            encoded.len()
        );

        Ok(encoded)
    }

    /// Compute checksum of the entire database state
    /// Used for DFSM state verification
    pub fn compute_database_checksum(&self) -> Result<[u8; 32]> {
        let encoded = self.encode_database()?;

        let mut hasher = Sha256::new();
        hasher.update(&encoded);

        Ok(hasher.finalize().into())
    }

    /// Decode database state from a byte array
    /// Used during DFSM state synchronization
    pub fn decode_database(data: &[u8]) -> Result<Vec<TreeEntry>> {
        let entries: Vec<TreeEntry> = bincode::deserialize(data)
            .map_err(|e| anyhow::anyhow!("Failed to decode database: {e}"))?;

        tracing::debug!("Decoded database: {} entries", entries.len());

        Ok(entries)
    }

    /// Synchronize corosync configuration from MemDb to filesystem
    ///
    /// Reads corosync.conf from memdb and writes to system file if changed.
    /// This syncs the cluster configuration from the distributed database
    /// to the local filesystem.
    ///
    /// # Arguments
    /// * `system_path` - Path to write the corosync.conf file (default: /etc/corosync/corosync.conf)
    /// * `force` - Force write even if unchanged
    pub fn sync_corosync_conf(&self, system_path: Option<&str>, force: bool) -> Result<()> {
        let system_path = system_path.unwrap_or("/etc/corosync/corosync.conf");
        tracing::info!(
            "Syncing corosync configuration to {} (force={})",
            system_path,
            force
        );

        // Path in memdb for corosync.conf
        let memdb_path = "/corosync.conf";

        // Try to read from memdb
        let memdb_data = match self.lookup_path(memdb_path) {
            Some(entry) if entry.is_file() => entry.data,
            Some(_) => {
                return Err(anyhow::anyhow!("{memdb_path} exists but is not a file"));
            }
            None => {
                tracing::debug!("{} not found in memdb, nothing to sync", memdb_path);
                return Ok(());
            }
        };

        // Read current system file if it exists
        let system_data = std::fs::read(system_path).ok();

        // Determine if we need to write
        let should_write = force || system_data.as_ref() != Some(&memdb_data);

        if !should_write {
            tracing::debug!("Corosync configuration unchanged, skipping write");
            return Ok(());
        }

        // SAFETY CHECK: Writing to /etc requires root permissions
        // We'll attempt the write but log clearly if it fails
        tracing::info!(
            "Corosync configuration changed (size: {} bytes), updating {}",
            memdb_data.len(),
            system_path
        );

        // Basic validation: check if it looks like a valid corosync config
        let config_str =
            std::str::from_utf8(&memdb_data).context("Corosync config is not valid UTF-8")?;

        if !config_str.contains("totem") {
            tracing::warn!("Corosync config validation: missing 'totem' section");
        }
        if !config_str.contains("nodelist") {
            tracing::warn!("Corosync config validation: missing 'nodelist' section");
        }

        // Attempt to write (will fail if not root or no permissions)
        match std::fs::write(system_path, &memdb_data) {
            Ok(()) => {
                tracing::info!("Successfully updated {}", system_path);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                tracing::warn!(
                    "Permission denied writing {}: {}. Run as root to enable corosync sync.",
                    system_path,
                    e
                );
                // Don't return error - this is expected in non-root mode
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to write {system_path}: {e}")),
        }
    }
}
