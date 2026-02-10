/// Basic FUSE subsystem test
///
/// This test verifies core FUSE functionality without actually mounting
/// to avoid test complexity and timeouts
use anyhow::Result;
use pmxcfs_rs::config::Config;
use pmxcfs_rs::memdb::MemDb;
use pmxcfs_rs::plugins;
use tempfile::TempDir;

#[test]
fn test_fuse_subsystem_components() -> Result<()> {

    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    // 1. Create memdb with test data
    let memdb = MemDb::open(&db_path, true)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    memdb.create("/testdir", libc::S_IFDIR, 0, now)?;
    memdb.create("/testdir/file1.txt", libc::S_IFREG, 0, now)?;
    memdb.write("/testdir/file1.txt", 0, 0, now, b"Hello pmxcfs!", 0, false)?;

    // 2. Create config
    println!("\n2. Creating FUSE configuration...");
    let config = Config::new(
        "testnode".to_string(),
        "127.0.0.1".to_string(),
        1000,
        false,
        true,
        "test-cluster".to_string(),
    );

    // 3. Initialize plugins
    println!("\n3. Initializing plugin registry...");
    let plugins = plugins::init_plugins("localhost");
    let plugin_list = plugins.list();
    println!("   Available plugins: {:?}", plugin_list);
    assert!(plugin_list.len() > 0, "Should have some plugins");

    // 4. Verify plugin functionality
    for plugin_name in &plugin_list {
        if let Some(plugin) = plugins.get(plugin_name) {
            match plugin.read() {
                Ok(data) => {
                    println!(
                        "   ✅ Plugin '{}' readable ({} bytes)",
                        plugin_name,
                        data.len()
                    );
                }
                Err(e) => {
                    println!("   ⚠️  Plugin '{}' error: {}", plugin_name, e);
                }
            }
        }
    }

    // 5. Verify memdb data is accessible
    println!("\n5. Verifying memdb data accessibility...");
    assert!(memdb.exists("/testdir")?, "testdir should exist");
    assert!(
        memdb.exists("/testdir/file1.txt")?,
        "file1.txt should exist"
    );

    let data = memdb.read("/testdir/file1.txt", 0, 1024)?;
    assert_eq!(&data[..], b"Hello pmxcfs!");

    // 6. Test write operations
    let new_data = b"Modified!";
    memdb.write(
        "/testdir/file1.txt",
        0,
        now,
        new_data,
        new_data.len() as u64,
        true,
    )?;
    let data = memdb.read("/testdir/file1.txt", 0, 1024)?;
    assert_eq!(&data[..], b"Modified!");

    // 7. Test directory operations
    memdb.create("/newdir", libc::S_IFDIR, 0, now)?;
    memdb.create("/newdir/newfile.txt", libc::S_IFREG, 0, now)?;
    memdb.write("/newdir/newfile.txt", 0, 0, now, b"New content", 0, false)?;

    let entries = memdb.readdir("/")?;
    let dir_names: Vec<&String> = entries.iter().map(|e| &e.name).collect();
    println!("   Root entries: {:?}", dir_names);
    assert!(
        dir_names.iter().any(|n| n == &"testdir"),
        "testdir should be in root"
    );
    assert!(
        dir_names.iter().any(|n| n == &"newdir"),
        "newdir should be in root"
    );

    // 8. Test deletion
    memdb.delete("/newdir/newfile.txt", 0, 1000)?;
    memdb.delete("/newdir", 0, 1000)?;
    assert!(!memdb.exists("/newdir")?, "newdir should be deleted");


    Ok(())
}

#[test]
fn test_fuse_private_path_detection() -> Result<()> {

    // This tests the logic that would be used in the FUSE filesystem
    // to determine if paths should have restricted permissions

    let test_cases = vec![
        ("/priv", true, "root priv should be private"),
        ("/priv/test", true, "priv subdir should be private"),
        ("/nodes/node1/priv", true, "node priv should be private"),
        (
            "/nodes/node1/priv/data",
            true,
            "node priv subdir should be private",
        ),
        (
            "/nodes/node1/config",
            false,
            "node config should not be private",
        ),
        ("/testdir", false, "testdir should not be private"),
        (
            "/private",
            false,
            "private (not priv) should not be private",
        ),
    ];

    for (path, expected, description) in test_cases {
        let is_private = is_private_path(path);
        assert_eq!(is_private, expected, "Failed for {}: {}", path, description);
    }

    Ok(())
}

// Helper function matching the logic in filesystem.rs
fn is_private_path(path: &str) -> bool {
    let path = path.trim_start_matches('/');

    // Check if path starts with "priv" or "priv/"
    if path.starts_with("priv") && (path.len() == 4 || path.as_bytes().get(4) == Some(&b'/')) {
        return true;
    }

    // Check for "nodes/*/priv" or "nodes/*/priv/*" pattern
    if let Some(after_nodes) = path.strip_prefix("nodes/") {
        if let Some(slash_pos) = after_nodes.find('/') {
            let after_nodename = &after_nodes[slash_pos..];

            if after_nodename.starts_with("/priv") {
                let priv_end = slash_pos + 5;
                if after_nodes.len() == priv_end
                    || after_nodes.as_bytes().get(priv_end) == Some(&b'/')
                {
                    return true;
                }
            }
        }
    }

    false
}

#[test]
fn test_fuse_inode_path_mapping() -> Result<()> {

    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let memdb = MemDb::open(&db_path, true)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    // Create nested directory structure
    memdb.create("/a", libc::S_IFDIR, 0, now)?;
    memdb.create("/a/b", libc::S_IFDIR, 0, now)?;
    memdb.create("/a/b/c", libc::S_IFDIR, 0, now)?;
    memdb.create("/a/b/c/file.txt", libc::S_IFREG, 0, now)?;
    memdb.write("/a/b/c/file.txt", 0, 0, now, b"deep file", 0, false)?;


    // Verify we can look up deep paths
    let entry = memdb
        .lookup_path("/a/b/c/file.txt")
        .ok_or_else(|| anyhow::anyhow!("Failed to lookup deep path"))?;

    println!("   Inode: {}", entry.inode);
    println!("   Size: {}", entry.size);
    assert!(entry.inode > 1, "Should have valid inode");
    assert_eq!(entry.size, 9, "File size should match");


    // Verify parent relationships
    println!("\n3. Verifying parent relationships...");
    let c_entry = memdb
        .lookup_path("/a/b/c")
        .ok_or_else(|| anyhow::anyhow!("Failed to lookup /a/b/c"))?;
    let b_entry = memdb
        .lookup_path("/a/b")
        .ok_or_else(|| anyhow::anyhow!("Failed to lookup /a/b"))?;

    assert_eq!(
        entry.parent, c_entry.inode,
        "file.txt parent should be c directory"
    );
    assert_eq!(
        c_entry.parent, b_entry.inode,
        "c parent should be b directory"
    );



    Ok(())
}
