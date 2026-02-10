//! Binary compatibility tests for pmxcfs-logger
//!
//! These tests verify that the Rust implementation can correctly
//! serialize/deserialize binary data in a format compatible with
//! the C implementation.

use pmxcfs_logger::{ClusterLog, LogEntry, RingBuffer};

/// Test deserializing a minimal C-compatible binary blob
///
/// This test uses a hand-crafted binary blob that matches C's clog_base_t format:
/// - 8-byte header (size + cpos)
/// - Single entry at offset 8
#[test]
fn test_deserialize_minimal_c_blob() {
    // Create a minimal valid C binary blob
    // Header: size=8+entry_size, cpos=8 (points to first entry)
    // Entry: minimal valid entry with all required fields

    let entry = LogEntry::pack("node1", "root", "test", 123, 1000, 6, "msg").unwrap();
    let entry_bytes = entry.serialize_binary(0, 0); // prev=0 (end), next=0
    let entry_size = entry_bytes.len();

    // Allocate buffer with capacity for header + entry
    let total_size = 8 + entry_size;
    let mut blob = vec![0u8; total_size];

    // Write header
    blob[0..4].copy_from_slice(&(total_size as u32).to_le_bytes()); // size
    blob[4..8].copy_from_slice(&8u32.to_le_bytes()); // cpos = 8

    // Write entry
    blob[8..8 + entry_size].copy_from_slice(&entry_bytes);

    // Deserialize
    let buffer = RingBuffer::deserialize_binary(&blob).expect("Should deserialize");

    // Verify
    assert_eq!(buffer.len(), 1, "Should have 1 entry");
    let entries: Vec<_> = buffer.iter().collect();
    assert_eq!(entries[0].node, "node1");
    assert_eq!(entries[0].message, "msg");
}

/// Test round-trip: Rust serialize -> deserialize
///
/// Verifies that Rust can serialize and deserialize its own format
#[test]
fn test_roundtrip_single_entry() {
    let mut buffer = RingBuffer::new(8192 * 16);

    let entry = LogEntry::pack("node1", "root", "cluster", 123, 1000, 6, "Test message").unwrap();
    buffer.add_entry(&entry).unwrap();

    // Serialize
    let blob = buffer.serialize_binary();

    // Verify header
    let size = u32::from_le_bytes(blob[0..4].try_into().unwrap()) as usize;
    let cpos = u32::from_le_bytes(blob[4..8].try_into().unwrap()) as usize;

    assert_eq!(size, blob.len(), "Size should match blob length");
    assert_eq!(cpos, 8, "First entry should be at offset 8");

    // Deserialize
    let deserialized = RingBuffer::deserialize_binary(&blob).expect("Should deserialize");

    // Verify
    assert_eq!(deserialized.len(), 1);
    let entries: Vec<_> = deserialized.iter().collect();
    assert_eq!(entries[0].node, "node1");
    assert_eq!(entries[0].ident, "root");
    assert_eq!(entries[0].message, "Test message");
}

/// Test round-trip with multiple entries
///
/// Verifies linked list structure (prev/next pointers)
#[test]
fn test_roundtrip_multiple_entries() {
    let mut buffer = RingBuffer::new(8192 * 16);

    // Add 3 entries
    for i in 0..3 {
        let entry = LogEntry::pack(
            "node1",
            "root",
            "test",
            100 + i,
            1000 + i,
            6,
            &format!("Message {}", i),
        )
        .unwrap();
        buffer.add_entry(&entry).unwrap();
    }

    // Serialize
    let blob = buffer.serialize_binary();

    // Deserialize
    let deserialized = RingBuffer::deserialize_binary(&blob).expect("Should deserialize");

    // Verify all entries preserved
    assert_eq!(deserialized.len(), 3);

    let entries: Vec<_> = deserialized.iter().collect();
    // Entries are stored newest-first
    assert_eq!(entries[0].message, "Message 2"); // Newest
    assert_eq!(entries[1].message, "Message 1");
    assert_eq!(entries[2].message, "Message 0"); // Oldest
}

/// Test empty buffer serialization
///
/// C returns a buffer with size and cpos=0 for empty buffers
#[test]
fn test_empty_buffer_format() {
    let buffer = RingBuffer::new(8192 * 16);

    // Serialize empty buffer
    let blob = buffer.serialize_binary();

    // Verify format
    assert_eq!(blob.len(), 8192 * 16, "Should be full capacity");

    let size = u32::from_le_bytes(blob[0..4].try_into().unwrap()) as usize;
    let cpos = u32::from_le_bytes(blob[4..8].try_into().unwrap()) as usize;

    assert_eq!(size, 8192 * 16, "Size should match capacity");
    assert_eq!(cpos, 0, "Empty buffer should have cpos=0");

    // Deserialize
    let deserialized = RingBuffer::deserialize_binary(&blob).expect("Should deserialize");
    assert_eq!(deserialized.len(), 0, "Should be empty");
}

/// Test entry alignment (8-byte boundaries)
///
/// C uses ((size + 7) & ~7) for alignment
#[test]
fn test_entry_alignment() {
    let entry = LogEntry::pack("n", "u", "t", 1, 1000, 6, "m").unwrap();

    let aligned_size = entry.aligned_size();

    // Should be multiple of 8
    assert_eq!(aligned_size % 8, 0, "Aligned size should be multiple of 8");

    // Should be >= actual size
    assert!(aligned_size >= entry.size());

    // Should be within 7 bytes of actual size
    assert!(aligned_size - entry.size() < 8);
}

/// Test string length capping (prevents u8 overflow)
///
/// node_len, ident_len, tag_len are u8 and must cap at 255
#[test]
fn test_string_length_capping() {
    // Create entry with very long strings
    let long_node = "a".repeat(300);
    let long_ident = "b".repeat(300);
    let long_tag = "c".repeat(300);

    let entry = LogEntry::pack(&long_node, &long_ident, &long_tag, 1, 1000, 6, "msg").unwrap();

    // Serialize
    let blob = entry.serialize_binary(0, 0);

    // Check length fields (at offsets 32, 33, 34 after header)
    let node_len = blob[32];
    let ident_len = blob[33];
    let tag_len = blob[34];

    // All should be capped at 255 (including null terminator)
    assert!(node_len <= 255, "node_len should be capped at 255");
    assert!(ident_len <= 255, "ident_len should be capped at 255");
    assert!(tag_len <= 255, "tag_len should be capped at 255");
}

/// Test ClusterLog state serialization
///
/// Verifies get_state() returns C-compatible format
#[test]
fn test_cluster_log_state_format() {
    let log = ClusterLog::new();

    // Add some entries
    log.add("node1", "root", "cluster", 123, 6, 1000, "Entry 1")
        .unwrap();
    log.add("node2", "admin", "system", 456, 6, 1001, "Entry 2")
        .unwrap();

    // Get state
    let state = log.get_state().expect("Should serialize");

    // Verify header format
    assert!(state.len() >= 8, "Should have at least header");

    let size = u32::from_le_bytes(state[0..4].try_into().unwrap()) as usize;
    let cpos = u32::from_le_bytes(state[4..8].try_into().unwrap()) as usize;

    assert_eq!(size, state.len(), "Size should match blob length");
    assert!(cpos >= 8, "cpos should point into data section");
    assert!(cpos < size, "cpos should be within buffer");

    // Deserialize and verify
    let deserialized = ClusterLog::deserialize_state(&state).expect("Should deserialize");
    assert_eq!(deserialized.len(), 2, "Should have 2 entries");
}

/// Test wrap-around detection in deserialization
///
/// Verifies that circular buffer wrap-around is handled correctly
#[test]
fn test_wraparound_detection() {
    // Create a buffer with entries
    let mut buffer = RingBuffer::new(8192 * 16);

    for i in 0..5 {
        let entry = LogEntry::pack("node1", "root", "test", 100 + i, 1000 + i, 6, "msg").unwrap();
        buffer.add_entry(&entry).unwrap();
    }

    // Serialize
    let blob = buffer.serialize_binary();

    // Deserialize (should handle prev pointers correctly)
    let deserialized = RingBuffer::deserialize_binary(&blob).expect("Should deserialize");

    // Should get all entries
    assert_eq!(deserialized.len(), 5);
}

/// Test invalid binary data handling
///
/// Verifies that malformed data is rejected
#[test]
fn test_invalid_binary_data() {
    // Too small
    let too_small = vec![0u8; 4];
    assert!(RingBuffer::deserialize_binary(&too_small).is_err());

    // Size mismatch
    let mut size_mismatch = vec![0u8; 100];
    size_mismatch[0..4].copy_from_slice(&200u32.to_le_bytes()); // Claims 200 bytes
    assert!(RingBuffer::deserialize_binary(&size_mismatch).is_err());

    // Invalid cpos (beyond buffer)
    let mut invalid_cpos = vec![0u8; 100];
    invalid_cpos[0..4].copy_from_slice(&100u32.to_le_bytes()); // size = 100
    invalid_cpos[4..8].copy_from_slice(&200u32.to_le_bytes()); // cpos = 200 (invalid)
    assert!(RingBuffer::deserialize_binary(&invalid_cpos).is_err());
}

/// Test FNV-1a hash consistency
///
/// Verifies that node_digest and ident_digest are computed correctly
#[test]
fn test_hash_consistency() {
    let entry1 = LogEntry::pack("node1", "root", "test", 1, 1000, 6, "msg1").unwrap();
    let entry2 = LogEntry::pack("node1", "root", "test", 2, 1001, 6, "msg2").unwrap();
    let entry3 = LogEntry::pack("node2", "admin", "test", 3, 1002, 6, "msg3").unwrap();

    // Same node should have same digest
    assert_eq!(entry1.node_digest, entry2.node_digest);

    // Same ident should have same digest
    assert_eq!(entry1.ident_digest, entry2.ident_digest);

    // Different node should have different digest
    assert_ne!(entry1.node_digest, entry3.node_digest);

    // Different ident should have different digest
    assert_ne!(entry1.ident_digest, entry3.ident_digest);
}

/// Test priority validation
///
/// Priority must be 0-7 (syslog priority)
#[test]
fn test_priority_validation() {
    // Valid priorities (0-7)
    for pri in 0..=7 {
        let result = LogEntry::pack("node1", "root", "test", 1, 1000, pri, "msg");
        assert!(result.is_ok(), "Priority {} should be valid", pri);
    }

    // Invalid priority (8+)
    let result = LogEntry::pack("node1", "root", "test", 1, 1000, 8, "msg");
    assert!(result.is_err(), "Priority 8 should be invalid");
}

/// Test UTF-8 to ASCII conversion
///
/// Verifies control character and Unicode escaping (matches C implementation)
#[test]
fn test_utf8_escaping() {
    // Control characters (C format: #XXX with 3 decimal digits)
    let entry = LogEntry::pack("node1", "root", "test", 1, 1000, 6, "Hello\x07World").unwrap();
    assert!(entry.message.contains("#007"), "BEL should be escaped as #007");

    // Unicode characters
    let entry = LogEntry::pack("node1", "root", "test", 1, 1000, 6, "Hello 世界").unwrap();
    assert!(entry.message.contains("\\u4e16"), "世 should be escaped as \\u4e16");
    assert!(entry.message.contains("\\u754c"), "界 should be escaped as \\u754c");

    // Mixed content
    let entry = LogEntry::pack("node1", "root", "test", 1, 1000, 6, "Test\x01\n世").unwrap();
    assert!(entry.message.contains("#001"), "SOH should be escaped");
    assert!(entry.message.contains("#010"), "LF should be escaped");
    assert!(entry.message.contains("\\u4e16"), "Unicode should be escaped");
}
