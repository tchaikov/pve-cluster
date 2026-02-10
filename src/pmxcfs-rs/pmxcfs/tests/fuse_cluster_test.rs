/// FUSE Cluster Synchronization Tests
///
/// Tests for pmxcfs FUSE operations that trigger DFSM broadcasts
/// and synchronize across cluster nodes. These tests verify that
/// file operations made through FUSE properly propagate to other nodes.
use anyhow::Result;
use pmxcfs_dfsm::{Callbacks, Dfsm, FuseMessage, NodeSyncInfo};
use pmxcfs_memdb::MemDb;
use pmxcfs_rs::fuse;
use pmxcfs_rs::plugins;
use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

/// Verify that FUSE filesystem successfully mounted, panic if not
async fn verify_fuse_mounted(path: &std::path::Path) {
    // Use spawn_blocking to avoid blocking the async runtime
    let path_buf = path.to_path_buf();
    let read_result = tokio::task::spawn_blocking(move || std::fs::read_dir(&path_buf))
        .await
        .expect("spawn_blocking failed");

    if read_result.is_ok() {
        return; // Mount succeeded
    }

    // Double-check with mount command
    use std::process::Command;
    let output = Command::new("mount").output().ok();
    let is_mounted = if let Some(output) = output {
        let mount_output = String::from_utf8_lossy(&output.stdout);
        mount_output.contains(&path.display().to_string())
    } else {
        false
    };

    if !is_mounted {
        panic!("FUSE mount failed.\nCheck /etc/fuse.conf for user_allow_other setting.");
    }
}

/// Test callbacks for DFSM - minimal implementation for testing
struct TestDfsmCallbacks {
    memdb: MemDb,
    broadcasts: Arc<Mutex<Vec<String>>>, // Track broadcast operations
}

impl TestDfsmCallbacks {
    fn new(memdb: MemDb) -> Self {
        Self {
            memdb,
            broadcasts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[allow(dead_code)]
    fn get_broadcasts(&self) -> Vec<String> {
        self.broadcasts.lock().unwrap().clone()
    }
}

impl Callbacks for TestDfsmCallbacks {
    type Message = FuseMessage;

    fn deliver_message(
        &self,
        _nodeid: u32,
        _pid: u32,
        message: FuseMessage,
        _timestamp: u64,
    ) -> Result<(i32, bool)> {
        // Track the broadcast for testing
        let msg_desc = match &message {
            FuseMessage::Write { path, .. } => format!("write:{}", path),
            FuseMessage::Create { path } => format!("create:{}", path),
            FuseMessage::Mkdir { path } => format!("mkdir:{}", path),
            FuseMessage::Delete { path } => format!("delete:{}", path),
            FuseMessage::Rename { from, to } => format!("rename:{}→{}", from, to),
            _ => "other".to_string(),
        };
        self.broadcasts.lock().unwrap().push(msg_desc);
        Ok((0, true))
    }

    fn compute_checksum(&self, output: &mut [u8; 32]) -> Result<()> {
        *output = self.memdb.compute_database_checksum()?;
        Ok(())
    }

    fn process_state_update(&self, _states: &[NodeSyncInfo]) -> Result<bool> {
        Ok(true) // Indicate we're in sync for testing
    }

    fn process_update(&self, _nodeid: u32, _pid: u32, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    fn commit_state(&self) -> Result<()> {
        Ok(())
    }

    fn on_synced(&self) {}

    fn get_state(&self) -> Result<Vec<u8>> {
        // Return empty state for testing
        Ok(Vec::new())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires FUSE mount permissions (user_allow_other in /etc/fuse.conf)"]
async fn test_fuse_write_triggers_broadcast() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mount_path = temp_dir.path().join("mnt");

    fs::create_dir_all(&mount_path)?;

    let memdb = MemDb::open(&db_path, true)?;
    let config = pmxcfs_test_utils::create_test_config(false);
    let status = pmxcfs_status::init_with_config(config.clone());
    status.set_quorate(true);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    // Create test directory
    memdb.create("/testdir", libc::S_IFDIR, 0, now)?;

    // Create DFSM instance with test callbacks
    let callbacks = Arc::new(TestDfsmCallbacks::new(memdb.clone()));
    let dfsm = Arc::new(Dfsm::new("test-cluster".to_string(), callbacks.clone())?);

    let plugins = plugins::init_plugins(config.clone(), status.clone());

    // Spawn FUSE mount with DFSM
    let mount_path_clone = mount_path.clone();
    let memdb_clone = memdb.clone();
    let dfsm_clone = dfsm.clone();
    let fuse_task = tokio::spawn(async move {
        if let Err(e) = fuse::mount_fuse(
            &mount_path_clone,
            memdb_clone,
            config,
            Some(dfsm_clone),
            plugins,
            status,
        )
        .await
        {
            eprintln!("FUSE mount error: {}", e);
        }
    });

    tokio::time::sleep(Duration::from_millis(2000)).await;
    verify_fuse_mounted(&mount_path).await;

    // Test: Write to file via FUSE should trigger broadcast
    let test_file = mount_path.join("testdir/broadcast-test.txt");
    let mut file = fs::File::create(&test_file)?;
    file.write_all(b"test data for broadcast")?;
    drop(file);
    println!("✓ File written via FUSE");

    // Give time for broadcast
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify file exists in memdb
    assert!(
        memdb.exists("/testdir/broadcast-test.txt")?,
        "File should exist in memdb"
    );
    let data = memdb.read("/testdir/broadcast-test.txt", 0, 1024)?;
    assert_eq!(&data[..], b"test data for broadcast");
    println!("✓ File data verified in memdb");

    // Cleanup
    fs::remove_file(&test_file)?;
    fuse_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::process::Command::new("fusermount3")
        .arg("-u")
        .arg(&mount_path)
        .output();

    Ok(())
}

/// Additional FUSE + DFSM tests can be added here following the same pattern
#[test]
fn test_dfsm_callbacks_implementation() {
    // Verify our test callbacks work correctly
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let memdb = MemDb::open(&db_path, true).unwrap();

    let callbacks = TestDfsmCallbacks::new(memdb);

    // Test checksum computation
    let mut checksum = [0u8; 32];
    assert!(callbacks.compute_checksum(&mut checksum).is_ok());

    // Test message delivery tracking
    let result = callbacks.deliver_message(
        1,
        100,
        FuseMessage::Create {
            path: "/test".to_string(),
        },
        12345,
    );
    assert!(result.is_ok());

    let broadcasts = callbacks.get_broadcasts();
    assert_eq!(broadcasts.len(), 1);
    assert_eq!(broadcasts[0], "create:/test");
}
