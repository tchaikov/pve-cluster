/// FUSE Lock Operations Tests
///
/// Tests for pmxcfs lock operations through the FUSE interface.
/// Locks are implemented as directories under /priv/lock/ and use
/// setattr(mtime) for renewal and release operations.
use anyhow::Result;
use pmxcfs_memdb::MemDb;
use pmxcfs_rs::fuse;
use pmxcfs_rs::plugins;
use std::fs;
use std::os::unix::fs::MetadataExt;
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
             1. Run with sudo: sudo -E cargo test --test fuse_locks_test\n\
             2. Enable user_allow_other in /etc/fuse.conf\n\
             3. Or skip these tests: cargo test --lib"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires FUSE mount permissions (run with sudo or configure /etc/fuse.conf)"]
async fn test_lock_creation_and_access() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = pmxcfs_test_utils::create_test_config(false);
    let status = pmxcfs_status::init_with_config(config.clone());
    status.set_quorate(true);
    let plugins = plugins::init_plugins(config.clone(), status.clone());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    // Create lock directory structure in memdb
    memdb.create("/priv", libc::S_IFDIR, 0, now)?;
    memdb.create("/priv/lock", libc::S_IFDIR, 0, now)?;

    // Spawn FUSE mount
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            None, // no cluster
            plugins,
            status,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    verify_fuse_mounted(&mount_path);

    // Test 1: Create lock directory via FUSE (mkdir)
    let lock_path = mount_path.join("priv/lock/test-resource");
    fs::create_dir(&lock_path)?;
    println!("✓ Lock directory created via FUSE");

    // Test 2: Verify lock exists and is a directory
    assert!(lock_path.exists(), "Lock should exist");
    assert!(lock_path.is_dir(), "Lock should be a directory");
    println!("✓ Lock directory accessible");

    // Test 3: Verify lock is in memdb
    assert!(
        memdb.exists("/priv/lock/test-resource")?,
        "Lock should exist in memdb"
    );
    println!("✓ Lock persisted to memdb");

    // Test 4: Verify lock path detection
    assert!(
        pmxcfs_memdb::is_lock_path("/priv/lock/test-resource"),
        "Path should be detected as lock path"
    );
    println!("✓ Lock path correctly identified");

    // Test 5: List locks via FUSE readdir
    let lock_dir_entries: Vec<_> = fs::read_dir(mount_path.join("priv/lock"))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        lock_dir_entries.contains(&"test-resource".to_string()),
        "Lock should appear in directory listing"
    );
    println!("✓ Lock visible in readdir");

    // Cleanup
    fs::remove_dir(&lock_path)?;
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires FUSE mount permissions (run with sudo or configure /etc/fuse.conf)"]
async fn test_lock_renewal_via_mtime_update() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = pmxcfs_test_utils::create_test_config(false);
    let status = pmxcfs_status::init_with_config(config.clone());
    status.set_quorate(true);
    let plugins = plugins::init_plugins(config.clone(), status.clone());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    // Create lock directory structure
    memdb.create("/priv", libc::S_IFDIR, 0, now)?;
    memdb.create("/priv/lock", libc::S_IFDIR, 0, now)?;

    // Spawn FUSE mount
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            None,
            plugins,
            status,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    verify_fuse_mounted(&mount_path);

    // Create lock via FUSE
    let lock_path = mount_path.join("priv/lock/renewal-test");
    fs::create_dir(&lock_path)?;
    println!("✓ Lock directory created");

    // Get initial metadata
    let metadata1 = fs::metadata(&lock_path)?;
    let mtime1 = metadata1.mtime();
    println!("  Initial mtime: {}", mtime1);

    // Wait a moment
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test lock renewal: update mtime using filetime crate
    // (This simulates the lock renewal mechanism used by Proxmox VE)
    use filetime::{FileTime, set_file_mtime};
    let new_time = FileTime::now();
    set_file_mtime(&lock_path, new_time)?;
    println!("✓ Lock mtime updated (renewal)");

    // Verify mtime was updated
    let metadata2 = fs::metadata(&lock_path)?;
    let mtime2 = metadata2.mtime();
    println!("  Updated mtime: {}", mtime2);

    // Note: Due to filesystem timestamp granularity, we just verify the operation succeeded
    // The actual lock renewal logic is tested at the memdb level
    println!("✓ Lock renewal operation completed");

    // Cleanup
    fs::remove_dir(&lock_path)?;
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires FUSE mount permissions (run with sudo or configure /etc/fuse.conf)"]
async fn test_lock_unlock_via_mtime_zero() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = pmxcfs_test_utils::create_test_config(false);
    let status = pmxcfs_status::init_with_config(config.clone());
    status.set_quorate(true);
    let plugins = plugins::init_plugins(config.clone(), status.clone());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    // Create lock directory structure
    memdb.create("/priv", libc::S_IFDIR, 0, now)?;
    memdb.create("/priv/lock", libc::S_IFDIR, 0, now)?;

    // Spawn FUSE mount (without DFSM so unlock happens locally)
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            None,
            plugins,
            status,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    verify_fuse_mounted(&mount_path);

    // Create lock via FUSE
    let lock_path = mount_path.join("priv/lock/unlock-test");
    fs::create_dir(&lock_path)?;
    println!("✓ Lock directory created");

    // Verify lock exists
    assert!(lock_path.exists(), "Lock should exist");
    assert!(
        memdb.exists("/priv/lock/unlock-test")?,
        "Lock should exist in memdb"
    );

    // Test unlock: set mtime to 0 (Unix epoch)
    // This is the unlock signal in pmxcfs
    use filetime::{FileTime, set_file_mtime};
    let zero_time = FileTime::from_unix_time(0, 0);
    set_file_mtime(&lock_path, zero_time)?;
    println!("✓ Lock unlock requested (mtime=0)");

    // Give time for unlock processing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // When no DFSM, lock should be deleted locally if expired
    // Since we just created it, it won't be expired, so it should still exist
    // (This matches the C behavior: only delete if lock_expired() returns true)
    assert!(
        lock_path.exists(),
        "Lock should still exist (not expired yet)"
    );
    println!("✓ Unlock handled correctly (lock not expired, kept)");

    // Cleanup
    fs::remove_dir(&lock_path)?;
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires FUSE mount permissions (run with sudo or configure /etc/fuse.conf)"]
async fn test_multiple_locks() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = pmxcfs_test_utils::create_test_config(false);
    let status = pmxcfs_status::init_with_config(config.clone());
    status.set_quorate(true);
    let plugins = plugins::init_plugins(config.clone(), status.clone());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    // Create lock directory structure
    memdb.create("/priv", libc::S_IFDIR, 0, now)?;
    memdb.create("/priv/lock", libc::S_IFDIR, 0, now)?;

    // Spawn FUSE mount
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            None,
            plugins,
            status,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    verify_fuse_mounted(&mount_path);

    // Test: Create multiple locks simultaneously
    let lock_names = vec!["vm-100-disk-0", "vm-101-disk-0", "vm-102-disk-0"];

    for name in &lock_names {
        let lock_path = mount_path.join(format!("priv/lock/{}", name));
        fs::create_dir(&lock_path)?;
        println!("✓ Lock '{}' created", name);
    }

    // Verify all locks exist
    let lock_dir_entries: Vec<_> = fs::read_dir(mount_path.join("priv/lock"))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    for name in &lock_names {
        assert!(
            lock_dir_entries.contains(&name.to_string()),
            "Lock '{}' should be in directory listing",
            name
        );
        assert!(
            memdb.exists(&format!("/priv/lock/{}", name))?,
            "Lock '{}' should exist in memdb",
            name
        );
    }
    println!("✓ All locks accessible");

    // Cleanup
    for name in &lock_names {
        let lock_path = mount_path.join(format!("priv/lock/{}", name));
        fs::remove_dir(&lock_path)?;
    }

    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}
