/// DFSM callbacks implementation for memdb synchronization
///
/// This module implements the DfsmCallbacks trait to integrate the DFSM
/// state machine with the memdb database for cluster-wide synchronization.
use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use pmxcfs_dfsm::{Callbacks, DfsmBroadcast, FuseMessage, NodeSyncInfo};
use pmxcfs_memdb::{MemDb, MemDbIndex};

/// DFSM callbacks for memdb synchronization
pub struct MemDbCallbacks {
    memdb: MemDb,
    status: Arc<pmxcfs_status::Status>,
    dfsm: RwLock<Weak<pmxcfs_dfsm::Dfsm<FuseMessage>>>,
}

impl MemDbCallbacks {
    /// Create new callbacks for a memdb instance
    pub fn new(memdb: MemDb, status: Arc<pmxcfs_status::Status>) -> Arc<Self> {
        Arc::new(Self {
            memdb,
            status,
            dfsm: RwLock::new(Weak::new()),
        })
    }

    /// Set the DFSM instance (called after DFSM is created)
    pub fn set_dfsm(&self, dfsm: &Arc<pmxcfs_dfsm::Dfsm<FuseMessage>>) {
        *self.dfsm.write() = Arc::downgrade(dfsm);
    }

    /// Get the DFSM instance if available
    fn get_dfsm(&self) -> Option<Arc<pmxcfs_dfsm::Dfsm<FuseMessage>>> {
        self.dfsm.read().upgrade()
    }

    /// Update version counters based on path changes
    /// Matches the C implementation's update_node_status_version logic
    fn update_version_counters(&self, path: &str) {
        // Trim leading slash but use FULL path for version tracking
        let path = path.trim_start_matches('/');

        // Update path-specific version counter (use full path, not just first component)
        self.status.increment_path_version(path);

        // Update vmlist version for VM configuration changes
        if path.starts_with("qemu-server/") || path.starts_with("lxc/") {
            self.status.increment_vmlist_version();
        }
    }
}

impl Callbacks for MemDbCallbacks {
    type Message = FuseMessage;

    /// Deliver an application message
    /// Returns (message_result, processed) where processed indicates if message was handled
    fn deliver_message(
        &self,
        nodeid: u32,
        pid: u32,
        fuse_message: FuseMessage,
        timestamp: u64,
    ) -> Result<(i32, bool)> {
        // C-style delivery: ALL nodes (including originator) process messages
        // No loopback check needed - the originator waits for this delivery
        // and uses the result as the FUSE operation return value

        tracing::debug!(
            "MemDbCallbacks: delivering FUSE message from node {}/{} at timestamp {}",
            nodeid,
            pid,
            timestamp
        );

        let mtime = timestamp as u32;

        // Dispatch to dedicated handler for each message type
        match fuse_message {
            FuseMessage::Create { ref path } => {
                let result = self.handle_create(path, mtime);
                Ok((result, result >= 0))
            }
            FuseMessage::Mkdir { ref path } => {
                let result = self.handle_mkdir(path, mtime);
                Ok((result, result >= 0))
            }
            FuseMessage::Write {
                ref path,
                offset,
                ref data,
            } => {
                let result = self.handle_write(path, offset, data, mtime);
                Ok((result, result >= 0))
            }
            FuseMessage::Delete { ref path } => {
                let result = self.handle_delete(path);
                Ok((result, result >= 0))
            }
            FuseMessage::Rename { ref from, ref to } => {
                let result = self.handle_rename(from, to);
                Ok((result, result >= 0))
            }
            FuseMessage::Mtime { ref path, mtime: msg_mtime } => {
                // Use mtime from message, not from timestamp (C: dcdb.c:900-901)
                let result = self.handle_mtime(path, nodeid, msg_mtime);
                Ok((result, result >= 0))
            }
            FuseMessage::UnlockRequest { path } => {
                self.handle_unlock_request(path)?;
                Ok((0, true))
            }
            FuseMessage::Unlock { path } => {
                self.handle_unlock(path)?;
                Ok((0, true))
            }
        }
    }

    /// Compute state checksum for verification
    /// Should compute SHA-256 checksum of current state
    fn compute_checksum(&self, output: &mut [u8; 32]) -> Result<()> {
        tracing::debug!("MemDbCallbacks: computing database checksum");

        let checksum = self
            .memdb
            .compute_database_checksum()
            .context("Failed to compute database checksum")?;

        output.copy_from_slice(&checksum);

        tracing::debug!("MemDbCallbacks: checksum = {:016x?}", &checksum[..8]);
        Ok(())
    }

    /// Get current state for synchronization
    fn get_state(&self) -> Result<Vec<u8>> {
        tracing::debug!("MemDbCallbacks: generating state for synchronization");

        // Generate MemDbIndex from current database
        let index = self
            .memdb
            .encode_index()
            .context("Failed to encode database index")?;

        // Serialize to wire format
        let serialized = index.serialize();

        tracing::info!(
            "MemDbCallbacks: state generated - version={}, entries={}, bytes={}",
            index.version,
            index.size,
            serialized.len()
        );

        Ok(serialized)
    }

    /// Process state update during synchronization
    /// Called when all states have been collected from nodes
    fn process_state_update(&self, states: &[NodeSyncInfo]) -> Result<bool> {
        tracing::info!(
            "MemDbCallbacks: processing state update from {} nodes",
            states.len()
        );

        // Parse all indices from node states
        let mut indices: Vec<(u32, u32, MemDbIndex)> = Vec::new();

        for node in states {
            if let Some(state_data) = &node.state {
                match MemDbIndex::deserialize(state_data) {
                    Ok(index) => {
                        tracing::info!(
                            "MemDbCallbacks: node {}/{} - version={}, entries={}, mtime={}",
                            node.node_id,
                            node.pid,
                            index.version,
                            index.size,
                            index.mtime
                        );
                        indices.push((node.node_id, node.pid, index));
                    }
                    Err(e) => {
                        tracing::error!(
                            "MemDbCallbacks: failed to parse index from node {}/{}: {}",
                            node.node_id,
                            node.pid,
                            e
                        );
                    }
                }
            }
        }

        if indices.is_empty() {
            tracing::warn!("MemDbCallbacks: no valid indices from any node");
            return Ok(true);
        }

        // Find leader (highest version, or if tie, highest mtime)
        // Matches C's dcdb_choose_leader_with_highest_index()
        let mut leader_idx = 0;
        for i in 1..indices.len() {
            let (_, _, current_index) = &indices[i];
            let (_, _, leader_index) = &indices[leader_idx];

            if current_index > leader_index {
                leader_idx = i;
            }
        }

        let (leader_nodeid, leader_pid, leader_index) = &indices[leader_idx];
        tracing::info!(
            "MemDbCallbacks: elected leader: {}/{} (version={}, mtime={})",
            leader_nodeid,
            leader_pid,
            leader_index.version,
            leader_index.mtime
        );

        // Build list of synced nodes (those whose index matches leader exactly)
        let mut synced_nodes = Vec::new();
        for (nodeid, pid, index) in &indices {
            // Check if indices are identical (same version, mtime, and all entries)
            let is_synced = index.version == leader_index.version
                && index.mtime == leader_index.mtime
                && index.size == leader_index.size
                && index.entries.len() == leader_index.entries.len()
                && index
                    .entries
                    .iter()
                    .zip(leader_index.entries.iter())
                    .all(|(a, b)| a.inode == b.inode && a.digest == b.digest);

            if is_synced {
                synced_nodes.push((*nodeid, *pid));
                tracing::info!(
                    "MemDbCallbacks: node {}/{} is synced with leader",
                    nodeid,
                    pid
                );
            } else {
                tracing::info!("MemDbCallbacks: node {}/{} needs updates", nodeid, pid);
            }
        }

        // Get DFSM instance to check if we're the leader
        let dfsm = self.get_dfsm();

        // Determine if WE are the leader
        let we_are_leader = dfsm
            .as_ref()
            .map(|d| d.get_nodeid() == *leader_nodeid && d.get_pid() == *leader_pid)
            .unwrap_or(false);

        // Determine if WE are synced
        let we_are_synced = dfsm
            .as_ref()
            .map(|d| {
                let our_nodeid = d.get_nodeid();
                let our_pid = d.get_pid();
                synced_nodes
                    .iter()
                    .any(|(nid, pid)| *nid == our_nodeid && *pid == our_pid)
            })
            .unwrap_or(false);

        if we_are_leader {
            tracing::info!("MemDbCallbacks: we are the leader, sending updates to followers");

            // Send updates to followers
            if let Some(dfsm) = dfsm {
                self.send_updates_to_followers(&dfsm, leader_index, &indices)?;
            } else {
                tracing::error!("MemDbCallbacks: cannot send updates - DFSM not available");
            }

            // Leader is always synced
            Ok(true)
        } else if we_are_synced {
            tracing::info!("MemDbCallbacks: we are synced with leader");
            Ok(true)
        } else {
            tracing::info!("MemDbCallbacks: we need updates from leader, entering Update mode");
            Ok(false)
        }
    }

    /// Process incremental update from leader
    ///
    /// Deserializes a TreeEntry from the wire format and applies it to the local database.
    /// Matches C's dcdb_parse_update_inode() function.
    fn process_update(&self, nodeid: u32, pid: u32, data: &[u8]) -> Result<()> {
        tracing::debug!(
            "MemDbCallbacks: processing update from {}/{} ({} bytes)",
            nodeid,
            pid,
            data.len()
        );

        // Deserialize TreeEntry from C wire format
        let tree_entry = pmxcfs_memdb::TreeEntry::deserialize_from_update(data)
            .context("Failed to deserialize TreeEntry from update message")?;

        tracing::info!(
            "MemDbCallbacks: received update for inode {} ({}), version={}",
            tree_entry.inode,
            tree_entry.name,
            tree_entry.version
        );

        // Apply the entry to our local database
        self.memdb
            .apply_tree_entry(tree_entry)
            .context("Failed to apply TreeEntry to database")?;

        tracing::debug!("MemDbCallbacks: update applied successfully");
        Ok(())
    }

    /// Commit synchronized state
    fn commit_state(&self) -> Result<()> {
        tracing::info!("MemDbCallbacks: committing synchronized state");
        // Database commits are automatic in our implementation

        // Increment all path versions to notify clients of database reload
        // Matches C's record_memdb_reload() called in database.c:607
        self.status.increment_all_path_versions();

        // Recreate VM list after database changes (matching C's bdb_backend_commit_update)
        // This ensures VM list is updated whenever the cluster database is synchronized
        self.status.scan_vmlist(&self.memdb);

        Ok(())
    }

    /// Called when cluster becomes synced
    fn on_synced(&self) {
        tracing::info!("MemDbCallbacks: cluster is now fully synchronized");
    }
}

// Helper methods for MemDbCallbacks (not part of trait)
impl MemDbCallbacks {
    /// Handle Create message - create an empty file
    /// Returns 0 on success, negative errno on failure
    fn handle_create(&self, path: &str, mtime: u32) -> i32 {
        match self.memdb.create(path, 0, 0, mtime) {
            Ok(_) => {
                tracing::info!("MemDbCallbacks: created file '{}'", path);
                self.update_version_counters(path);
                0
            }
            Err(e) => {
                tracing::warn!("MemDbCallbacks: failed to create '{}': {}", path, e);
                -libc::EACCES
            }
        }
    }

    /// Handle Mkdir message - create a directory
    /// Returns 0 on success, negative errno on failure
    fn handle_mkdir(&self, path: &str, mtime: u32) -> i32 {
        match self.memdb.create(path, libc::S_IFDIR, 0, mtime) {
            Ok(_) => {
                tracing::info!("MemDbCallbacks: created directory '{}'", path);
                self.update_version_counters(path);
                0
            }
            Err(e) => {
                tracing::warn!("MemDbCallbacks: failed to mkdir '{}': {}", path, e);
                -libc::EACCES
            }
        }
    }

    /// Handle Write message - write data to a file
    /// Returns 0 on success, negative errno on failure
    fn handle_write(&self, path: &str, offset: u64, data: &[u8], mtime: u32) -> i32 {
        // Create file if it doesn't exist
        if let Err(e) = self.memdb.exists(path) {
            tracing::warn!("MemDbCallbacks: failed to check if '{}' exists: {}", path, e);
            return -libc::EIO;
        }

        if !self.memdb.exists(path).unwrap_or(false) {
            if let Err(e) = self.memdb.create(path, 0, 0, mtime) {
                tracing::warn!("MemDbCallbacks: failed to create '{}': {}", path, e);
                return -libc::EACCES;
            }
        }

        // Write data
        if !data.is_empty() {
            match self.memdb.write(path, offset, 0, mtime, data, false) {
                Ok(_) => {
                    tracing::info!(
                        "MemDbCallbacks: wrote {} bytes to '{}' at offset {}",
                        data.len(),
                        path,
                        offset
                    );
                    self.update_version_counters(path);
                    0
                }
                Err(e) => {
                    tracing::warn!("MemDbCallbacks: failed to write to '{}': {}", path, e);
                    -libc::EACCES
                }
            }
        } else {
            0
        }
    }

    /// Handle Delete message - delete a file or directory
    /// Returns 0 on success, negative errno on failure
    fn handle_delete(&self, path: &str) -> i32 {
        match self.memdb.exists(path) {
            Ok(exists) if exists => match self.memdb.delete(path, 0, SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as u32) {
                Ok(_) => {
                    tracing::info!("MemDbCallbacks: deleted '{}'", path);
                    self.update_version_counters(path);
                    0
                }
                Err(e) => {
                    tracing::warn!("MemDbCallbacks: failed to delete '{}': {}", path, e);
                    -libc::EACCES
                }
            },
            Ok(_) => {
                tracing::debug!("MemDbCallbacks: path '{}' already deleted", path);
                0 // Not an error - already deleted
            }
            Err(e) => {
                tracing::warn!("MemDbCallbacks: failed to check if '{}' exists: {}", path, e);
                -libc::EIO
            }
        }
    }

    /// Handle Rename message - rename a file or directory
    /// Returns 0 on success, negative errno on failure
    fn handle_rename(&self, from: &str, to: &str) -> i32 {
        match self.memdb.exists(from) {
            Ok(exists) if exists => match self.memdb.rename(from, to, 0, SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as u32) {
                Ok(_) => {
                    tracing::info!("MemDbCallbacks: renamed '{}' to '{}'", from, to);
                    self.update_version_counters(from);
                    self.update_version_counters(to);
                    0
                }
                Err(e) => {
                    tracing::warn!("MemDbCallbacks: failed to rename '{}' to '{}': {}", from, to, e);
                    -libc::EACCES
                }
            },
            Ok(_) => {
                tracing::debug!("MemDbCallbacks: source path '{}' not found for rename", from);
                -libc::ENOENT
            }
            Err(e) => {
                tracing::warn!("MemDbCallbacks: failed to check if '{}' exists: {}", from, e);
                -libc::EIO
            }
        }
    }

    /// Handle Mtime message - update modification time
    /// Returns 0 on success, negative errno on failure
    fn handle_mtime(&self, path: &str, nodeid: u32, mtime: u32) -> i32 {
        match self.memdb.exists(path) {
            Ok(exists) if exists => match self.memdb.set_mtime(path, nodeid, mtime) {
                Ok(_) => {
                    tracing::info!(
                        "MemDbCallbacks: updated mtime for '{}' from node {}",
                        path,
                        nodeid
                    );
                    self.update_version_counters(path);
                    0
                }
                Err(e) => {
                    tracing::warn!("MemDbCallbacks: failed to update mtime for '{}': {}", path, e);
                    -libc::EACCES
                }
            },
            Ok(_) => {
                tracing::debug!("MemDbCallbacks: path '{}' not found for mtime update", path);
                -libc::ENOENT
            }
            Err(e) => {
                tracing::warn!("MemDbCallbacks: failed to check if '{}' exists: {}", path, e);
                -libc::EIO
            }
        }
    }

    /// Handle UnlockRequest message - check if lock expired and broadcast Unlock if needed
    ///
    /// Only the leader processes unlock requests (C: dcdb.c:830-838)
    fn handle_unlock_request(&self, path: String) -> Result<()> {
        tracing::debug!("MemDbCallbacks: processing unlock request for: {}", path);

        // Only the leader (lowest nodeid) should process unlock requests
        if let Some(dfsm) = self.get_dfsm() {
            if !dfsm.is_leader() {
                tracing::debug!("Not leader, ignoring unlock request for: {}", path);
                return Ok(());
            }
        } else {
            tracing::warn!("DFSM not available, cannot process unlock request");
            return Ok(());
        }

        // Get the lock entry to compute checksum
        if let Some(entry) = self.memdb.lookup_path(&path)
            && entry.is_dir()
            && pmxcfs_memdb::is_lock_path(&path)
        {
            let csum = entry.compute_checksum();

            // Check if lock expired (C: dcdb.c:834)
            if self.memdb.lock_expired(&path, &csum) {
                tracing::info!("Lock expired, sending unlock message for: {}", path);
                // Send Unlock message to cluster (C: dcdb.c:836)
                self.get_dfsm().broadcast(FuseMessage::Unlock { path: path.clone() });
            } else {
                tracing::debug!("Lock not expired for: {}", path);
            }
        }

        Ok(())
    }

    /// Handle Unlock message - delete an expired lock
    ///
    /// This is broadcast by the leader when a lock expires (C: dcdb.c:834)
    fn handle_unlock(&self, path: String) -> Result<()> {
        tracing::info!("MemDbCallbacks: processing unlock message for: {}", path);

        // Delete the lock directory
        if let Err(e) = self.memdb.delete(&path, 0, SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as u32) {
            tracing::warn!("Failed to delete lock {}: {}", path, e);
        } else {
            tracing::info!("Successfully deleted lock: {}", path);
            self.update_version_counters(&path);
        }

        Ok(())
    }

    /// Send updates to followers (leader only)
    ///
    /// Compares the leader index with each follower and sends Update messages
    /// for entries that differ. Matches C's dcdb_create_and_send_updates().
    fn send_updates_to_followers(
        &self,
        dfsm: &pmxcfs_dfsm::Dfsm<FuseMessage>,
        leader_index: &MemDbIndex,
        all_indices: &[(u32, u32, MemDbIndex)],
    ) -> Result<()> {
        use std::collections::HashSet;

        // Collect all inodes that need updating across all followers
        let mut inodes_to_update: HashSet<u64> = HashSet::new();
        let mut any_follower_needs_updates = false;

        for (_nodeid, _pid, follower_index) in all_indices {
            // Skip if this is us (the leader) - check if indices are identical
            // Must match the same check in process_state_update()
            let is_synced = follower_index.version == leader_index.version
                && follower_index.mtime == leader_index.mtime
                && follower_index.size == leader_index.size
                && follower_index.entries.len() == leader_index.entries.len();

            if is_synced {
                continue;
            }

            // This follower needs updates
            any_follower_needs_updates = true;

            // Find differences between leader and this follower
            let diffs = leader_index.find_differences(follower_index);
            tracing::debug!(
                "MemDbCallbacks: found {} differing inodes for follower",
                diffs.len()
            );
            inodes_to_update.extend(diffs);
        }

        // If no follower needs updates at all, we're done
        if !any_follower_needs_updates {
            tracing::info!("MemDbCallbacks: no updates needed, all nodes are synced");
            dfsm.send_update_complete()?;
            return Ok(());
        }

        tracing::info!(
            "MemDbCallbacks: sending updates ({} differing entries)",
            inodes_to_update.len()
        );

        // Send Update message for each differing inode
        // IMPORTANT: Do NOT send the root directory entry (inode ROOT_INODE)!
        // C uses inode 0 for root and never stores it in the database.
        // The root exists only in memory and is recreated on database reload.
        // Only send regular files and directories (inode > ROOT_INODE).
        let mut sent_count = 0;
        for inode in inodes_to_update {
            // Skip root - it should never be sent as an UPDATE
            if inode == pmxcfs_memdb::ROOT_INODE {
                tracing::debug!("MemDbCallbacks: skipping root entry (inode {})", inode);
                continue;
            }

            // Look up the TreeEntry for this inode
            match self.memdb.get_entry_by_inode(inode) {
                Some(tree_entry) => {
                    tracing::info!(
                        "MemDbCallbacks: sending UPDATE for inode {:#018x} (name='{}', parent={:#018x}, type={}, version={}, size={})",
                        inode,
                        tree_entry.name,
                        tree_entry.parent,
                        tree_entry.entry_type,
                        tree_entry.version,
                        tree_entry.size
                    );

                    if let Err(e) = dfsm.send_update(tree_entry) {
                        tracing::error!(
                            "MemDbCallbacks: failed to send update for inode {}: {}",
                            inode,
                            e
                        );
                        // Continue sending other updates even if one fails
                    } else {
                        sent_count += 1;
                    }
                }
                None => {
                    tracing::error!(
                        "MemDbCallbacks: cannot find TreeEntry for inode {} in database",
                        inode
                    );
                }
            }
        }

        tracing::info!("MemDbCallbacks: sent {} updates", sent_count);

        // Send UpdateComplete to signal end of updates
        dfsm.send_update_complete()?;
        tracing::info!("MemDbCallbacks: sent UpdateComplete");

        Ok(())
    }
}
