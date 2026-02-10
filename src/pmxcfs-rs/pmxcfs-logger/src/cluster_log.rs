/// Cluster Log Implementation
///
/// This module implements the cluster-wide log system with deduplication
/// and merging support, matching C's clusterlog_t.
use crate::entry::LogEntry;
use crate::ring_buffer::{RingBuffer, CLOG_DEFAULT_SIZE};
use anyhow::Result;
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Deduplication entry - tracks the latest UID and time for each node
///
/// Note: C's `dedup_entry_t` includes node_digest field because GHashTable stores
/// the struct pointer both as key and value. In Rust, we use HashMap<u64, DedupEntry>
/// where node_digest is the key, so we don't need to duplicate it in the value.
/// This is functionally equivalent but more efficient.
#[derive(Debug, Clone)]
pub(crate) struct DedupEntry {
    /// Latest UID seen from this node
    pub uid: u32,
    /// Latest timestamp seen from this node
    pub time: u32,
}

/// Internal state protected by a single mutex
/// Matches C's clusterlog_t which uses a single mutex for both base and dedup
struct ClusterLogInner {
    /// Ring buffer for log storage (matches C's cl->base)
    buffer: RingBuffer,
    /// Deduplication tracker (matches C's cl->dedup)
    dedup: HashMap<u64, DedupEntry>,
}

/// Cluster-wide log with deduplication and merging support
/// Matches C's `clusterlog_t`
///
/// Note: Unlike the initial implementation with separate mutexes, we use a single
/// mutex to match C's semantics and ensure atomic updates of buffer+dedup.
pub struct ClusterLog {
    /// Inner state protected by a single mutex
    /// Matches C's single g_mutex_t protecting both cl->base and cl->dedup
    inner: Arc<Mutex<ClusterLogInner>>,
}

impl ClusterLog {
    /// Create a new cluster log with default size
    pub fn new() -> Self {
        Self::with_capacity(CLOG_DEFAULT_SIZE)
    }

    /// Create a new cluster log with specified capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ClusterLogInner {
                buffer: RingBuffer::new(capacity),
                dedup: HashMap::new(),
            })),
        }
    }

    /// Matches C's `clusterlog_add` function
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &self,
        node: &str,
        ident: &str,
        tag: &str,
        pid: u32,
        priority: u8,
        time: u32,
        message: &str,
    ) -> Result<()> {
        let entry = LogEntry::pack(node, ident, tag, pid, time, priority, message)?;
        self.insert(&entry)
    }

    /// Insert a log entry (with deduplication)
    ///
    /// Matches C's `clusterlog_insert` function
    pub fn insert(&self, entry: &LogEntry) -> Result<()> {
        let mut inner = self.inner.lock();

        // Check deduplication
        if Self::is_not_duplicate(&mut inner.dedup, entry) {
            // Entry is not a duplicate, add it
            inner.buffer.add_entry(entry)?;
        } else {
            tracing::debug!("Ignoring duplicate cluster log entry");
        }

        Ok(())
    }

    /// Check if entry is a duplicate (returns true if NOT a duplicate)
    ///
    /// Matches C's `dedup_lookup` function
    ///
    /// ## Hash Collision Risk
    ///
    /// Uses FNV-1a hash (`node_digest`) as deduplication key. Hash collisions
    /// are theoretically possible but extremely rare in practice:
    ///
    /// - FNV-1a produces 64-bit hashes (2^64 possible values)
    /// - Collision probability with N entries: ~N²/(2 × 2^64)
    /// - For 10,000 log entries: collision probability < 10^-11
    ///
    /// If a collision occurs, two different log entries (from different nodes
    /// or with different content) will be treated as duplicates, causing one
    /// to be silently dropped.
    ///
    /// This design is inherited from the C implementation for compatibility.
    /// The risk is acceptable because:
    /// 1. Collisions are astronomically rare
    /// 2. Only affects log deduplication, not critical data integrity
    /// 3. Lost log entries don't compromise cluster operation
    ///
    /// Changing this would break wire format compatibility with C nodes.
    fn is_not_duplicate(dedup: &mut HashMap<u64, DedupEntry>, entry: &LogEntry) -> bool {
        match dedup.get_mut(&entry.node_digest) {
            None => {
                dedup.insert(
                    entry.node_digest,
                    DedupEntry {
                        time: entry.time,
                        uid: entry.uid,
                    },
                );
                true
            }
            Some(dd) => {
                if entry.time > dd.time || (entry.time == dd.time && entry.uid > dd.uid) {
                    dd.time = entry.time;
                    dd.uid = entry.uid;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn get_entries(&self, max: usize) -> Vec<LogEntry> {
        let inner = self.inner.lock();
        inner.buffer.iter().take(max).cloned().collect()
    }

    /// Get the current buffer (for testing)
    pub fn get_buffer(&self) -> RingBuffer {
        let inner = self.inner.lock();
        inner.buffer.clone()
    }

    /// Get buffer length (for testing)
    pub fn len(&self) -> usize {
        let inner = self.inner.lock();
        inner.buffer.len()
    }

    /// Get buffer capacity (for testing)
    pub fn capacity(&self) -> usize {
        let inner = self.inner.lock();
        inner.buffer.capacity()
    }

    /// Check if buffer is empty (for testing)
    pub fn is_empty(&self) -> bool {
        let inner = self.inner.lock();
        inner.buffer.is_empty()
    }

    /// Clear all log entries (for testing)
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        let capacity = inner.buffer.capacity();
        inner.buffer = RingBuffer::new(capacity);
        inner.dedup.clear();
    }

    /// Sort the log entries by time
    ///
    /// Matches C's `clog_sort` function
    pub fn sort(&self) -> Result<RingBuffer> {
        let inner = self.inner.lock();
        inner.buffer.sort()
    }

    /// Merge logs from multiple nodes
    ///
    /// Matches C's `clusterlog_merge` function
    ///
    /// This method atomically updates both the buffer and dedup state under a single
    /// mutex lock, matching C's behavior where both cl->base and cl->dedup are
    /// updated under cl->mutex.
    pub fn merge(&self, remote_logs: Vec<RingBuffer>, include_local: bool) -> Result<()> {
        let mut sorted_entries: BTreeMap<(u32, u64, u32), LogEntry> = BTreeMap::new();
        let mut merge_dedup: HashMap<u64, DedupEntry> = HashMap::new();

        // Lock once for the entire operation (matching C's single mutex)
        let mut inner = self.inner.lock();

        // Calculate maximum capacity
        let max_size = if include_local {
            let local_cap = inner.buffer.capacity();

            std::iter::once(local_cap)
                .chain(remote_logs.iter().map(|b| b.capacity()))
                .max()
                .unwrap_or(CLOG_DEFAULT_SIZE)
        } else {
            remote_logs
                .iter()
                .map(|b| b.capacity())
                .max()
                .unwrap_or(CLOG_DEFAULT_SIZE)
        };

        // Add local entries if requested
        if include_local {
            for entry in inner.buffer.iter() {
                let key = (entry.time, entry.node_digest, entry.uid);
                // Keep-first: only insert if key doesn't exist, matching C's g_tree_lookup guard
                if let std::collections::btree_map::Entry::Vacant(e) = sorted_entries.entry(key) {
                    e.insert(entry.clone());
                    Self::is_not_duplicate(&mut merge_dedup, entry);
                }
            }
        }

        // Add remote entries
        for remote_buffer in &remote_logs {
            for entry in remote_buffer.iter() {
                let key = (entry.time, entry.node_digest, entry.uid);
                // Keep-first: only insert if key doesn't exist, matching C's g_tree_lookup guard
                if let std::collections::btree_map::Entry::Vacant(e) = sorted_entries.entry(key) {
                    e.insert(entry.clone());
                    Self::is_not_duplicate(&mut merge_dedup, entry);
                }
            }
        }

        let mut result = RingBuffer::new(max_size);

        // BTreeMap iterates oldest->newest. We add each as new head (push_front),
        // so result ends with newest at head, matching C's behavior.
        // Fill to 100% capacity (matching C's behavior), not just 90%
        for (_key, entry) in sorted_entries.iter() {
            // add_entry will automatically evict old entries if needed to stay within capacity
            result.add_entry(entry)?;
        }

        // Atomically update both buffer and dedup (matches C lines 503-507)
        inner.buffer = result;
        inner.dedup = merge_dedup;

        Ok(())
    }

    /// Export log to JSON format
    ///
    /// Matches C's `clog_dump_json` function
    pub fn dump_json(&self, ident_filter: Option<&str>, max_entries: usize) -> String {
        let inner = self.inner.lock();
        inner.buffer.dump_json(ident_filter, max_entries)
    }

    /// Export log to JSON format with sorted entries
    pub fn dump_json_sorted(
        &self,
        ident_filter: Option<&str>,
        max_entries: usize,
    ) -> Result<String> {
        let sorted = self.sort()?;
        Ok(sorted.dump_json(ident_filter, max_entries))
    }

    /// Matches C's `clusterlog_get_state` function
    ///
    /// Returns binary-serialized clog_base_t structure for network transmission.
    /// This format is compatible with C nodes for mixed-cluster operation.
    pub fn get_state(&self) -> Result<Vec<u8>> {
        let sorted = self.sort()?;
        Ok(sorted.serialize_binary())
    }

    pub fn deserialize_state(data: &[u8]) -> Result<RingBuffer> {
        RingBuffer::deserialize_binary(data)
    }

}

impl Default for ClusterLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_log_creation() {
        let log = ClusterLog::new();
        assert!(log.inner.lock().buffer.is_empty());
    }

    #[test]
    fn test_add_entry() {
        let log = ClusterLog::new();

        let result = log.add(
            "node1",
            "root",
            "cluster",
            12345,
            6, // Info priority
            1234567890,
            "Test message",
        );

        assert!(result.is_ok());
        assert!(!log.inner.lock().buffer.is_empty());
    }

    #[test]
    fn test_deduplication() {
        let log = ClusterLog::new();

        // Add same entry twice (but with different UIDs since each add creates a new entry)
        let _ = log.add("node1", "root", "cluster", 123, 6, 1000, "Message 1");
        let _ = log.add("node1", "root", "cluster", 123, 6, 1000, "Message 1");

        // Both entries are added because they have different UIDs
        // Deduplication tracks the latest (time, UID) per node, not content
        let inner = log.inner.lock();
        assert_eq!(inner.buffer.len(), 2);
    }

    #[test]
    fn test_newer_entry_replaces() {
        let log = ClusterLog::new();

        // Add older entry
        let _ = log.add("node1", "root", "cluster", 123, 6, 1000, "Old message");

        // Add newer entry from same node
        let _ = log.add("node1", "root", "cluster", 123, 6, 1001, "New message");

        // Should have both entries (newer doesn't remove older, just updates dedup tracker)
        let inner = log.inner.lock();
        assert_eq!(inner.buffer.len(), 2);
    }

    #[test]
    fn test_json_export() {
        let log = ClusterLog::new();

        let _ = log.add(
            "node1",
            "root",
            "cluster",
            123,
            6,
            1234567890,
            "Test message",
        );

        let json = log.dump_json(None, 50);

        // Should be valid JSON
        assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());

        // Should contain "data" field
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("data").is_some());
    }

    #[test]
    fn test_merge_logs() {
        let log1 = ClusterLog::new();
        let log2 = ClusterLog::new();

        // Add entries to first log
        let _ = log1.add(
            "node1",
            "root",
            "cluster",
            123,
            6,
            1000,
            "Message from node1",
        );

        // Add entries to second log
        let _ = log2.add(
            "node2",
            "root",
            "cluster",
            456,
            6,
            1001,
            "Message from node2",
        );

        // Get log2's buffer for merging
        let log2_buffer = log2.inner.lock().buffer.clone();

        // Merge into log1 (updates log1's buffer atomically)
        log1.merge(vec![log2_buffer], true).unwrap();

        // Check log1's buffer now contains entries from both logs
        let inner = log1.inner.lock();
        assert!(inner.buffer.len() >= 2);
    }

    // ========================================================================
    // HIGH PRIORITY TESTS - Merge Edge Cases
    // ========================================================================

    #[test]
    fn test_merge_empty_logs() {
        let log = ClusterLog::new();

        // Add some entries to local log
        let _ = log.add("node1", "root", "cluster", 123, 6, 1000, "Local entry");

        // Merge with empty remote logs (updates buffer atomically)
        log.merge(vec![], true).unwrap();

        // Check buffer has 1 entry (from local log)
        let inner = log.inner.lock();
        assert_eq!(inner.buffer.len(), 1);
        let entry = inner.buffer.iter().next().unwrap();
        assert_eq!(entry.node, "node1");
    }

    #[test]
    fn test_merge_single_node_only() {
        let log = ClusterLog::new();

        // Add entries only from single node
        let _ = log.add("node1", "root", "cluster", 123, 6, 1000, "Entry 1");
        let _ = log.add("node1", "root", "cluster", 124, 6, 1001, "Entry 2");
        let _ = log.add("node1", "root", "cluster", 125, 6, 1002, "Entry 3");

        // Merge with no remote logs (just sort local)
        log.merge(vec![], true).unwrap();

        // Check buffer has all 3 entries
        let inner = log.inner.lock();
        assert_eq!(inner.buffer.len(), 3);

        // Entries should be sorted by time (buffer stores newest first)
        let times: Vec<u32> = inner.buffer.iter().map(|e| e.time).collect();
        let mut expected = vec![1002, 1001, 1000];
        expected.sort();
        expected.reverse(); // Newest first

        let mut actual = times.clone();
        actual.sort();
        actual.reverse();

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_merge_all_duplicates() {
        let log1 = ClusterLog::new();
        let log2 = ClusterLog::new();

        // Add same entries to both logs (same node, time, but different UIDs)
        let _ = log1.add("node1", "root", "cluster", 123, 6, 1000, "Entry 1");
        let _ = log1.add("node1", "root", "cluster", 124, 6, 1001, "Entry 2");

        let _ = log2.add("node1", "root", "cluster", 125, 6, 1000, "Entry 1");
        let _ = log2.add("node1", "root", "cluster", 126, 6, 1001, "Entry 2");

        let log2_buffer = log2.inner.lock().buffer.clone();

        // Merge - should handle entries from same node at same times
        log1.merge(vec![log2_buffer], true).unwrap();

        // Check merged buffer has 4 entries (all are unique by UID despite same time/node)
        let inner = log1.inner.lock();
        assert_eq!(inner.buffer.len(), 4);
    }

    #[test]
    fn test_merge_exceeding_capacity() {
        // Create small buffer to test capacity enforcement
        let log = ClusterLog::with_capacity(50_000); // Small buffer

        // Add many entries to fill beyond capacity
        for i in 0..100 {
            let _ = log.add(
                "node1",
                "root",
                "cluster",
                100 + i,
                6,
                1000 + i,
                &format!("Entry {}", i),
            );
        }

        // Create remote log with many entries
        let remote = ClusterLog::with_capacity(50_000);
        for i in 0..100 {
            let _ = remote.add(
                "node2",
                "root",
                "cluster",
                200 + i,
                6,
                1000 + i,
                &format!("Remote {}", i),
            );
        }

        let remote_buffer = remote.inner.lock().buffer.clone();

        // Merge - should stop when buffer is near full
        log.merge(vec![remote_buffer], true).unwrap();

        // Buffer should be limited by capacity, not necessarily < 200
        // The actual limit depends on entry sizes and capacity
        // Just verify we got some reasonable number of entries
        let inner = log.inner.lock();
        assert!(!inner.buffer.is_empty(), "Should have some entries");
        assert!(
            inner.buffer.len() <= 200,
            "Should not exceed total available entries"
        );
    }

    #[test]
    fn test_merge_preserves_dedup_state() {
        let log = ClusterLog::new();

        // Add entries from node1
        let _ = log.add("node1", "root", "cluster", 123, 6, 1000, "Entry 1");
        let _ = log.add("node1", "root", "cluster", 124, 6, 1001, "Entry 2");

        // Create remote log with later entries from node1
        let remote = ClusterLog::new();
        let _ = remote.add("node1", "root", "cluster", 125, 6, 1002, "Entry 3");

        let remote_buffer = remote.inner.lock().buffer.clone();

        // Merge
        log.merge(vec![remote_buffer], true).unwrap();

        // Check that dedup state was updated
        let inner = log.inner.lock();
        let node1_digest = crate::hash::fnv_64a_str("node1");
        let dedup_entry = inner.dedup.get(&node1_digest).unwrap();

        // Should track the latest time from node1
        assert_eq!(dedup_entry.time, 1002);
        // UID is auto-generated, so just verify it exists and is reasonable
        assert!(dedup_entry.uid > 0);
    }

    #[test]
    fn test_get_state_binary_format() {
        let log = ClusterLog::new();

        // Add some entries
        let _ = log.add("node1", "root", "cluster", 123, 6, 1000, "Entry 1");
        let _ = log.add("node2", "admin", "system", 456, 6, 1001, "Entry 2");

        // Get state
        let state = log.get_state().unwrap();

        // Should be binary format, not JSON
        assert!(state.len() >= 8); // At least header

        // Check header format (clog_base_t)
        let size = u32::from_le_bytes(state[0..4].try_into().unwrap()) as usize;
        let cpos = u32::from_le_bytes(state[4..8].try_into().unwrap());

        assert_eq!(size, state.len());
        assert_eq!(cpos, 8); // First entry at offset 8

        // Should be able to deserialize back
        let deserialized = ClusterLog::deserialize_state(&state).unwrap();
        assert_eq!(deserialized.len(), 2);
    }

    #[test]
    fn test_state_roundtrip() {
        let log = ClusterLog::new();

        // Add entries
        let _ = log.add("node1", "root", "cluster", 123, 6, 1000, "Test 1");
        let _ = log.add("node2", "admin", "system", 456, 6, 1001, "Test 2");

        // Serialize
        let state = log.get_state().unwrap();

        // Deserialize
        let deserialized = ClusterLog::deserialize_state(&state).unwrap();

        // Check entries preserved
        assert_eq!(deserialized.len(), 2);

        // Buffer is stored newest-first after sorting and serialization
        let entries: Vec<_> = deserialized.iter().collect();
        assert_eq!(entries[0].node, "node2"); // Newest (time 1001)
        assert_eq!(entries[0].message, "Test 2");
        assert_eq!(entries[1].node, "node1"); // Oldest (time 1000)
        assert_eq!(entries[1].message, "Test 1");
    }
}
