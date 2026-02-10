//! DFSM Callbacks for Status Synchronization (kvstore)
//!
//! This module implements the DfsmCallbacks trait for the status kvstore DFSM instance.
//! It handles synchronization of ephemeral status data across the cluster:
//! - Key-value status updates from nodes (RRD data, IP addresses, etc.)
//! - Cluster log entries
//!
//! Equivalent to C implementation's kvstore DFSM callbacks in status.c
//!
//! Note: The kvstore DFSM doesn't use FuseMessage like the main database DFSM.
//! It uses raw CPG messages for lightweight status synchronization.
//! Most DfsmCallbacks methods are stubbed since status data is ephemeral and
//! doesn't require the full database synchronization machinery.

use pmxcfs_dfsm::{Callbacks, KvStoreMessage, NodeSyncInfo};
use pmxcfs_status::Status;
use std::sync::Arc;
use tracing::{debug, warn};

/// Callbacks for status synchronization DFSM (kvstore)
///
/// This implements the DfsmCallbacks trait but only uses basic CPG event handling.
/// Most methods are stubbed since kvstore doesn't use database synchronization.
pub struct StatusCallbacks {
    status: Arc<Status>,
}

impl StatusCallbacks {
    /// Create new status callbacks
    pub fn new(status: Arc<Status>) -> Self {
        Self { status }
    }
}

impl Callbacks for StatusCallbacks {
    type Message = KvStoreMessage;

    /// Deliver a message - handles KvStore messages for status synchronization
    ///
    /// The kvstore DFSM handles KvStore messages (UPDATE, LOG, etc.) for
    /// ephemeral status data synchronization across the cluster.
    fn deliver_message(
        &self,
        nodeid: u32,
        pid: u32,
        kvstore_message: KvStoreMessage,
        timestamp: u64,
    ) -> anyhow::Result<(i32, bool)> {
        debug!(nodeid, pid, timestamp, "Delivering KvStore message");

        // Handle different KvStore message types
        match kvstore_message {
            KvStoreMessage::Update { key, value } => {
                debug!(key, value_len = value.len(), "KvStore UPDATE");

                // Store the key-value data for this node (matches C's cfs_kvstore_node_set)
                self.status.set_node_kv(nodeid, key, value);
                Ok((0, true))
            }
            KvStoreMessage::Log {
                time,
                priority,
                node,
                ident,
                tag,
                message,
            } => {
                debug!(
                    time, priority, %node, %ident, %tag, %message,
                    "KvStore LOG"
                );

                // Add log entry to cluster log
                if let Err(e) = self
                    .status
                    .add_remote_cluster_log(time, priority, node, ident, tag, message)
                {
                    warn!(error = %e, "Failed to add cluster log entry");
                }

                Ok((0, true))
            }
            KvStoreMessage::UpdateComplete => {
                debug!("KvStore UpdateComplete");
                Ok((0, true))
            }
        }
    }

    /// Compute checksum (not used by kvstore - ephemeral data doesn't need checksums)
    fn compute_checksum(&self, output: &mut [u8; 32]) -> anyhow::Result<()> {
        // Status data is ephemeral and doesn't use checksums
        output.fill(0);
        Ok(())
    }

    /// Get state for synchronization (returns cluster log state)
    ///
    /// Returns the cluster log in C-compatible binary format (clog_base_t).
    /// This enables mixed C/Rust cluster operation - C nodes can deserialize
    /// the state we send, and we can deserialize states from C nodes.
    fn get_state(&self) -> anyhow::Result<Vec<u8>> {
        debug!("Status kvstore: get_state called - serializing cluster log");
        self.status.get_cluster_log_state()
    }

    /// Process state update (handles cluster log state sync)
    ///
    /// Deserializes cluster log states from remote nodes and merges them with
    /// the local log. This enables cluster-wide log synchronization in mixed
    /// C/Rust clusters.
    fn process_state_update(&self, states: &[NodeSyncInfo]) -> anyhow::Result<bool> {
        debug!(
            "Status kvstore: process_state_update called with {} states",
            states.len()
        );

        if states.is_empty() {
            return Ok(true);
        }

        self.status.merge_cluster_log_states(states)?;
        Ok(true)
    }

    /// Process incremental update (not used by kvstore)
    ///
    /// Kvstore uses direct CPG messages (UPDATE, LOG) instead of incremental sync
    fn process_update(&self, _nodeid: u32, _pid: u32, _data: &[u8]) -> anyhow::Result<()> {
        warn!("Status kvstore: received unexpected process_update call");
        Ok(())
    }

    /// Commit state (no-op for kvstore - ephemeral data, no database commit)
    fn commit_state(&self) -> anyhow::Result<()> {
        // No commit needed for ephemeral status data
        Ok(())
    }

    /// Called when cluster becomes synced
    fn on_synced(&self) {
        debug!("Status kvstore: cluster synced");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmxcfs_dfsm::KvStoreMessage;
    use pmxcfs_status::ClusterLogEntry;

    #[test]
    fn test_kvstore_update_message_handling() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = Arc::new(Status::new(config, None));
        let callbacks = StatusCallbacks::new(status.clone());

        // Initialize cluster and register node 2
        status.init_cluster("test-cluster".to_string());
        status.register_node(2, "node2".to_string(), "192.168.1.11".to_string());

        // Simulate receiving a kvstore UPDATE message from node 2
        let key = "test-key".to_string();
        let value = b"test-value".to_vec();
        let message = KvStoreMessage::Update {
            key: key.clone(),
            value: value.clone(),
        };

        let result = callbacks.deliver_message(2, 1000, message, 12345);
        assert!(result.is_ok(), "deliver_message should succeed");

        let (res, continue_processing) = result.unwrap();
        assert_eq!(res, 0, "Result code should be 0 for success");
        assert!(continue_processing, "Should continue processing");

        // Verify the data was stored in kvstore
        let stored_value = status.get_node_kv(2, &key);
        assert_eq!(
            stored_value,
            Some(value),
            "Should store the key-value pair for node 2"
        );
    }

    #[test]
    fn test_kvstore_update_multiple_nodes() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = Arc::new(Status::new(config, None));
        let callbacks = StatusCallbacks::new(status.clone());

        // Initialize cluster and register nodes
        status.init_cluster("test-cluster".to_string());
        status.register_node(1, "node1".to_string(), "192.168.1.10".to_string());
        status.register_node(2, "node2".to_string(), "192.168.1.11".to_string());

        // Store data from multiple nodes
        let msg1 = KvStoreMessage::Update {
            key: "ip".to_string(),
            value: b"192.168.1.10".to_vec(),
        };
        let msg2 = KvStoreMessage::Update {
            key: "ip".to_string(),
            value: b"192.168.1.11".to_vec(),
        };

        callbacks.deliver_message(1, 1000, msg1, 12345).unwrap();
        callbacks.deliver_message(2, 1001, msg2, 12346).unwrap();

        // Verify each node's data is stored separately
        assert_eq!(
            status.get_node_kv(1, "ip"),
            Some(b"192.168.1.10".to_vec()),
            "Node 1 IP should be stored"
        );
        assert_eq!(
            status.get_node_kv(2, "ip"),
            Some(b"192.168.1.11".to_vec()),
            "Node 2 IP should be stored"
        );
    }

    #[test]
    fn test_kvstore_log_message_handling() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = Arc::new(Status::new(config, None));
        let callbacks = StatusCallbacks::new(status.clone());

        // Clear any existing log entries
        status.clear_cluster_log();

        // Simulate receiving a LOG message
        let message = KvStoreMessage::Log {
            time: 1234567890,
            priority: 6, // LOG_INFO
            node: "node1".to_string(),
            ident: "pmxcfs".to_string(),
            tag: "cluster".to_string(),
            message: "Test log entry".to_string(),
        };

        let result = callbacks.deliver_message(1, 1000, message, 12345);
        assert!(result.is_ok(), "LOG message delivery should succeed");

        // Verify the log entry was added
        let log_entries = status.get_log_entries(10);
        assert_eq!(log_entries.len(), 1, "Should have 1 log entry");
        assert_eq!(log_entries[0].node, "node1");
        assert_eq!(log_entries[0].message, "Test log entry");
        assert_eq!(log_entries[0].priority, 6);
    }

    #[test]
    fn test_kvstore_update_complete_message() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = Arc::new(Status::new(config, None));
        let callbacks = StatusCallbacks::new(status.clone());

        let message = KvStoreMessage::UpdateComplete;

        let result = callbacks.deliver_message(1, 1000, message, 12345);
        assert!(result.is_ok(), "UpdateComplete should succeed");

        let (res, continue_processing) = result.unwrap();
        assert_eq!(res, 0);
        assert!(continue_processing);
    }

    #[test]
    fn test_compute_checksum_returns_zeros() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = Arc::new(Status::new(config, None));
        let callbacks = StatusCallbacks::new(status);

        let mut checksum = [0u8; 32];
        let result = callbacks.compute_checksum(&mut checksum);

        assert!(result.is_ok(), "compute_checksum should succeed");
        assert_eq!(
            checksum, [0u8; 32],
            "Checksum should be all zeros for ephemeral data"
        );
    }

    #[test]
    fn test_get_state_returns_cluster_log() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = Arc::new(Status::new(config, None));
        let callbacks = StatusCallbacks::new(status.clone());

        // Add a log entry first
        status.clear_cluster_log();
        let entry = ClusterLogEntry {
            uid: 0,
            timestamp: 1234567890,
            priority: 6,
            tag: "test".to_string(),
            pid: 0,
            node: "node1".to_string(),
            ident: "pmxcfs".to_string(),
            message: "Test message".to_string(),
        };
        status.add_log_entry(entry);

        // Get state should return serialized cluster log
        let result = callbacks.get_state();
        assert!(result.is_ok(), "get_state should succeed");

        let state = result.unwrap();
        assert!(
            !state.is_empty(),
            "State should not be empty when cluster log has entries"
        );
    }

    #[test]
    fn test_process_state_update_with_empty_states() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = Arc::new(Status::new(config, None));
        let callbacks = StatusCallbacks::new(status);

        let states: Vec<NodeSyncInfo> = vec![];
        let result = callbacks.process_state_update(&states);

        assert!(result.is_ok(), "Empty state update should succeed");
        assert!(result.unwrap(), "Should return true for empty states");
    }

    #[test]
    fn test_process_update_logs_warning() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = Arc::new(Status::new(config, None));
        let callbacks = StatusCallbacks::new(status);

        // process_update is not used by kvstore, but should not fail
        let result = callbacks.process_update(1, 1000, &[1, 2, 3]);
        assert!(
            result.is_ok(),
            "process_update should succeed even though not used"
        );
    }

    #[test]
    fn test_commit_state_is_noop() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = Arc::new(Status::new(config, None));
        let callbacks = StatusCallbacks::new(status);

        let result = callbacks.commit_state();
        assert!(result.is_ok(), "commit_state should succeed (no-op)");
    }
}
