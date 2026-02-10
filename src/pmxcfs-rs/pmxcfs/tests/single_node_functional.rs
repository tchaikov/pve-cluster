/// Single-node functional test
///
/// This test simulates a complete single-node pmxcfs deployment
/// without requiring root privileges or actual FUSE mounting.
use anyhow::Result;
use pmxcfs_config::Config;
use pmxcfs_memdb::MemDb;
use pmxcfs_rs::plugins::{PluginRegistry, init_plugins};
use pmxcfs_status::{Status, VmType};
use std::sync::Arc;
use tempfile::TempDir;

/// Helper to initialize plugins for testing
fn init_test_plugins(nodename: &str, status: Arc<Status>) -> Arc<PluginRegistry> {
    let config = Config::shared(
        nodename.to_string(),
        "127.0.0.1".parse().unwrap(),
        33, // www-data gid
        false,
        false,
        "pmxcfs".to_string(),
    );
    init_plugins(config, status)
}

/// Test complete single-node workflow
#[tokio::test]
async fn test_single_node_workflow() -> Result<()> {
    println!("\n=== Single-Node Functional Test ===\n");

    // Initialize status subsystem
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

    // Clear any VMs from previous tests
    let existing_vms: Vec<u32> = status.get_vmlist().keys().copied().collect();
    for vmid in existing_vms {
        status.delete_vm(vmid);
    }

    let plugins = init_test_plugins("localhost", status.clone());
    println!(
        "   ✅ Plugin system initialized ({} plugins)",
        plugins.list().len()
    );

    // Create temporary database
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("pmxcfs.db");
    println!("\n2. Creating database at {}", db_path.display());

    let db = MemDb::open(&db_path, true)?;

    // Test directory structure creation
    println!("\n3. Creating directory structure...");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    db.create("/nodes", libc::S_IFDIR, 0, now)?;
    db.create("/nodes/localhost", libc::S_IFDIR, 0, now)?;
    db.create("/nodes/localhost/qemu-server", libc::S_IFDIR, 0, now)?;
    db.create("/nodes/localhost/lxc", libc::S_IFDIR, 0, now)?;
    db.create("/nodes/localhost/priv", libc::S_IFDIR, 0, now)?;

    db.create("/priv", libc::S_IFDIR, 0, now)?;
    db.create("/priv/lock", libc::S_IFDIR, 0, now)?;
    db.create("/priv/lock/qemu-server", libc::S_IFDIR, 0, now)?;
    db.create("/priv/lock/lxc", libc::S_IFDIR, 0, now)?;
    db.create("/qemu-server", libc::S_IFDIR, 0, now)?;
    db.create("/lxc", libc::S_IFDIR, 0, now)?;

    // Test configuration file creation
    println!("\n4. Creating configuration files...");

    // Create corosync.conf
    let corosync_conf = b"totem {\n  version: 2\n  cluster_name: test\n}\n";
    db.create("/corosync.conf", libc::S_IFREG, 0, now)?;
    db.write("/corosync.conf", 0, 0, now, corosync_conf, false)?;
    println!(
        "   ✅ Created /corosync.conf ({} bytes)",
        corosync_conf.len()
    );

    // Create datacenter.cfg
    let datacenter_cfg = b"keyboard: en-us\n";
    db.create("/datacenter.cfg", libc::S_IFREG, 0, now)?;
    db.write("/datacenter.cfg", 0, 0, now, datacenter_cfg, false)?;
    println!(
        "   ✅ Created /datacenter.cfg ({} bytes)",
        datacenter_cfg.len()
    );

    // Create some VM configs
    let vm_config = b"cores: 2\nmemory: 2048\nnet0: virtio=00:00:00:00:00:01,bridge=vmbr0\n";
    db.create("/qemu-server/100.conf", libc::S_IFREG, 0, now)?;
    db.write("/qemu-server/100.conf", 0, 0, now, vm_config, false)?;

    db.create("/qemu-server/101.conf", libc::S_IFREG, 0, now)?;
    db.write("/qemu-server/101.conf", 0, 0, now, vm_config, false)?;

    // Create LXC container config
    let ct_config = b"cores: 1\nmemory: 512\nrootfs: local:100/vm-100-disk-0.raw\n";
    db.create("/lxc/200.conf", libc::S_IFREG, 0, now)?;
    db.write("/lxc/200.conf", 0, 0, now, ct_config, false)?;

    // Create private file
    let private_data = b"secret token data";
    db.create("/priv/token.cfg", libc::S_IFREG, 0, now)?;
    db.write("/priv/token.cfg", 0, 0, now, private_data, false)?;

    // Test file operations

    // Read back corosync.conf
    let read_data = db.read("/corosync.conf", 0, 1024)?;
    assert_eq!(&read_data[..], corosync_conf);

    // Test file size limit (1MB)
    let large_data = vec![0u8; 1024 * 1024]; // Exactly 1MB
    db.create("/large.bin", libc::S_IFREG, 0, now)?;
    let result = db.write("/large.bin", 0, 0, now, &large_data, false);
    assert!(result.is_ok(), "1MB file should be accepted");

    // Test directory listing
    let entries = db.readdir("/qemu-server")?;
    assert_eq!(entries.len(), 2, "Should have 2 VM configs");

    // Test rename
    db.rename("/qemu-server/101.conf", "/qemu-server/102.conf", 0, 1000)?;
    assert!(db.exists("/qemu-server/102.conf")?);
    assert!(!db.exists("/qemu-server/101.conf")?);

    // Test delete
    db.delete("/large.bin", 0, 1000)?;
    assert!(!db.exists("/large.bin")?);

    // Test VM list management

    // Clear VMs again right before this section to avoid test interference
    let existing_vms: Vec<u32> = status.get_vmlist().keys().copied().collect();
    for vmid in existing_vms {
        status.delete_vm(vmid);
    }

    status.register_vm(100, VmType::Qemu, "localhost".to_string());
    status.register_vm(102, VmType::Qemu, "localhost".to_string());
    status.register_vm(200, VmType::Lxc, "localhost".to_string());

    let vmlist = status.get_vmlist();
    assert_eq!(vmlist.len(), 3, "Should have 3 VMs registered");

    // Verify VM types
    assert_eq!(vmlist.get(&100).unwrap().vmtype, VmType::Qemu);
    assert_eq!(vmlist.get(&200).unwrap().vmtype, VmType::Lxc);

    // Test lock management

    let lock_path = "/priv/lock/qemu-server/100.conf";
    let csum = [1u8; 32];

    db.acquire_lock(lock_path, &csum)?;
    assert!(db.is_locked(lock_path));

    db.release_lock(lock_path, &csum)?;
    assert!(!db.is_locked(lock_path));

    // Test checksum operations

    let checksum = db.compute_database_checksum()?;
    println!(
        "   ✅ Database checksum: {:02x}{:02x}{:02x}{:02x}...",
        checksum[0], checksum[1], checksum[2], checksum[3]
    );

    // Modify database and verify checksum changes
    db.write("/datacenter.cfg", 0, 0, now, b"keyboard: de\n", false)?;
    let new_checksum = db.compute_database_checksum()?;
    assert_ne!(
        checksum, new_checksum,
        "Checksum should change after modification"
    );

    // Test database encoding
    let _encoded = db.encode_database()?;

    // Test RRD data collection

    // Set RRD data in C-compatible format
    // Format: "key:timestamp:val1:val2:val3:..."
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    status
        .set_rrd_data(
            "pve2-node/localhost".to_string(),
            format!(
                "{}:0:1.5:4:45.5:2.1:8000000000:6000000000:0:0:0:0:1000000:500000",
                now
            ),
        )
        .await?;

    let rrd_dump = status.get_rrd_dump();
    assert!(
        rrd_dump.contains("pve2-node/localhost"),
        "Should have node data"
    );
    let num_entries = rrd_dump.lines().count();

    // Test cluster log
    use pmxcfs_status::ClusterLogEntry;
    let log_entry = ClusterLogEntry {
        uid: 0,
        timestamp: now,
        priority: 6, // Info priority
        tag: "startup".to_string(),
        pid: 0,
        node: "localhost".to_string(),
        ident: "pmxcfs".to_string(),
        message: "Cluster filesystem started".to_string(),
    };
    status.add_log_entry(log_entry);

    let log_entries = status.get_log_entries(100);
    assert_eq!(log_entries.len(), 1);

    // Test plugin system

    // Test .version plugin
    if let Some(plugin) = plugins.get(".version") {
        let content = plugin.read()?;
        let version_str = String::from_utf8(content)?;
        assert!(version_str.contains("version"));
        assert!(version_str.contains("9.0.6"));
    }

    // Test .vmlist plugin
    if let Some(plugin) = plugins.get(".vmlist") {
        let content = plugin.read()?;
        let vmlist_str = String::from_utf8(content)?;
        assert!(vmlist_str.contains("\"100\""));
        assert!(vmlist_str.contains("\"200\""));
        assert!(vmlist_str.contains("qemu"));
        assert!(vmlist_str.contains("lxc"));
        println!(
            "   ✅ .vmlist plugin: {} bytes, {} VMs",
            vmlist_str.len(),
            3
        );
    }

    // Test .rrd plugin
    if let Some(plugin) = plugins.get(".rrd") {
        let content = plugin.read()?;
        let rrd_str = String::from_utf8(content)?;
        // Should contain the node RRD data in C-compatible format
        assert!(
            rrd_str.contains("pve2-node/localhost"),
            "RRD should contain node data"
        );
    }

    // Test database persistence

    drop(db); // Close database

    // Reopen and verify data persists
    let db = MemDb::open(&db_path, false)?;
    assert!(db.exists("/corosync.conf")?);
    assert!(db.exists("/qemu-server/100.conf")?);
    assert!(db.exists("/lxc/200.conf")?);

    let read_conf = db.read("/corosync.conf", 0, 1024)?;
    assert_eq!(&read_conf[..], corosync_conf);

    // Test state export

    let all_entries = db.get_all_entries()?;

    // Verify entry structure
    let root_entry = db.lookup_path("/").expect("Root should exist");
    assert_eq!(root_entry.inode, 0); // Root inode is 0
    assert!(root_entry.is_dir());

    println!("\n=== Single-Node Test Complete ===\n");
    println!("\nTest Summary:");
    println!("\nDatabase Statistics:");
    println!("  • Total entries: {}", all_entries.len());
    println!("  • VMs/CTs tracked: {}", vmlist.len());
    println!("  • RRD entries: {}", num_entries);
    println!("  • Cluster log entries: 1");
    println!(
        "  • Database size: {} bytes",
        std::fs::metadata(&db_path)?.len()
    );

    Ok(())
}

/// Test simulated multi-operation workflow
#[tokio::test]
async fn test_realistic_workflow() -> Result<()> {
    println!("\n=== Realistic Workflow Test ===\n");

    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("pmxcfs.db");
    let db = MemDb::open(&db_path, true)?;

    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

    // Clear any VMs from previous tests
    let existing_vms: Vec<u32> = status.get_vmlist().keys().copied().collect();
    for vmid in existing_vms {
        status.delete_vm(vmid);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;

    println!("Scenario: Creating a new VM");

    // 1. Check if VMID is available
    let vmid = 103;
    assert!(!status.vm_exists(vmid));

    // 2. Acquire lock for VM creation
    let lock_path = format!("/priv/lock/qemu-server/{}.conf", vmid);
    let csum = [1u8; 32];

    // Create lock directories first
    db.create("/priv", libc::S_IFDIR, 0, now).ok();
    db.create("/priv/lock", libc::S_IFDIR, 0, now).ok();
    db.create("/priv/lock/qemu-server", libc::S_IFDIR, 0, now).ok();

    db.acquire_lock(&lock_path, &csum)?;

    // 3. Create VM configuration
    let config_path = format!("/qemu-server/{}.conf", vmid);
    db.create("/qemu-server", libc::S_IFDIR, 0, now).ok(); // May already exist
    let vm_config = format!(
        "name: test-vm-{}\ncores: 4\nmemory: 4096\nbootdisk: scsi0\n",
        vmid
    );
    db.create(&config_path, libc::S_IFREG, 0, now)?;
    db.write(&config_path, 0, 0, now, vm_config.as_bytes(), false)?;

    // 4. Register VM in cluster
    status.register_vm(vmid, VmType::Qemu, "localhost".to_string());

    // 5. Release lock
    db.release_lock(&lock_path, &csum)?;

    // 6. Verify VM is accessible
    assert!(db.exists(&config_path)?);
    assert!(status.vm_exists(vmid));

    Ok(())
}
