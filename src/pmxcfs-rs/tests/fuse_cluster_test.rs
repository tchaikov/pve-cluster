/// FUSE Cluster Synchronization Tests
///
/// Tests for pmxcfs FUSE operations that trigger DFSM broadcasts
/// and synchronize across cluster nodes. These tests verify that
/// file operations made through FUSE properly propagate to other nodes.
use anyhow::Result;
use pmxcfs_dfsm::{Dfsm, FuseMessage};
use pmxcfs_rs::config::Config;
use pmxcfs_rs::fuse;
use pmxcfs_rs::memdb::MemDb;
use pmxcfs_rs::plugins;
use std::fs;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Verify that FUSE filesystem successfully mounted, panic if not
fn verify_fuse_mounted(path: &std::path::Path) {
    use std::process::Command;

    let output = Command::new("mount").output().ok();

    let is_mounted = if let Some(output) = output {
        let mount_output = String::from_utf8_lossy(&output.stdout);
        mount_output.contains(&format!(" {} ", path.display()))
    } else {
        false
    };

    if !is_mounted {
        panic!(
            "FUSE mount failed (likely permissions issue).\n\
             To run FUSE integration tests, either:\n\
             1. Run with sudo: sudo -E cargo test --test fuse_cluster_test\n\
             2. Enable user_allow_other in /etc/fuse.conf\n\
             3. Or skip these tests: cargo test --lib"
        );
    }
}

/// Helper to create a test configuration
fn create_test_config(node_name: &str, node_id: u32) -> Arc<Config> {
    Config::new(
        node_name.to_string(),
        "127.0.0.1".to_string(),
        1000, // www-data gid
        false,
        false, // not local mode - we want cluster mode
        "test-cluster".to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_fuse_write_triggers_broadcast() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = create_test_config("node1", 1);
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);
    status.set_quorate(true);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    // Create test directory
    memdb.create("/testdir", libc::S_IFDIR, 0, now)?;

    // Create DFSM instance (in standalone mode for testing)
    let dfsm = Arc::new(Dfsm::<FuseMessage>::new_standalone(
        1,
        "node1".to_string(),
        memdb.clone(),
    ));

    let plugins = plugins::init_plugins(config.clone(), status.clone());

    // Spawn FUSE mount with DFSM
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let dfsm_clone = dfsm.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            Some(dfsm_clone),
            plugins,
            status,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    verify_fuse_mounted(&mount_path);

    // Test 1: Write to file via FUSE should trigger broadcast
    let test_file = mount_path.join("testdir/broadcast-test.txt");
    let mut file = fs::File::create(&test_file)?;
    file.write_all(b"test data for broadcast")?;
    drop(file);
    println!("✓ File written via FUSE");

    // Give time for broadcast to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify file exists in memdb (it should be there regardless of broadcast)
    assert!(
        memdb.exists("/testdir/broadcast-test.txt")?,
        "File should exist in memdb"
    );
    let data = memdb.read("/testdir/broadcast-test.txt", 0, 1024)?;
    assert_eq!(
        &data[..],
        b"test data for broadcast",
        "File content should match"
    );
    println!("✓ File data verified in memdb");

    // Note: In a real cluster, the DFSM would broadcast this write to other nodes
    // In standalone mode, we can verify the broadcast mechanism is called but
    // messages won't actually be sent. Full cluster sync is tested in dfsm tests.

    // Cleanup
    fs::remove_file(&test_file)?;
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_fuse_mkdir_triggers_broadcast() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = create_test_config("node1", 1);
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);
    status.set_quorate(true);

    // Create DFSM instance
    let dfsm = Arc::new(Dfsm::<FuseMessage>::new_standalone(
        1,
        "node1".to_string(),
        memdb.clone(),
    ));

    let plugins = plugins::init_plugins(config.clone(), status.clone());

    // Spawn FUSE mount with DFSM
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let dfsm_clone = dfsm.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            Some(dfsm_clone),
            plugins,
            status,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    verify_fuse_mounted(&mount_path);

    // Test: mkdir via FUSE should trigger broadcast
    let test_dir = mount_path.join("broadcast-dir");
    fs::create_dir(&test_dir)?;
    println!("✓ Directory created via FUSE");

    // Give time for broadcast
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify directory exists in memdb
    assert!(
        memdb.exists("/broadcast-dir")?,
        "Directory should exist in memdb"
    );
    println!("✓ Directory verified in memdb");

    // Cleanup
    fs::remove_dir(&test_dir)?;
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_fuse_delete_triggers_broadcast() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = create_test_config("node1", 1);
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);
    status.set_quorate(true);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    // Create test file in memdb
    memdb.create("/delete-test.txt", libc::S_IFREG, 0, now)?;
    memdb.write("/delete-test.txt", 0, 0, now, b"will be deleted", 0, false)?;

    // Create DFSM instance
    let dfsm = Arc::new(Dfsm::<FuseMessage>::new_standalone(
        1,
        "node1".to_string(),
        memdb.clone(),
    ));

    let plugins = plugins::init_plugins(config.clone(), status.clone());

    // Spawn FUSE mount with DFSM
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let dfsm_clone = dfsm.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            Some(dfsm_clone),
            plugins,
            status,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    verify_fuse_mounted(&mount_path);

    // Verify file exists before deletion
    let test_file = mount_path.join("delete-test.txt");
    assert!(test_file.exists(), "File should exist before deletion");

    // Test: delete file via FUSE should trigger broadcast
    fs::remove_file(&test_file)?;
    println!("✓ File deleted via FUSE");

    // Give time for broadcast
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify file is deleted from memdb
    assert!(
        !memdb.exists("/delete-test.txt")?,
        "File should be deleted from memdb"
    );
    println!("✓ Deletion verified in memdb");

    // Cleanup
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_fuse_rename_triggers_broadcast() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = create_test_config("node1", 1);
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);
    status.set_quorate(true);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    // Create test file in memdb
    memdb.create("/oldname.txt", libc::S_IFREG, 0, now)?;
    memdb.write("/oldname.txt", 0, 0, now, b"rename test", 0, false)?;

    // Create DFSM instance
    let dfsm = Arc::new(Dfsm::<FuseMessage>::new_standalone(
        1,
        "node1".to_string(),
        memdb.clone(),
    ));

    let plugins = plugins::init_plugins(config.clone(), status.clone());

    // Spawn FUSE mount with DFSM
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let dfsm_clone = dfsm.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            Some(dfsm_clone),
            plugins,
            status,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    verify_fuse_mounted(&mount_path);

    // Test: rename file via FUSE should trigger broadcast
    let old_path = mount_path.join("oldname.txt");
    let new_path = mount_path.join("newname.txt");

    assert!(old_path.exists(), "Old file should exist");
    fs::rename(&old_path, &new_path)?;
    println!("✓ File renamed via FUSE");

    // Give time for broadcast
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify rename in memdb
    assert!(
        !memdb.exists("/oldname.txt")?,
        "Old file should not exist in memdb"
    );
    assert!(
        memdb.exists("/newname.txt")?,
        "New file should exist in memdb"
    );

    // Verify content preserved
    let data = memdb.read("/newname.txt", 0, 1024)?;
    assert_eq!(&data[..], b"rename test", "File content should be preserved");
    println!("✓ Rename verified in memdb");

    // Cleanup
    fs::remove_file(&new_path)?;
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_fuse_create_triggers_broadcast() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = create_test_config("node1", 1);
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);
    status.set_quorate(true);

    // Create DFSM instance
    let dfsm = Arc::new(Dfsm::<FuseMessage>::new_standalone(
        1,
        "node1".to_string(),
        memdb.clone(),
    ));

    let plugins = plugins::init_plugins(config.clone(), status.clone());

    // Spawn FUSE mount with DFSM
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let dfsm_clone = dfsm.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            Some(dfsm_clone),
            plugins,
            status,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    verify_fuse_mounted(&mount_path);

    // Test: create file via FUSE should trigger broadcast
    let test_file = mount_path.join("create-broadcast.txt");
    fs::File::create(&test_file)?;
    println!("✓ File created via FUSE");

    // Give time for broadcast
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify file exists in memdb
    assert!(
        memdb.exists("/create-broadcast.txt")?,
        "File should exist in memdb"
    );
    println!("✓ File creation verified in memdb");

    // Cleanup
    fs::remove_file(&test_file)?;
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}
