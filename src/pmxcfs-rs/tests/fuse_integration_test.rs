/// Integration tests for FUSE filesystem with proxmox-fuse-rs
///
/// These tests verify that the FUSE subsystem works correctly after
/// migrating from fuser to proxmox-fuse-rs
use anyhow::Result;
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
             1. Run with sudo: sudo -E cargo test --test fuse_integration_test\n\
             2. Enable user_allow_other in /etc/fuse.conf and add your user to the 'fuse' group\n\
             3. Or skip these tests: cargo test --test fuse_basic_test"
        );
    }
}

/// Helper to create a test configuration
fn create_test_config() -> Arc<Config> {
    Config::new(
        "testnode".to_string(),
        "127.0.0.1".to_string(),
        1000, // www-data gid
        false,
        true, // local mode
        "test-cluster".to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_fuse_mount_and_basic_operations() -> Result<()> {

    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    // Create mount point
    fs::create_dir_all(&mount_path)?;

    // Create database
    let memdb = MemDb::open(&db_path, true)?;

    // Create some test data in memdb
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    memdb.create("/testdir", libc::S_IFDIR, 0, now)?;
    memdb.create("/testdir/file1.txt", libc::S_IFREG, 0, now)?;
    memdb.write(
        "/testdir/file1.txt",
        0,
        now,
        b"Hello from pmxcfs!",
        0,
        false,
    )?;

    memdb.create("/nodes", libc::S_IFDIR, 0, now)?;
    memdb.create("/nodes/testnode", libc::S_IFDIR, 0, now)?;
    memdb.create("/nodes/testnode/config", libc::S_IFREG, 0, now)?;
    memdb.write(
        "/nodes/testnode/config",
        0,
        now,
        b"test=configuration",
        0,
        false,
    )?;


    // Create config and plugins (no RRD persistence needed for test)
    let config = create_test_config();
    let plugins = {
        let test_config = pmxcfs_test_utils::create_test_config(false);
        let test_status = pmxcfs_status::init_with_config(test_config.clone());
        plugins::init_plugins(config.clone(), test_status)
    };

    // Create status for FUSE (set quorate for tests)
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);
    status.set_quorate(true);

    // Spawn FUSE mount in background
    println!("\n2. Mounting FUSE filesystem...");
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let config_clone = config.clone();
    let plugins_clone = plugins.clone();
    let status_clone = status.clone();

    let fuse_task = tokio::spawn(async move {
        if let Err(e) = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config_clone,
            None, // no cluster
            plugins_clone,
            status_clone,
        )
        .await
        {
            eprintln!("FUSE mount error: {}", e);
        }
    });

    // Give FUSE time to initialize and check if mount succeeded
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify FUSE mounted successfully
    verify_fuse_mounted(&mount_path);


    // Test 1: Check if mount point is accessible
    let root_entries = fs::read_dir(&mount_path)?;
    let mut entry_names: Vec<String> = root_entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    entry_names.sort();

    println!("   Root directory entries: {:?}", entry_names);
    assert!(
        entry_names.contains(&"testdir".to_string()),
        "testdir should be visible"
    );
    assert!(
        entry_names.contains(&"nodes".to_string()),
        "nodes should be visible"
    );

    // Test 2: Read existing file
    let file_path = mount_path.join("testdir/file1.txt");
    let mut file = fs::File::open(&file_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    assert_eq!(contents, "Hello from pmxcfs!");
    println!("   Read: '{}'", contents);

    // Test 3: Write to existing file
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&file_path)?;
    file.write_all(b"Modified content!")?;
    drop(file);

    // Verify write
    let mut file = fs::File::open(&file_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    assert_eq!(contents, "Modified content!");
    println!("   After write: '{}'", contents);

    // Test 4: Create new file
    let new_file_path = mount_path.join("testdir/newfile.txt");
    eprintln!("DEBUG: About to create file at {:?}", new_file_path);
    let mut new_file = match fs::File::create(&new_file_path) {
        Ok(f) => {
            eprintln!("DEBUG: File created OK");
            f
        }
        Err(e) => {
            eprintln!("DEBUG: File create FAILED: {:?}", e);
            return Err(e.into());
        }
    };
    eprintln!("DEBUG: Writing content");
    new_file.write_all(b"New file content")?;
    eprintln!("DEBUG: Content written");
    drop(new_file);
    eprintln!("DEBUG: File closed");

    // Verify creation
    let mut file = fs::File::open(&new_file_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    assert_eq!(contents, "New file content");
    println!("   Created and verified: newfile.txt");

    // Test 5: Create directory
    let new_dir_path = mount_path.join("newdir");
    fs::create_dir(&new_dir_path)?;

    // Verify directory exists
    assert!(new_dir_path.exists());
    assert!(new_dir_path.is_dir());

    // Test 6: List directory
    let testdir_entries = fs::read_dir(mount_path.join("testdir"))?;
    let mut file_names: Vec<String> = testdir_entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    file_names.sort();

    println!("   testdir entries: {:?}", file_names);
    assert!(
        file_names.contains(&"file1.txt".to_string()),
        "file1.txt should exist"
    );
    assert!(
        file_names.contains(&"newfile.txt".to_string()),
        "newfile.txt should exist"
    );

    // Test 7: Get file metadata
    let metadata = fs::metadata(&file_path)?;
    println!("   File size: {} bytes", metadata.len());
    println!("   Is file: {}", metadata.is_file());
    println!("   Is dir: {}", metadata.is_dir());
    assert!(metadata.is_file());
    assert!(!metadata.is_dir());

    // Test 8: Test plugin files
    let plugin_files = vec![".version", ".members", ".vmlist", ".rrd", ".clusterlog"];

    for plugin_name in &plugin_files {
        let plugin_path = mount_path.join(plugin_name);
        if plugin_path.exists() {
            match fs::File::open(&plugin_path) {
                Ok(mut file) => {
                    let mut contents = Vec::new();
                    file.read_to_end(&mut contents)?;
                    println!(
                        "   ✅ Plugin '{}' readable ({} bytes)",
                        plugin_name,
                        contents.len()
                    );
                }
                Err(e) => {
                    println!(
                        "   ⚠️  Plugin '{}' exists but not readable: {}",
                        plugin_name, e
                    );
                }
            }
        } else {
            println!("   ℹ️  Plugin '{}' not present", plugin_name);
        }
    }

    // Test 9: Delete file
    fs::remove_file(&new_file_path)?;
    assert!(!new_file_path.exists());

    // Test 10: Delete directory
    fs::remove_dir(&new_dir_path)?;
    assert!(!new_dir_path.exists());

    // Test 11: Verify changes persisted to memdb
    println!("\n13. Verifying memdb persistence...");
    assert!(
        memdb.exists("/testdir/file1.txt")?,
        "file1.txt should exist in memdb"
    );
    assert!(
        !memdb.exists("/testdir/newfile.txt")?,
        "newfile.txt should be deleted from memdb"
    );
    assert!(
        !memdb.exists("/newdir")?,
        "newdir should be deleted from memdb"
    );

    let read_data = memdb.read("/testdir/file1.txt", 0, 1024)?;
    assert_eq!(
        &read_data[..],
        b"Modified content!",
        "File content should be updated in memdb"
    );


    // Cleanup: unmount filesystem
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Force unmount
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_fuse_concurrent_operations() -> Result<()> {

    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = create_test_config();
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);
    status.set_quorate(true);
    let plugins = plugins::init_plugins(config.clone(), status.clone());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    memdb.create("/testdir", libc::S_IFDIR, 0, now)?;

    // Spawn FUSE mount
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(&mount_path_clone, memdb_clone, config, None, plugins, status).await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify FUSE mounted successfully
    verify_fuse_mounted(&mount_path);


    // Create multiple files concurrently
    let mut tasks = vec![];
    for i in 0..5 {
        let mount = mount_path.clone();
        let task = tokio::task::spawn_blocking(move || -> Result<()> {
            let file_path = mount.join(format!("testdir/file{}.txt", i));
            let mut file = fs::File::create(&file_path)?;
            file.write_all(format!("Content {}", i).as_bytes())?;
            Ok(())
        });
        tasks.push(task);
    }

    // Wait for all tasks
    for task in tasks {
        task.await??;
    }


    // Read all files and verify
    for i in 0..5 {
        let file_path = mount_path.join(format!("testdir/file{}.txt", i));
        let mut file = fs::File::open(&file_path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        assert_eq!(contents, format!("Content {}", i));
    }



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
async fn test_fuse_error_handling() -> Result<()> {

    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = create_test_config();
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);
    status.set_quorate(true);
    let plugins = plugins::init_plugins(config.clone(), status.clone());

    // Spawn FUSE mount
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let fuse_task = tokio::spawn(async move {
        let _ = fuse::mount_fuse(&mount_path_clone, memdb_clone, config, None, plugins, status).await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify FUSE mounted successfully
    verify_fuse_mounted(&mount_path);

    let result = fs::File::open(mount_path.join("nonexistent.txt"));
    assert!(result.is_err(), "Should fail to open non-existent file");

    let result = fs::remove_file(mount_path.join("nonexistent.txt"));
    assert!(result.is_err(), "Should fail to delete non-existent file");

    let result = fs::create_dir(mount_path.join("nonexistent/subdir"));
    assert!(
        result.is_err(),
        "Should fail to create dir in non-existent parent"
    );


    // Cleanup
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("umount")
        .arg("-l")
        .arg(&mount_path)
        .output();

    Ok(())
}
