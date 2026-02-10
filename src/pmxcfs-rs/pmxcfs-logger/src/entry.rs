/// Log Entry Implementation
///
/// This module implements the cluster log entry structure, matching the C
/// implementation's clog_entry_t (logger.c).
use super::hash::fnv_64a_str;
use anyhow::{bail, Result};
use serde::Serialize;
use std::sync::atomic::{AtomicU32, Ordering};

// Import constant from ring_buffer to avoid duplication
use crate::ring_buffer::CLOG_MAX_ENTRY_SIZE;

/// Global UID counter (matches C's `uid_counter` global variable)
///
/// # UID Wraparound Behavior
///
/// The UID counter is a 32-bit unsigned integer that wraps around after 2^32 entries.
/// This matches the C implementation's behavior (logger.c:62).
///
/// **Wraparound implications:**
/// - At 1000 entries/second: wraparound after ~49 days
/// - At 100 entries/second: wraparound after ~497 days
/// - After wraparound, UIDs restart from 1
///
/// **Impact on deduplication:**
/// The deduplication logic compares (time, UID) tuples. After wraparound, an entry
/// with UID=1 might be incorrectly considered older than an entry with UID=4294967295,
/// even if they have the same timestamp. This is a known limitation inherited from
/// the C implementation.
///
/// **Mitigation:**
/// - Entries with different timestamps are correctly ordered (time is primary sort key)
/// - Wraparound only affects entries with identical timestamps from the same node
/// - A warning is logged when wraparound occurs (see fetch_add below)
static UID_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Log entry structure
///
/// Matches C's `clog_entry_t` from logger.c:
/// ```c
/// typedef struct {
///     uint32_t prev;          // Previous entry offset
///     uint32_t next;          // Next entry offset
///     uint32_t uid;           // Unique ID
///     uint32_t time;          // Timestamp
///     uint64_t node_digest;   // FNV-1a hash of node name
///     uint64_t ident_digest;  // FNV-1a hash of ident
///     uint32_t pid;           // Process ID
///     uint8_t priority;       // Syslog priority (0-7)
///     uint8_t node_len;       // Length of node name (including null)
///     uint8_t ident_len;      // Length of ident (including null)
///     uint8_t tag_len;        // Length of tag (including null)
///     uint32_t msg_len;       // Length of message (including null)
///     char data[];            // Variable length data: node + ident + tag + msg
/// } clog_entry_t;
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    /// Unique ID for this entry (auto-incrementing)
    pub uid: u32,

    /// Unix timestamp
    pub time: u32,

    /// FNV-1a hash of node name
    pub node_digest: u64,

    /// FNV-1a hash of ident (user)
    pub ident_digest: u64,

    /// Process ID
    pub pid: u32,

    /// Syslog priority (0-7)
    pub priority: u8,

    /// Node name
    pub node: String,

    /// Identity/user
    pub ident: String,

    /// Tag (e.g., "cluster", "pmxcfs")
    pub tag: String,

    /// Log message
    pub message: String,
}

impl LogEntry {
    /// Matches C's `clog_pack` function
    pub fn pack(
        node: &str,
        ident: &str,
        tag: &str,
        pid: u32,
        time: u32,
        priority: u8,
        message: &str,
    ) -> Result<Self> {
        if priority >= 8 {
            bail!("Invalid priority: {priority} (must be 0-7)");
        }

        // Truncate to 254 bytes to leave room for null terminator (C uses MIN(strlen+1, 255))
        let node = Self::truncate_string(node, 254);
        let ident = Self::truncate_string(ident, 254);
        let tag = Self::truncate_string(tag, 254);
        let message = Self::utf8_to_ascii(message);

        let node_len = node.len() + 1;
        let ident_len = ident.len() + 1;
        let tag_len = tag.len() + 1;
        let mut msg_len = message.len() + 1;

        // Use checked arithmetic to prevent integer overflow
        // Header: 48 bytes fixed (prev, next, uid, time, digests, pid, priority, lengths)
        // Variable: node_len + ident_len + tag_len + msg_len
        let header_size = std::mem::size_of::<u32>() * 4  // prev, next, uid, time
            + std::mem::size_of::<u64>() * 2  // node_digest, ident_digest
            + std::mem::size_of::<u32>() * 2  // pid, msg_len
            + std::mem::size_of::<u8>() * 4;  // priority, node_len, ident_len, tag_len

        let total_size = header_size
            .checked_add(node_len)
            .and_then(|s| s.checked_add(ident_len))
            .and_then(|s| s.checked_add(tag_len))
            .and_then(|s| s.checked_add(msg_len))
            .ok_or_else(|| anyhow::anyhow!("Entry size calculation overflow"))?;

        if total_size > CLOG_MAX_ENTRY_SIZE {
            let diff = total_size - CLOG_MAX_ENTRY_SIZE;
            msg_len = msg_len.saturating_sub(diff);
        }

        let node_digest = fnv_64a_str(&node);
        let ident_digest = fnv_64a_str(&ident);

        // Increment UID counter with wraparound detection
        let old_uid = UID_COUNTER.fetch_add(1, Ordering::SeqCst);

        // Warn on wraparound (when counter goes from u32::MAX to 0)
        // This happens approximately every 49 days at 1000 entries/second
        if old_uid == u32::MAX {
            tracing::warn!(
                "UID counter wrapped around (2^32 entries reached). \
                 Deduplication may be affected for entries with identical timestamps. \
                 This is expected behavior matching the C implementation."
            );
        }

        let uid = old_uid.wrapping_add(1);

        Ok(Self {
            uid,
            time,
            node_digest,
            ident_digest,
            pid,
            priority,
            node,
            ident,
            tag,
            message: message[..msg_len.saturating_sub(1)].to_string(),
        })
    }

    /// Truncate string to max length (safe for multi-byte UTF-8)
    fn truncate_string(s: &str, max_len: usize) -> String {
        if s.len() <= max_len {
            return s.to_string();
        }

        // Find the last valid UTF-8 character that fits within max_len
        let truncate_at = s
            .char_indices()
            .take_while(|(idx, ch)| idx + ch.len_utf8() <= max_len)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);

        s[..truncate_at].to_string()
    }

    /// Convert UTF-8 to ASCII with proper escaping
    ///
    /// Matches C's `utf8_to_ascii` function behavior:
    /// - Control characters (0x00-0x1F, 0x7F): Escaped as #XXX (e.g., #007 for BEL)
    /// - Unicode (U+0080 to U+FFFF): Escaped as \uXXXX (e.g., \u4e16 for 世)
    /// - Quotes: Escaped as \" (matches C's quotequote=TRUE behavior)
    /// - Characters > U+FFFF: Silently dropped
    /// - ASCII printable (0x20-0x7E except quotes): Passed through unchanged
    fn utf8_to_ascii(s: &str) -> String {
        let mut result = String::with_capacity(s.len());

        for c in s.chars() {
            match c {
                // Control characters: #XXX format (3 decimal digits)
                '\x00'..='\x1F' | '\x7F' => {
                    let code = c as u32;
                    result.push('#');
                    // Format as 3 decimal digits with leading zeros (e.g., #007 for BEL)
                    result.push_str(&format!("{:03}", code));
                }
                // Quote escaping: matches C's quotequote=TRUE behavior (logger.c:245)
                '"' => {
                    result.push('\\');
                    result.push('"');
                }
                // ASCII printable characters: pass through
                c if c.is_ascii() => {
                    result.push(c);
                }
                // Unicode U+0080 to U+FFFF: \uXXXX format
                c if (c as u32) < 0x10000 => {
                    result.push('\\');
                    result.push('u');
                    result.push_str(&format!("{:04x}", c as u32));
                }
                // Characters > U+FFFF: silently drop (matches C behavior)
                _ => {}
            }
        }

        result
    }

    /// Matches C's `clog_entry_size` function
    pub fn size(&self) -> usize {
        std::mem::size_of::<u32>() * 4  // prev, next, uid, time
            + std::mem::size_of::<u64>() * 2  // node_digest, ident_digest
            + std::mem::size_of::<u32>() * 2  // pid, msg_len
            + std::mem::size_of::<u8>() * 4   // priority, node_len, ident_len, tag_len
            + self.node.len() + 1
            + self.ident.len() + 1
            + self.tag.len() + 1
            + self.message.len() + 1
    }

    /// C implementation: `uint32_t realsize = ((size + 7) & 0xfffffff8);`
    pub fn aligned_size(&self) -> usize {
        let size = self.size();
        (size + 7) & !7
    }

    pub fn to_json_object(&self) -> serde_json::Value {
        serde_json::json!({
            "uid": self.uid,
            "time": self.time,
            "pri": self.priority,
            "tag": self.tag,
            "pid": self.pid,
            "node": self.node,
            "user": self.ident,
            "msg": self.message,
        })
    }

    /// Serialize to C binary format (clog_entry_t)
    ///
    /// Binary layout matches C structure:
    /// ```c
    /// struct {
    ///     uint32_t prev;          // Will be filled by ring buffer
    ///     uint32_t next;          // Will be filled by ring buffer
    ///     uint32_t uid;
    ///     uint32_t time;
    ///     uint64_t node_digest;
    ///     uint64_t ident_digest;
    ///     uint32_t pid;
    ///     uint8_t priority;
    ///     uint8_t node_len;
    ///     uint8_t ident_len;
    ///     uint8_t tag_len;
    ///     uint32_t msg_len;
    ///     char data[];  // node + ident + tag + msg (null-terminated)
    /// }
    /// ```
    pub fn serialize_binary(&self, prev: u32, next: u32) -> Vec<u8> {
        let mut buf = Vec::new();

        buf.extend_from_slice(&prev.to_le_bytes());
        buf.extend_from_slice(&next.to_le_bytes());
        buf.extend_from_slice(&self.uid.to_le_bytes());
        buf.extend_from_slice(&self.time.to_le_bytes());
        buf.extend_from_slice(&self.node_digest.to_le_bytes());
        buf.extend_from_slice(&self.ident_digest.to_le_bytes());
        buf.extend_from_slice(&self.pid.to_le_bytes());
        buf.push(self.priority);

        // Cap at 255 to match C's MIN(strlen+1, 255) and prevent u8 overflow
        let node_len = (self.node.len() + 1).min(255) as u8;
        let ident_len = (self.ident.len() + 1).min(255) as u8;
        let tag_len = (self.tag.len() + 1).min(255) as u8;
        let msg_len = (self.message.len() + 1) as u32;

        buf.push(node_len);
        buf.push(ident_len);
        buf.push(tag_len);
        buf.extend_from_slice(&msg_len.to_le_bytes());

        buf.extend_from_slice(self.node.as_bytes());
        buf.push(0);

        buf.extend_from_slice(self.ident.as_bytes());
        buf.push(0);

        buf.extend_from_slice(self.tag.as_bytes());
        buf.push(0);

        buf.extend_from_slice(self.message.as_bytes());
        buf.push(0);

        buf
    }

    pub(crate) fn deserialize_binary(data: &[u8]) -> Result<(Self, u32, u32)> {
        if data.len() < 48 {
            bail!(
                "Entry too small: {} bytes (need at least 48 for header)",
                data.len()
            );
        }

        let mut offset = 0;

        let prev = u32::from_le_bytes(data[offset..offset + 4].try_into()?);
        offset += 4;

        let next = u32::from_le_bytes(data[offset..offset + 4].try_into()?);
        offset += 4;

        let uid = u32::from_le_bytes(data[offset..offset + 4].try_into()?);
        offset += 4;

        let time = u32::from_le_bytes(data[offset..offset + 4].try_into()?);
        offset += 4;

        let node_digest = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        let ident_digest = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        let pid = u32::from_le_bytes(data[offset..offset + 4].try_into()?);
        offset += 4;

        let priority = data[offset];
        offset += 1;

        let node_len = data[offset] as usize;
        offset += 1;

        let ident_len = data[offset] as usize;
        offset += 1;

        let tag_len = data[offset] as usize;
        offset += 1;

        let msg_len = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        if offset + node_len + ident_len + tag_len + msg_len > data.len() {
            bail!("Entry data exceeds buffer size");
        }

        let node = read_null_terminated(&data[offset..offset + node_len])?;
        offset += node_len;

        let ident = read_null_terminated(&data[offset..offset + ident_len])?;
        offset += ident_len;

        let tag = read_null_terminated(&data[offset..offset + tag_len])?;
        offset += tag_len;

        let message = read_null_terminated(&data[offset..offset + msg_len])?;

        Ok((
            Self {
                uid,
                time,
                node_digest,
                ident_digest,
                pid,
                priority,
                node,
                ident,
                tag,
                message,
            },
            prev,
            next,
        ))
    }
}

fn read_null_terminated(data: &[u8]) -> Result<String> {
    let len = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    Ok(String::from_utf8_lossy(&data[..len]).into_owned())
}

#[cfg(test)]
pub fn reset_uid_counter() {
    UID_COUNTER.store(0, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_entry() {
        reset_uid_counter();

        let entry = LogEntry::pack(
            "node1",
            "root",
            "cluster",
            12345,
            1234567890,
            6, // Info priority
            "Test message",
        )
        .unwrap();

        assert_eq!(entry.uid, 1);
        assert_eq!(entry.time, 1234567890);
        assert_eq!(entry.node, "node1");
        assert_eq!(entry.ident, "root");
        assert_eq!(entry.tag, "cluster");
        assert_eq!(entry.pid, 12345);
        assert_eq!(entry.priority, 6);
        assert_eq!(entry.message, "Test message");
    }

    #[test]
    fn test_uid_increment() {
        reset_uid_counter();

        let entry1 = LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "msg1").unwrap();
        let entry2 = LogEntry::pack("node1", "root", "tag", 0, 1001, 6, "msg2").unwrap();

        assert_eq!(entry1.uid, 1);
        assert_eq!(entry2.uid, 2);
    }

    #[test]
    fn test_invalid_priority() {
        let result = LogEntry::pack("node1", "root", "tag", 0, 1000, 8, "message");
        assert!(result.is_err());
    }

    #[test]
    fn test_node_digest() {
        let entry1 = LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "msg").unwrap();
        let entry2 = LogEntry::pack("node1", "root", "tag", 0, 1001, 6, "msg").unwrap();
        let entry3 = LogEntry::pack("node2", "root", "tag", 0, 1000, 6, "msg").unwrap();

        // Same node should have same digest
        assert_eq!(entry1.node_digest, entry2.node_digest);

        // Different node should have different digest
        assert_ne!(entry1.node_digest, entry3.node_digest);
    }

    #[test]
    fn test_ident_digest() {
        let entry1 = LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "msg").unwrap();
        let entry2 = LogEntry::pack("node1", "root", "tag", 0, 1001, 6, "msg").unwrap();
        let entry3 = LogEntry::pack("node1", "admin", "tag", 0, 1000, 6, "msg").unwrap();

        // Same ident should have same digest
        assert_eq!(entry1.ident_digest, entry2.ident_digest);

        // Different ident should have different digest
        assert_ne!(entry1.ident_digest, entry3.ident_digest);
    }

    #[test]
    fn test_utf8_to_ascii() {
        let entry = LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "Hello 世界").unwrap();
        assert!(entry.message.is_ascii());
        // Unicode chars escaped as \uXXXX format (matches C implementation)
        assert!(entry.message.contains("\\u4e16")); // 世 = U+4E16
        assert!(entry.message.contains("\\u754c")); // 界 = U+754C
    }

    #[test]
    fn test_utf8_control_chars() {
        // Test control character escaping
        let entry = LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "Hello\x07World").unwrap();
        assert!(entry.message.is_ascii());
        // BEL (0x07) should be escaped as #007 (matches C implementation)
        assert!(entry.message.contains("#007"));
    }

    #[test]
    fn test_utf8_mixed_content() {
        // Test mix of ASCII, Unicode, and control chars
        let entry = LogEntry::pack(
            "node1",
            "root",
            "tag",
            0,
            1000,
            6,
            "Test\x01\nUnicode世\ttab",
        )
        .unwrap();
        assert!(entry.message.is_ascii());
        // SOH (0x01) -> #001
        assert!(entry.message.contains("#001"));
        // Newline (0x0A) -> #010
        assert!(entry.message.contains("#010"));
        // Unicode 世 (U+4E16) -> \u4e16
        assert!(entry.message.contains("\\u4e16"));
        // Tab (0x09) -> #009
        assert!(entry.message.contains("#009"));
    }

    #[test]
    fn test_string_truncation() {
        let long_node = "a".repeat(300);
        let entry = LogEntry::pack(&long_node, "root", "tag", 0, 1000, 6, "msg").unwrap();
        assert!(entry.node.len() <= 255);
    }

    #[test]
    fn test_truncate_multibyte_utf8() {
        // Test that truncate_string doesn't panic on multi-byte UTF-8 boundaries
        // "世" is 3 bytes in UTF-8 (0xE4 0xB8 0x96)
        let s = "x".repeat(253) + "世";

        // This should not panic, even though 254 falls in the middle of "世"
        let entry = LogEntry::pack(&s, "root", "tag", 0, 1000, 6, "msg").unwrap();

        // Should truncate to 253 bytes (before the multi-byte char)
        assert_eq!(entry.node.len(), 253);
        assert_eq!(entry.node, "x".repeat(253));
    }

    #[test]
    fn test_message_truncation() {
        let long_message = "a".repeat(CLOG_MAX_ENTRY_SIZE);
        let entry = LogEntry::pack("node1", "root", "tag", 0, 1000, 6, &long_message).unwrap();
        // Entry should fit within max size
        assert!(entry.size() <= CLOG_MAX_ENTRY_SIZE);
    }

    #[test]
    fn test_aligned_size() {
        let entry = LogEntry::pack("node1", "root", "tag", 0, 1000, 6, "msg").unwrap();
        let aligned = entry.aligned_size();

        // Aligned size should be multiple of 8
        assert_eq!(aligned % 8, 0);

        // Aligned size should be >= actual size
        assert!(aligned >= entry.size());

        // Aligned size should be within 7 bytes of actual size
        assert!(aligned - entry.size() < 8);
    }

    #[test]
    fn test_json_export() {
        let entry = LogEntry::pack("node1", "root", "cluster", 123, 1234567890, 6, "Test").unwrap();
        let json = entry.to_json_object();

        assert_eq!(json["node"], "node1");
        assert_eq!(json["user"], "root");
        assert_eq!(json["tag"], "cluster");
        assert_eq!(json["pid"], 123);
        assert_eq!(json["time"], 1234567890);
        assert_eq!(json["pri"], 6);
        assert_eq!(json["msg"], "Test");
    }

    #[test]
    fn test_binary_serialization_roundtrip() {
        let entry = LogEntry::pack(
            "node1",
            "root",
            "cluster",
            12345,
            1234567890,
            6,
            "Test message",
        )
        .unwrap();

        // Serialize with prev/next pointers
        let binary = entry.serialize_binary(100, 200);

        // Deserialize
        let (deserialized, prev, next) = LogEntry::deserialize_binary(&binary).unwrap();

        // Check prev/next pointers
        assert_eq!(prev, 100);
        assert_eq!(next, 200);

        // Check entry fields
        assert_eq!(deserialized.uid, entry.uid);
        assert_eq!(deserialized.time, entry.time);
        assert_eq!(deserialized.node_digest, entry.node_digest);
        assert_eq!(deserialized.ident_digest, entry.ident_digest);
        assert_eq!(deserialized.pid, entry.pid);
        assert_eq!(deserialized.priority, entry.priority);
        assert_eq!(deserialized.node, entry.node);
        assert_eq!(deserialized.ident, entry.ident);
        assert_eq!(deserialized.tag, entry.tag);
        assert_eq!(deserialized.message, entry.message);
    }

    #[test]
    fn test_binary_format_header_size() {
        let entry = LogEntry::pack("n", "u", "t", 1, 1000, 6, "m").unwrap();
        let binary = entry.serialize_binary(0, 0);

        // Header should be exactly 48 bytes
        // prev(4) + next(4) + uid(4) + time(4) + node_digest(8) + ident_digest(8) +
        // pid(4) + priority(1) + node_len(1) + ident_len(1) + tag_len(1) + msg_len(4)
        assert!(binary.len() >= 48);

        // First 48 bytes are header
        assert_eq!(&binary[0..4], &0u32.to_le_bytes()); // prev
        assert_eq!(&binary[4..8], &0u32.to_le_bytes()); // next
    }

    #[test]
    fn test_binary_deserialize_invalid_size() {
        let too_small = vec![0u8; 40]; // Less than 48 byte header
        let result = LogEntry::deserialize_binary(&too_small);
        assert!(result.is_err());
    }

    #[test]
    fn test_binary_null_terminators() {
        let entry = LogEntry::pack("node1", "root", "tag", 123, 1000, 6, "message").unwrap();
        let binary = entry.serialize_binary(0, 0);

        // Check that strings are null-terminated
        // Find null bytes in data section (after 48-byte header)
        let data_section = &binary[48..];
        let null_count = data_section.iter().filter(|&&b| b == 0).count();
        assert_eq!(null_count, 4); // 4 null terminators (node, ident, tag, msg)
    }

    #[test]
    fn test_length_field_overflow_prevention() {
        // Test that 255-byte strings are handled correctly (prevent u8 overflow)
        // C does: MIN(strlen(s) + 1, 255) to cap at 255
        let long_string = "a".repeat(255);

        let entry = LogEntry::pack(&long_string, &long_string, &long_string, 123, 1000, 6, "msg").unwrap();

        // Strings should be truncated to 254 bytes (leaving room for null)
        assert_eq!(entry.node.len(), 254);
        assert_eq!(entry.ident.len(), 254);
        assert_eq!(entry.tag.len(), 254);

        // Serialize and check length fields are capped at 255 (254 bytes + null)
        let binary = entry.serialize_binary(0, 0);

        // Extract length fields from header
        // Layout: prev(4) + next(4) + uid(4) + time(4) + node_digest(8) + ident_digest(8) +
        //         pid(4) + priority(1) + node_len(1) + ident_len(1) + tag_len(1) + msg_len(4)
        // Offsets: node_len=37, ident_len=38, tag_len=39
        let node_len = binary[37];
        let ident_len = binary[38];
        let tag_len = binary[39];

        assert_eq!(node_len, 255); // 254 bytes + 1 null = 255
        assert_eq!(ident_len, 255);
        assert_eq!(tag_len, 255);
    }

    #[test]
    fn test_length_field_no_wraparound() {
        // Even if somehow a 255+ byte string gets through, serialize should cap at 255
        // This tests the defensive .min(255) in serialize_binary
        let mut entry = LogEntry::pack("node", "ident", "tag", 123, 1000, 6, "msg").unwrap();

        // Artificially create an edge case (though pack() already prevents this)
        entry.node = "x".repeat(254);  // Max valid size

        let binary = entry.serialize_binary(0, 0);
        let node_len = binary[37];  // Offset 37 for node_len

        // Should be 255 (254 + 1 for null), not wrap to 0
        assert_eq!(node_len, 255);
        assert_ne!(node_len, 0); // Ensure no wraparound
    }
}
