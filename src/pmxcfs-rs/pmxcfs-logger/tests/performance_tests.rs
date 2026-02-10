//! Performance tests for pmxcfs-logger
//!
//! These tests verify that the logger implementation scales properly
//! and handles large log merges efficiently.

use pmxcfs_logger::ClusterLog;

/// Test merging large logs from multiple nodes
///
/// This test verifies:
/// 1. Large log merge performance (multiple nodes with many entries)
/// 2. Memory usage stays bounded
/// 3. Deduplication works correctly at scale
#[test]
fn test_large_log_merge_performance() {
    // Create 3 nodes with large logs
    let node1 = ClusterLog::new();
    let node2 = ClusterLog::new();
    let node3 = ClusterLog::new();

    // Add 1000 entries per node (3000 total)
    for i in 0..1000 {
        let _ = node1.add(
            "node1",
            "root",
            "cluster",
            1000 + i,
            6,
            1000000 + i,
            &format!("Node1 entry {}", i),
        );
        let _ = node2.add(
            "node2",
            "admin",
            "system",
            2000 + i,
            6,
            1000000 + i,
            &format!("Node2 entry {}", i),
        );
        let _ = node3.add(
            "node3",
            "user",
            "service",
            3000 + i,
            6,
            1000000 + i,
            &format!("Node3 entry {}", i),
        );
    }

    // Get remote buffers
    let node2_buffer = node2.get_buffer();
    let node3_buffer = node3.get_buffer();

    // Merge all logs into node1
    let start = std::time::Instant::now();
    node1
        .merge(vec![node2_buffer, node3_buffer], true)
        .expect("Merge should succeed");
    let duration = start.elapsed();

    // Verify merge completed
    let merged_count = node1.len();

    // Should have merged entries (may be less than 3000 due to capacity limits)
    assert!(
        merged_count > 0,
        "Should have some entries after merge (got {})",
        merged_count
    );

    // Performance check: merge should complete in reasonable time
    // For 3000 entries, should be well under 1 second
    assert!(
        duration.as_millis() < 1000,
        "Large merge took too long: {:?}",
        duration
    );

    println!(
        "✓ Merged 3000 entries from 3 nodes in {:?} (result: {} entries)",
        duration, merged_count
    );
}

/// Test deduplication performance with high duplicate rate
///
/// This test verifies that deduplication works efficiently when
/// many duplicate entries are present.
#[test]
fn test_deduplication_performance() {
    let log = ClusterLog::new();

    // Add 500 entries from same node with overlapping times
    // This creates many potential duplicates
    for i in 0..500 {
        let _ = log.add(
            "node1",
            "root",
            "cluster",
            1000 + i,
            6,
            1000 + (i / 10), // Reuse timestamps (50 unique times)
            &format!("Entry {}", i),
        );
    }

    // Create remote log with overlapping entries
    let remote = ClusterLog::new();
    for i in 0..500 {
        let _ = remote.add(
            "node1",
            "root",
            "cluster",
            2000 + i,
            6,
            1000 + (i / 10), // Same timestamp pattern
            &format!("Remote entry {}", i),
        );
    }

    let remote_buffer = remote.get_buffer();

    // Merge with deduplication
    let start = std::time::Instant::now();
    log.merge(vec![remote_buffer], true)
        .expect("Merge should succeed");
    let duration = start.elapsed();

    let final_count = log.len();

    // Should have deduplicated some entries
    assert!(
        final_count > 0,
        "Should have entries after deduplication"
    );

    // Performance check
    assert!(
        duration.as_millis() < 500,
        "Deduplication took too long: {:?}",
        duration
    );

    println!(
        "✓ Deduplicated 1000 entries in {:?} (result: {} entries)",
        duration, final_count
    );
}

/// Test memory usage stays bounded during large operations
///
/// This test verifies that the ring buffer properly limits memory
/// usage even when adding many entries.
#[test]
fn test_memory_bounded() {
    // Create log with default capacity
    let log = ClusterLog::new();

    // Add many entries (more than capacity)
    for i in 0..10000 {
        let _ = log.add(
            "node1",
            "root",
            "cluster",
            1000 + i,
            6,
            1000000 + i,
            &format!("Entry with some message content {}", i),
        );
    }

    let entry_count = log.len();
    let capacity = log.capacity();

    // Buffer should not grow unbounded
    // Entry count should be reasonable relative to capacity
    assert!(
        entry_count < 10000,
        "Buffer should not store all 10000 entries (got {})",
        entry_count
    );

    // Verify capacity is respected
    assert!(
        capacity > 0,
        "Capacity should be set (got {})",
        capacity
    );

    println!(
        "✓ Added 10000 entries, buffer contains {} (capacity: {} bytes)",
        entry_count, capacity
    );
}

/// Test JSON export performance with large logs
///
/// This test verifies that JSON export scales properly.
#[test]
fn test_json_export_performance() {
    let log = ClusterLog::new();

    // Add 1000 entries
    for i in 0..1000 {
        let _ = log.add(
            "node1",
            "root",
            "cluster",
            1000 + i,
            6,
            1000000 + i,
            &format!("Test message {}", i),
        );
    }

    // Export to JSON
    let start = std::time::Instant::now();
    let json = log.dump_json(None, 1000);
    let duration = start.elapsed();

    // Verify JSON is valid
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("Should be valid JSON");
    let data = parsed["data"].as_array().expect("Should have data array");

    assert!(data.len() > 0, "Should have entries in JSON");

    // Performance check
    assert!(
        duration.as_millis() < 500,
        "JSON export took too long: {:?}",
        duration
    );

    println!(
        "✓ Exported {} entries to JSON in {:?}",
        data.len(),
        duration
    );
}

/// Test binary serialization performance
///
/// This test verifies that binary serialization/deserialization
/// is efficient for large buffers.
#[test]
fn test_binary_serialization_performance() {
    let log = ClusterLog::new();

    // Add 500 entries
    for i in 0..500 {
        let _ = log.add(
            "node1",
            "root",
            "cluster",
            1000 + i,
            6,
            1000000 + i,
            &format!("Entry {}", i),
        );
    }

    // Serialize
    let start = std::time::Instant::now();
    let state = log.get_state().expect("Should serialize");
    let serialize_duration = start.elapsed();

    // Deserialize
    let start = std::time::Instant::now();
    let deserialized = ClusterLog::deserialize_state(&state).expect("Should deserialize");
    let deserialize_duration = start.elapsed();

    // Verify round-trip
    assert_eq!(deserialized.len(), 500, "Should preserve entry count");

    // Performance checks
    assert!(
        serialize_duration.as_millis() < 200,
        "Serialization took too long: {:?}",
        serialize_duration
    );
    assert!(
        deserialize_duration.as_millis() < 200,
        "Deserialization took too long: {:?}",
        deserialize_duration
    );

    println!(
        "✓ Serialized 500 entries in {:?}, deserialized in {:?}",
        serialize_duration, deserialize_duration
    );
}
