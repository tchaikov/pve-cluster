// Local integration tests that don't require containers
// Tests for MemDb functionality and basic plugin integration

mod common;

use anyhow::Result;
use pmxcfs_memdb::MemDb;
use pmxcfs_rs::plugins;

use common::*;

/// Test basic MemDb CRUD operations
#[test]
fn test_memdb_create_read_write() -> Result<()> {
    let (_temp_dir, memdb) = create_minimal_test_db()?;

    // Create a file
    memdb.create("/test-file.txt", libc::S_IFREG, 0, TEST_MTIME)?;

    // Write content
    let content = b"Hello, World!";
    memdb.write("/test-file.txt", 0, 0, TEST_MTIME, content, false)?;

    // Read it back
    let data = memdb.read("/test-file.txt", 0, 1024)?;
    assert_eq!(data, content, "File content should match");

    Ok(())
}

/// Test directory operations
#[test]
fn test_memdb_directories() -> Result<()> {
    let (_temp_dir, memdb) = create_minimal_test_db()?;

    // Create directory structure
    memdb.create("/nodes", libc::S_IFDIR, 0, TEST_MTIME)?;
    memdb.create("/nodes/testnode", libc::S_IFDIR, 0, TEST_MTIME)?;
    memdb.create("/nodes/testnode/qemu-server", libc::S_IFDIR, 0, TEST_MTIME)?;

    // List directory
    let entries = memdb.readdir("/nodes/testnode")?;
    assert_eq!(entries.len(), 1, "Should have 1 entry");
    assert_eq!(entries[0].name, "qemu-server");

    // Verify directory exists
    assert!(memdb.exists("/nodes")?);
    assert!(memdb.exists("/nodes/testnode")?);
    assert!(memdb.exists("/nodes/testnode/qemu-server")?);

    Ok(())
}

/// Test file operations: rename and delete
#[test]
fn test_memdb_file_operations() -> Result<()> {
    let (_temp_dir, memdb) = create_minimal_test_db()?;

    // Create and write file
    memdb.create("/old-name.txt", libc::S_IFREG, 0, TEST_MTIME)?;
    memdb.write("/old-name.txt", 0, 0, TEST_MTIME, b"test", false)?;

    // Test rename
    memdb.rename("/old-name.txt", "/new-name.txt", 0, 1000)?;
    assert!(!memdb.exists("/old-name.txt")?, "Old name should not exist");
    assert!(memdb.exists("/new-name.txt")?, "New name should exist");

    // Verify content survived rename
    let data = memdb.read("/new-name.txt", 0, 1024)?;
    assert_eq!(data, b"test");

    // Test delete
    memdb.delete("/new-name.txt", 0, 1000)?;
    assert!(!memdb.exists("/new-name.txt")?, "File should be deleted");

    Ok(())
}

/// Test database persistence across reopens
#[test]
fn test_memdb_persistence() -> Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let db_path = temp_dir.path().join("persist.db");

    // Create and populate database
    {
        let memdb = MemDb::open(&db_path, true)?;
        memdb.create("/persistent.txt", libc::S_IFREG, 0, TEST_MTIME)?;
        memdb.write("/persistent.txt", 0, 0, TEST_MTIME, b"persistent data", false)?;
    }

    // Reopen database and verify data persists
    {
        let memdb = MemDb::open(&db_path, false)?;
        let data = memdb.read("/persistent.txt", 0, 1024)?;
        assert_eq!(
            data, b"persistent data",
            "Data should persist across reopens"
        );
    }

    Ok(())
}

/// Test write with offset (partial write/append)
#[test]
fn test_memdb_write_offset() -> Result<()> {
    let (_temp_dir, memdb) = create_minimal_test_db()?;

    memdb.create("/offset-test.txt", libc::S_IFREG, 0, TEST_MTIME)?;

    // Write at offset 0
    memdb.write("/offset-test.txt", 0, 0, TEST_MTIME, b"Hello", false)?;

    // Write at offset 5 (append)
    memdb.write("/offset-test.txt", 5, 0, TEST_MTIME, b", World!", false)?;

    // Read full content
    let data = memdb.read("/offset-test.txt", 0, 1024)?;
    assert_eq!(data, b"Hello, World!");

    Ok(())
}

/// Test write with truncation
///
/// Now tests CORRECT behavior after fixing the API bug.
/// truncate=true should clear the file before writing.
#[test]
fn test_memdb_write_truncate() -> Result<()> {
    let (_temp_dir, memdb) = create_minimal_test_db()?;

    memdb.create("/truncate-test.txt", libc::S_IFREG, 0, TEST_MTIME)?;

    // Write initial content
    memdb.write("/truncate-test.txt", 0, 0, TEST_MTIME, b"Hello, World!", false)?;

    // Overwrite with truncate=true (should clear first, then write)
    memdb.write("/truncate-test.txt", 0, 0, TEST_MTIME, b"Hi", true)?;

    // Should only have "Hi"
    let data = memdb.read("/truncate-test.txt", 0, 1024)?;
    assert_eq!(data, b"Hi", "Truncate should clear file before writing");

    Ok(())
}

/// Test file size limit (C implementation limits to 1MB)
#[test]
fn test_memdb_file_size_limit() -> Result<()> {
    let (_temp_dir, memdb) = create_minimal_test_db()?;

    memdb.create("/large.bin", libc::S_IFREG, 0, TEST_MTIME)?;

    // Exactly 1MB should be accepted
    let one_mb = vec![0u8; 1024 * 1024];
    assert!(
        memdb
            .write("/large.bin", 0, 0, TEST_MTIME, &one_mb, false)
            .is_ok(),
        "1MB file should be accepted"
    );

    // Over 1MB should fail
    let over_one_mb = vec![0u8; 1024 * 1024 + 1];
    assert!(
        memdb
            .write("/large.bin", 0, 0, TEST_MTIME, &over_one_mb, false)
            .is_err(),
        "Over 1MB file should be rejected"
    );

    Ok(())
}

/// Test plugin initialization and basic functionality
#[test]
fn test_plugin_initialization() -> Result<()> {
    let config = create_test_config(true);
    let status = create_test_status();

    let plugin_registry = plugins::init_plugins(config, status);

    // Verify plugins are registered
    let plugin_list = plugin_registry.list();
    assert!(!plugin_list.is_empty(), "Should have plugins registered");

    // Verify expected plugins exist
    assert!(
        plugin_registry.get(".version").is_some(),
        "Should have .version plugin"
    );
    assert!(
        plugin_registry.get(".vmlist").is_some(),
        "Should have .vmlist plugin"
    );
    assert!(
        plugin_registry.get(".rrd").is_some(),
        "Should have .rrd plugin"
    );
    assert!(
        plugin_registry.get(".members").is_some(),
        "Should have .members plugin"
    );
    assert!(
        plugin_registry.get(".clusterlog").is_some(),
        "Should have .clusterlog plugin"
    );

    Ok(())
}

/// Test .version plugin output
#[test]
fn test_version_plugin() -> Result<()> {
    let config = create_test_config(true);
    let status = create_test_status();
    let plugins = plugins::init_plugins(config, status);

    let version_plugin = plugins
        .get(".version")
        .expect(".version plugin should exist");

    let version_data = version_plugin.read()?;
    let version_str = String::from_utf8_lossy(&version_data);

    // Verify it's valid JSON
    let version_json: serde_json::Value = serde_json::from_slice(&version_data)?;
    assert!(version_json.is_object(), "Version should be JSON object");

    // Verify it contains expected fields
    assert!(
        version_str.contains("version"),
        "Should contain 'version' field"
    );

    Ok(())
}

/// Test error case: reading non-existent file
#[test]
fn test_memdb_error_nonexistent_file() {
    let (_temp_dir, memdb) = create_minimal_test_db().unwrap();

    let result = memdb.read("/does-not-exist.txt", 0, 1024);
    assert!(result.is_err(), "Reading non-existent file should fail");
}

/// Test error case: creating file in non-existent directory
#[test]
fn test_memdb_error_no_parent_directory() {
    let (_temp_dir, memdb) = create_minimal_test_db().unwrap();

    let result = memdb.create("/nonexistent/file.txt", libc::S_IFREG, 0, TEST_MTIME);
    assert!(
        result.is_err(),
        "Creating file in non-existent directory should fail"
    );
}

/// Test error case: writing to non-existent file
#[test]
fn test_memdb_error_write_nonexistent() {
    let (_temp_dir, memdb) = create_minimal_test_db().unwrap();

    let result = memdb.write("/does-not-exist.txt", 0, 0, TEST_MTIME, b"test", false);
    assert!(result.is_err(), "Writing to non-existent file should fail");
}

/// Test error case: deleting non-existent file
#[test]
fn test_memdb_error_delete_nonexistent() {
    let (_temp_dir, memdb) = create_minimal_test_db().unwrap();

    let result = memdb.delete("/does-not-exist.txt", 0, 1000);
    assert!(result.is_err(), "Deleting non-existent file should fail");
}
