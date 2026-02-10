/// Ring Buffer Implementation for Cluster Log
///
/// This module implements a circular buffer for storing log entries,
/// matching the C implementation's clog_base_t structure.
use super::entry::LogEntry;
use super::hash::fnv_64a_str;
use anyhow::{bail, Result};
use std::collections::VecDeque;

/// Matches C's CLOG_DEFAULT_SIZE constant
pub(crate) const CLOG_DEFAULT_SIZE: usize = 8192 * 16; // 131,072 bytes (128 KB)

/// Matches C's CLOG_MAX_ENTRY_SIZE constant
pub(crate) const CLOG_MAX_ENTRY_SIZE: usize = 4096; // 4,096 bytes (4 KB)

/// Ring buffer for log entries
///
/// This is a simplified Rust version of the C implementation's ring buffer.
/// The C version uses a raw byte buffer with manual pointer arithmetic,
/// but we use a VecDeque for safety and simplicity while maintaining
/// the same conceptual behavior.
///
/// C structure (clog_base_t):
/// ```c
/// struct clog_base {
///     uint32_t size;    // Total buffer size
///     uint32_t cpos;    // Current position
///     char data[];      // Variable length data
/// };
/// ```
#[derive(Debug, Clone)]
pub struct RingBuffer {
    /// Maximum capacity in bytes
    capacity: usize,

    /// Current size in bytes (approximate)
    current_size: usize,

    /// Entries stored in the buffer (newest first)
    /// We use VecDeque for efficient push/pop at both ends
    entries: VecDeque<LogEntry>,
}

impl RingBuffer {
    /// Create a new ring buffer with specified capacity
    pub fn new(capacity: usize) -> Self {
        // Ensure minimum capacity
        let capacity = if capacity < CLOG_MAX_ENTRY_SIZE * 10 {
            CLOG_DEFAULT_SIZE
        } else {
            capacity
        };

        Self {
            capacity,
            current_size: 0,
            entries: VecDeque::new(),
        }
    }

    /// Add an entry to the buffer
    ///
    /// Matches C's `clog_copy` function which calls `clog_alloc_entry`
    /// to allocate space in the ring buffer.
    pub fn add_entry(&mut self, entry: &LogEntry) -> Result<()> {
        let entry_size = entry.aligned_size();

        // Make room if needed (remove oldest entries)
        while self.current_size + entry_size > self.capacity && !self.entries.is_empty() {
            if let Some(old_entry) = self.entries.pop_back() {
                self.current_size = self.current_size.saturating_sub(old_entry.aligned_size());
            }
        }

        // Add new entry at the front (newest first)
        self.entries.push_front(entry.clone());
        self.current_size += entry_size;

        Ok(())
    }

    /// Check if buffer is near full (>90% capacity)
    pub fn is_near_full(&self) -> bool {
        self.current_size > (self.capacity * 9 / 10)
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get number of entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Get buffer capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Iterate over entries (newest first)
    pub fn iter(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    /// Sort entries by time, node_digest, and uid
    ///
    /// Matches C's `clog_sort` function
    ///
    /// C uses GTree with custom comparison function `clog_entry_sort_fn`:
    /// ```c
    /// if (entry1->time != entry2->time) {
    ///     return entry1->time - entry2->time;
    /// }
    /// if (entry1->node_digest != entry2->node_digest) {
    ///     return entry1->node_digest - entry2->node_digest;
    /// }
    /// return entry1->uid - entry2->uid;
    /// ```
    pub fn sort(&self) -> Result<Self> {
        let mut new_buffer = Self::new(self.capacity);

        // Collect and sort entries
        let mut sorted: Vec<LogEntry> = self.entries.iter().cloned().collect();

        // Sort by time (ascending), then node_digest, then uid
        sorted.sort_by_key(|e| (e.time, e.node_digest, e.uid));

        // Add sorted entries to new buffer
        // Since add_entry pushes to front, we add in forward order to get newest-first
        // sorted = [oldest...newest], add_entry pushes to front, so:
        // - Add oldest: [oldest]
        // - Add next: [next, oldest]
        // - Add newest: [newest, next, oldest]
        for entry in sorted.iter() {
            new_buffer.add_entry(entry)?;
        }

        Ok(new_buffer)
    }

    /// Dump buffer to JSON format
    ///
    /// Matches C's `clog_dump_json` function
    ///
    /// # Arguments
    /// * `ident_filter` - Optional ident filter (user filter)
    /// * `max_entries` - Maximum number of entries to include
    pub fn dump_json(&self, ident_filter: Option<&str>, max_entries: usize) -> String {
        // Compute ident digest if filter is provided
        let ident_digest = ident_filter.map(fnv_64a_str);

        let mut data = Vec::new();
        let mut count = 0;

        // Iterate over entries (newest first, matching C's walk from cpos->prev)
        for entry in self.iter() {
            if count >= max_entries {
                break;
            }

            // Apply ident filter if specified
            if let Some(digest) = ident_digest {
                if digest != entry.ident_digest {
                    continue;
                }
            }

            data.push(entry.to_json_object());
            count += 1;
        }

        let result = serde_json::json!({
            "data": data
        });

        serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
    }

    /// Dump buffer contents (for debugging)
    ///
    /// Matches C's `clog_dump` function
    #[allow(dead_code)]
    pub fn dump(&self) {
        for (idx, entry) in self.entries.iter().enumerate() {
            println!(
                "[{}] uid={:08x} time={} node={}{{{:016X}}} tag={}[{}{{{:016X}}}]: {}",
                idx,
                entry.uid,
                entry.time,
                entry.node,
                entry.node_digest,
                entry.tag,
                entry.ident,
                entry.ident_digest,
                entry.message
            );
        }
    }

    /// Serialize to C binary format (clog_base_t)
    ///
    /// Returns a full memory dump of the ring buffer matching C's format.
    /// C's clusterlog_get_state() returns g_memdup2(cl->base, clog->size),
    /// which is the entire allocated buffer capacity, not just used space.
    ///
    /// Binary layout matches C structure:
    /// ```c
    /// struct clog_base {
    ///     uint32_t size;    // Total allocated buffer capacity
    ///     uint32_t cpos;    // Offset to newest entry (not always 8!)
    ///     char data[];      // Ring buffer data (entries at various offsets)
    /// };
    /// ```
    ///
    /// Entry offsets and linkage:
    /// - entry.prev: offset to previous (older) entry
    /// - entry.next: end offset of THIS entry (offset + aligned_size), NOT pointer to next entry!
    pub fn serialize_binary(&self) -> Vec<u8> {
        // Allocate full buffer capacity (matching C's g_malloc0(size))
        let mut buf = vec![0u8; self.capacity];

        // Empty buffer case
        if self.entries.is_empty() {
            buf[0..4].copy_from_slice(&(self.capacity as u32).to_le_bytes()); // size
            buf[4..8].copy_from_slice(&0u32.to_le_bytes()); // cpos = 0 (empty)
            return buf;
        }

        // Calculate all offsets first
        let mut offsets = Vec::with_capacity(self.entries.len());
        let mut current_offset = 8usize;

        for entry in self.iter() {
            let aligned_size = entry.aligned_size();

            // Check if we have space
            if current_offset + aligned_size > self.capacity {
                break;
            }

            offsets.push(current_offset as u32);
            current_offset += aligned_size;
        }

        // Track where newest entry is (first entry at offset 8)
        let newest_offset = 8u32;

        // Write entries with correct prev/next pointers
        // Entries are in newest-first order: [newest, second-newest, ..., oldest]
        for (i, entry) in self.iter().enumerate() {
            let offset = offsets[i] as usize;
            let aligned_size = entry.aligned_size();

            // entry.prev points to the next-older entry (or 0 if this is oldest)
            let prev = if i + 1 < offsets.len() {
                offsets[i + 1]
            } else {
                0 // Oldest entry has prev = 0
            };

            // entry.next is the end offset of THIS entry
            let next = offset as u32 + aligned_size as u32;

            let entry_bytes = entry.serialize_binary(prev, next);

            // Write entry data
            buf[offset..offset + entry_bytes.len()].copy_from_slice(&entry_bytes);

            // Padding is already zeroed in vec![0u8; capacity]
        }

        // Write header
        buf[0..4].copy_from_slice(&(self.capacity as u32).to_le_bytes()); // size = full capacity
        buf[4..8].copy_from_slice(&newest_offset.to_le_bytes()); // cpos = offset to newest entry

        buf
    }

    /// Deserialize from C binary format
    ///
    /// Parses clog_base_t structure and extracts all entries.
    /// Includes wrap-around guards matching C's logic in `clog_dump`, `clog_dump_json`,
    /// and `clog_sort` functions.
    pub fn deserialize_binary(data: &[u8]) -> Result<Self> {
        if data.len() < 8 {
            bail!(
                "Buffer too small: {} bytes (need at least 8 for header)",
                data.len()
            );
        }

        // Read header
        let size = u32::from_le_bytes(data[0..4].try_into()?) as usize;
        let initial_cpos = u32::from_le_bytes(data[4..8].try_into()?) as usize;

        if size != data.len() {
            bail!(
                "Size mismatch: header says {}, got {} bytes",
                size,
                data.len()
            );
        }

        // Empty buffer (cpos == 0)
        if initial_cpos == 0 {
            return Ok(Self::new(size));
        }

        // Validate cpos range
        if initial_cpos < 8 || initial_cpos >= size {
            bail!("Invalid cpos: {initial_cpos} (size: {size})");
        }

        // Parse entries starting from cpos, walking backwards via prev pointers
        // Apply C's wrap-around guards from `clog_dump` and `clog_dump_json`
        let mut entries = VecDeque::new();
        let mut current_pos = initial_cpos;
        let mut visited = std::collections::HashSet::new();

        loop {
            // Guard against infinite loops
            if !visited.insert(current_pos) {
                break; // Already visited this position
            }

            // C guard: cpos must be non-zero
            if current_pos == 0 {
                break;
            }

            // Validate bounds
            if current_pos >= size {
                break;
            }

            // Parse entry at current_pos
            let entry_data = &data[current_pos..];
            let (entry, prev, _next) = LogEntry::deserialize_binary(entry_data)?;

            // Add to back (we're walking backwards in time, newest to oldest)
            // VecDeque should end up as [newest, ..., oldest]
            entries.push_back(entry);

            // C wrap-around guard: if (cpos < cur->prev && cur->prev <= clog->cpos) break;
            // Detects when prev wraps around past initial position
            if current_pos < prev as usize && prev as usize <= initial_cpos {
                break;
            }

            current_pos = prev as usize;
        }

        // Create ring buffer with entries
        let mut buffer = Self::new(size);
        buffer.entries = entries;

        // Recalculate current_size
        buffer.current_size = buffer
            .entries
            .iter()
            .map(|e| e.aligned_size())
            .sum();

        Ok(buffer)
    }
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new(CLOG_DEFAULT_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_creation() {
        let buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);
        assert_eq!(buffer.capacity, CLOG_DEFAULT_SIZE);
        assert_eq!(buffer.len(), 0);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_add_entry() {
        let mut buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);
        let entry = LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "message").unwrap();

        let result = buffer.add_entry(&entry);
        assert!(result.is_ok());
        assert_eq!(buffer.len(), 1);
        assert!(!buffer.is_empty());
    }

    #[test]
    fn test_ring_buffer_wraparound() {
        // Create a buffer with minimum required size (CLOG_MAX_ENTRY_SIZE * 10)
        // but fill it beyond 90% to trigger wraparound
        let mut buffer = RingBuffer::new(CLOG_MAX_ENTRY_SIZE * 10);

        // Add many small entries to fill the buffer
        // Each entry is small, so we need many to fill the buffer
        let initial_count = 50_usize;
        for i in 0..initial_count {
            let entry =
                LogEntry::pack("node1", "root", "tag", 0, 1000 + i as u32, 6, "msg").unwrap();
            let _ = buffer.add_entry(&entry);
        }

        // All entries should fit initially
        let count_before = buffer.len();
        assert_eq!(count_before, initial_count);

        // Now add entries with large messages to trigger wraparound
        // Make messages large enough to fill the buffer beyond capacity
        let large_msg = "x".repeat(7000); // Very large message (close to max)
        let large_entries_count = 20_usize;
        for i in 0..large_entries_count {
            let entry =
                LogEntry::pack("node1", "root", "tag", 0, 2000 + i as u32, 6, &large_msg).unwrap();
            let _ = buffer.add_entry(&entry);
        }

        // Should have removed some old entries due to capacity limits
        assert!(
            buffer.len() < count_before + large_entries_count,
            "Expected wraparound to remove old entries (have {} entries, expected < {})",
            buffer.len(),
            count_before + large_entries_count
        );

        // Newest entry should be present
        let newest = buffer.iter().next().unwrap();
        assert_eq!(newest.time, 2000 + large_entries_count as u32 - 1); // Last added entry
    }

    #[test]
    fn test_sort_by_time() {
        let mut buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);

        // Add entries in random time order
        let _ = buffer.add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1002, 6, "c").unwrap());
        let _ = buffer.add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "a").unwrap());
        let _ = buffer.add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1001, 6, "b").unwrap());

        let sorted = buffer.sort().unwrap();

        // Check that entries are sorted by time (oldest first after reversing)
        let times: Vec<u32> = sorted.iter().map(|e| e.time).collect();
        let mut times_sorted = times.clone();
        times_sorted.sort();
        times_sorted.reverse(); // Newest first in buffer
        assert_eq!(times, times_sorted);
    }

    #[test]
    fn test_sort_by_node_digest() {
        let mut buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);

        // Add entries with same time but different nodes
        let _ = buffer.add_entry(&LogEntry::pack("node3", "root", "tag", 0, 1000, 6, "c").unwrap());
        let _ = buffer.add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "a").unwrap());
        let _ = buffer.add_entry(&LogEntry::pack("node2", "root", "tag", 0, 1000, 6, "b").unwrap());

        let sorted = buffer.sort().unwrap();

        // Entries with same time should be sorted by node_digest
        // Within same time, should be sorted
        for entries in sorted.iter().collect::<Vec<_>>().windows(2) {
            if entries[0].time == entries[1].time {
                assert!(entries[0].node_digest >= entries[1].node_digest);
            }
        }
    }

    #[test]
    fn test_json_dump() {
        let mut buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);
        let _ = buffer
            .add_entry(&LogEntry::pack("node1", "root", "cluster", 123, 1000, 6, "msg").unwrap());

        let json = buffer.dump_json(None, 50);

        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("data").is_some());

        let data = parsed["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);

        let entry = &data[0];
        assert_eq!(entry["node"], "node1");
        assert_eq!(entry["user"], "root");
        assert_eq!(entry["tag"], "cluster");
    }

    #[test]
    fn test_json_dump_with_filter() {
        let mut buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);

        // Add entries with different users
        let _ =
            buffer.add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "msg1").unwrap());
        let _ =
            buffer.add_entry(&LogEntry::pack("node1", "admin", "tag", 0, 1001, 6, "msg2").unwrap());
        let _ =
            buffer.add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1002, 6, "msg3").unwrap());

        // Filter for "root" only
        let json = buffer.dump_json(Some("root"), 50);

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let data = parsed["data"].as_array().unwrap();

        // Should only have 2 entries (the ones from "root")
        assert_eq!(data.len(), 2);

        for entry in data {
            assert_eq!(entry["user"], "root");
        }
    }

    #[test]
    fn test_json_dump_max_entries() {
        let mut buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);

        // Add 10 entries
        for i in 0..10 {
            let _ = buffer
                .add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1000 + i, 6, "msg").unwrap());
        }

        // Request only 5 entries
        let json = buffer.dump_json(None, 5);

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let data = parsed["data"].as_array().unwrap();

        assert_eq!(data.len(), 5);
    }

    #[test]
    fn test_iterator() {
        let mut buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);

        let _ = buffer.add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "a").unwrap());
        let _ = buffer.add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1001, 6, "b").unwrap());
        let _ = buffer.add_entry(&LogEntry::pack("node1", "root", "tag", 0, 1002, 6, "c").unwrap());

        let messages: Vec<String> = buffer.iter().map(|e| e.message.clone()).collect();

        // Should be in reverse order (newest first)
        assert_eq!(messages, vec!["c", "b", "a"]);
    }

    #[test]
    fn test_binary_serialization_roundtrip() {
        let mut buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);

        let _ = buffer.add_entry(
            &LogEntry::pack("node1", "root", "cluster", 123, 1000, 6, "Entry 1").unwrap(),
        );
        let _ = buffer.add_entry(
            &LogEntry::pack("node2", "admin", "system", 456, 1001, 5, "Entry 2").unwrap(),
        );

        // Serialize
        let binary = buffer.serialize_binary();

        // Deserialize
        let deserialized = RingBuffer::deserialize_binary(&binary).unwrap();

        // Check entry count
        assert_eq!(deserialized.len(), buffer.len());

        // Check entries match
        let orig_entries: Vec<_> = buffer.iter().collect();
        let deser_entries: Vec<_> = deserialized.iter().collect();

        for (orig, deser) in orig_entries.iter().zip(deser_entries.iter()) {
            assert_eq!(deser.uid, orig.uid);
            assert_eq!(deser.time, orig.time);
            assert_eq!(deser.node, orig.node);
            assert_eq!(deser.message, orig.message);
        }
    }

    #[test]
    fn test_binary_format_header() {
        let mut buffer = RingBuffer::new(CLOG_DEFAULT_SIZE);
        let _ = buffer.add_entry(&LogEntry::pack("n", "u", "t", 1, 1000, 6, "m").unwrap());

        let binary = buffer.serialize_binary();

        // Check header format
        assert!(binary.len() >= 8);

        let size = u32::from_le_bytes(binary[0..4].try_into().unwrap()) as usize;
        let cpos = u32::from_le_bytes(binary[4..8].try_into().unwrap());

        assert_eq!(size, binary.len());
        assert_eq!(cpos, 8); // First entry at offset 8
    }

    #[test]
    fn test_binary_empty_buffer() {
        let buffer = RingBuffer::new(CLOG_DEFAULT_SIZE); // Use default size to avoid capacity upgrade
        let binary = buffer.serialize_binary();

        // Empty buffer returns full capacity (matching C's g_memdup2(cl->base, clog->size))
        assert_eq!(binary.len(), CLOG_DEFAULT_SIZE); // Full capacity, not just header!

        // Check header
        let size = u32::from_le_bytes(binary[0..4].try_into().unwrap()) as usize;
        let cpos = u32::from_le_bytes(binary[4..8].try_into().unwrap());

        assert_eq!(size, CLOG_DEFAULT_SIZE);
        assert_eq!(cpos, 0); // Empty buffer has cpos = 0

        let deserialized = RingBuffer::deserialize_binary(&binary).unwrap();
        assert_eq!(deserialized.len(), 0);
        assert_eq!(deserialized.capacity(), CLOG_DEFAULT_SIZE);
    }
}
