// Local integration tests that don't require containers
// Run with: cargo test --test local_integration

use std::sync::Arc;
use tempfile::TempDir;

use pmxcfs_rs::{config::Config, memdb::MemDb, plugins};

#[test]
fn test_memdb_plugin_integration() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Open database
    let memdb = MemDb::open(&db_path, true).expect("Failed to create memdb");

    // Create config
    let config = Arc::new(Config::new(
        "testnode".to_string(),
        "127.0.0.1".to_string(),
        33, // www-data gid
        false,
        true, // local mode
        "test-cluster".to_string(),
    ));

    // Initialize plugins
    let _plugin_registry = plugins::init_plugins(&config.nodename);

    // Test 1: Create a file in memdb
    println!("Test 1: Creating file in memdb");
    let mtime = 1234567890;
    memdb
        .create("/test-file.txt", libc::S_IFREG as u32, mtime)
        .expect("Failed to create file");

    let content = b"Hello, World!";
    memdb
        .write("/test-file.txt", 0, mtime, content, 0, true)
        .expect("Failed to write file");

    // Test 2: Read file back
    println!("Test 2: Reading file from memdb");
    let data = memdb
        .read("/test-file.txt", 0, 1024)
        .expect("Failed to read file");
    assert_eq!(data, content, "File content mismatch");

    // Test 3: Create directory structure
    println!("Test 3: Creating directory structure");
    memdb
        .create("/nodes", libc::S_IFDIR as u32, mtime)
        .expect("Failed to create nodes dir");
    memdb
        .create("/nodes/testnode", libc::S_IFDIR as u32, mtime)
        .expect("Failed to create testnode dir");
    memdb
        .create("/nodes/testnode/qemu-server", libc::S_IFDIR as u32, mtime)
        .expect("Failed to create qemu-server dir");

    // Test 4: List directory
    println!("Test 4: Listing directory");
    let entries = memdb
        .readdir("/nodes/testnode")
        .expect("Failed to read directory");
    assert_eq!(entries.len(), 1, "Should have 1 entry");
    assert_eq!(entries[0].name, "qemu-server");

    // Test 5: Test plugin versions
    let initial_version = plugins::create_version_info();
    assert!(
        initial_version.contains("\"version\""),
        "Version info should be JSON"
    );

    plugins::version_increment_path("qemu-server");
    let new_version = plugins::create_version_info();
    assert_ne!(initial_version, new_version, "Version should have changed");

}

#[test]
fn test_database_persistence() {
    // Test that data persists across database reopens

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("persist.db");

    // Create and populate database
    {
        let memdb = MemDb::open(&db_path, true).expect("Failed to create memdb");

        let mtime = 1234567890;
        memdb
            .create("/persistent.txt", libc::S_IFREG as u32, mtime)
            .expect("Failed to create file");
        memdb
            .write("/persistent.txt", 0, mtime, b"persistent data", 0, true)
            .expect("Failed to write");
    }

    // Reopen database and verify data
    {
        let memdb = MemDb::open(&db_path, false).expect("Failed to open memdb");

        let data = memdb
            .read("/persistent.txt", 0, 1024)
            .expect("Failed to read file");
        assert_eq!(data, b"persistent data", "Data should persist");
    }

}

#[test]
fn test_file_operations() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("ops.db");
    let memdb = MemDb::open(&db_path, true).expect("Failed to create memdb");

    let mtime = 1234567890;

    // Test rename
    memdb
        .create("/old-name.txt", libc::S_IFREG as u32, mtime)
        .expect("Failed to create file");
    memdb
        .write("/old-name.txt", 0, mtime, b"test", 0, true)
        .expect("Failed to write");

    memdb
        .rename("/old-name.txt", "/new-name.txt")
        .expect("Failed to rename");

    assert!(
        !memdb.exists("/old-name.txt").unwrap(),
        "Old name should not exist"
    );
    assert!(
        memdb.exists("/new-name.txt").unwrap(),
        "New name should exist"
    );

    // Test delete
    memdb.delete("/new-name.txt", 0, 1000).expect("Failed to delete");
    assert!(
        !memdb.exists("/new-name.txt").unwrap(),
        "File should be deleted"
    );

}
