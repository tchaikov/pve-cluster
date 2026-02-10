//! Common test utilities for pmxcfs integration tests
//!
//! This module provides shared test setup and helper functions to ensure
//! consistency across all integration tests and reduce code duplication.

use anyhow::Result;
use pmxcfs_config::Config;
use pmxcfs_memdb::MemDb;
use pmxcfs_status::Status;
use std::sync::Arc;
use tempfile::TempDir;

// Test constants
pub const TEST_MTIME: u32 = 1234567890;
pub const TEST_NODE_NAME: &str = "testnode";
pub const TEST_CLUSTER_NAME: &str = "test-cluster";
pub const TEST_WWW_DATA_GID: u32 = 33;

/// Creates a standard test configuration
///
/// # Arguments
/// * `local_mode` - Whether to run in local mode (no cluster)
///
/// # Returns
/// Arc-wrapped Config suitable for testing
pub fn create_test_config(local_mode: bool) -> Arc<Config> {
    Config::shared(
        TEST_NODE_NAME.to_string(),
        "127.0.0.1".parse().unwrap(),
        TEST_WWW_DATA_GID,
        false, // debug mode
        local_mode,
        TEST_CLUSTER_NAME.to_string(),
    )
}

/// Creates a test database with standard directory structure
///
/// Creates the following directories:
/// - /nodes/{nodename}/qemu-server
/// - /nodes/{nodename}/lxc
/// - /nodes/{nodename}/priv
/// - /priv/lock/qemu-server
/// - /priv/lock/lxc
/// - /qemu-server
/// - /lxc
///
/// # Returns
/// (TempDir, MemDb) - The temp directory must be kept alive for database to persist
pub fn create_test_db() -> Result<(TempDir, MemDb)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let db = MemDb::open(&db_path, true)?;

    // Create standard directory structure
    let now = TEST_MTIME;

    // Node-specific directories
    db.create("/nodes", libc::S_IFDIR, 0, now)?;
    db.create(&format!("/nodes/{}", TEST_NODE_NAME), libc::S_IFDIR, 0, now)?;
    db.create(
        &format!("/nodes/{}/qemu-server", TEST_NODE_NAME), libc::S_IFDIR, 0,
        now,
    )?;
    db.create(
        &format!("/nodes/{}/lxc", TEST_NODE_NAME), libc::S_IFDIR, 0,
        now,
    )?;
    db.create(
        &format!("/nodes/{}/priv", TEST_NODE_NAME), libc::S_IFDIR, 0,
        now,
    )?;

    // Global directories
    db.create("/priv", libc::S_IFDIR, 0, now)?;
    db.create("/priv/lock", libc::S_IFDIR, 0, now)?;
    db.create("/priv/lock/qemu-server", libc::S_IFDIR, 0, now)?;
    db.create("/priv/lock/lxc", libc::S_IFDIR, 0, now)?;
    db.create("/qemu-server", libc::S_IFDIR, 0, now)?;
    db.create("/lxc", libc::S_IFDIR, 0, now)?;

    Ok((temp_dir, db))
}

/// Creates a minimal test database (no standard directories)
///
/// Use this when you want full control over database structure
///
/// # Returns
/// (TempDir, MemDb) - The temp directory must be kept alive for database to persist
#[allow(dead_code)]
pub fn create_minimal_test_db() -> Result<(TempDir, MemDb)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let db = MemDb::open(&db_path, true)?;
    Ok((temp_dir, db))
}

/// Creates a test status instance
///
/// NOTE: This uses the global Status singleton. Be aware that tests using this
/// will share the same Status instance and may interfere with each other if run
/// in parallel. Consider running Status-dependent tests serially using:
/// `#[serial]` attribute from the `serial_test` crate.
///
/// # Returns
/// Arc-wrapped Status instance
pub fn create_test_status() -> Arc<Status> {
    pmxcfs_status::init()
}

/// Clears all VMs from the status subsystem
///
/// Useful for ensuring clean state before tests that register VMs.
///
/// # Arguments
/// * `status` - The status instance to clear
#[allow(dead_code)]
pub fn clear_test_vms(status: &Arc<Status>) {
    let existing_vms: Vec<u32> = status.get_vmlist().keys().copied().collect();
    for vmid in existing_vms {
        status.delete_vm(vmid);
    }
}

/// Creates test VM configuration content
///
/// # Arguments
/// * `vmid` - VM ID
/// * `cores` - Number of CPU cores
/// * `memory` - Memory in MB
///
/// # Returns
/// Configuration file content as bytes
#[allow(dead_code)]
pub fn create_vm_config(vmid: u32, cores: u32, memory: u32) -> Vec<u8> {
    format!(
        "name: test-vm-{}\ncores: {}\nmemory: {}\nbootdisk: scsi0\n",
        vmid, cores, memory
    )
    .into_bytes()
}

/// Creates test CT (container) configuration content
///
/// # Arguments
/// * `vmid` - Container ID
/// * `cores` - Number of CPU cores
/// * `memory` - Memory in MB
///
/// # Returns
/// Configuration file content as bytes
#[allow(dead_code)]
pub fn create_ct_config(vmid: u32, cores: u32, memory: u32) -> Vec<u8> {
    format!(
        "cores: {}\nmemory: {}\nrootfs: local:100/vm-{}-disk-0.raw\n",
        cores, memory, vmid
    )
    .into_bytes()
}

/// Creates a test lock path for a VM config
///
/// # Arguments
/// * `vmid` - VM ID
/// * `vm_type` - "qemu" or "lxc"
///
/// # Returns
/// Lock path in format `/priv/lock/{vm_type}/{vmid}.conf`
pub fn create_lock_path(vmid: u32, vm_type: &str) -> String {
    format!("/priv/lock/{}/{}.conf", vm_type, vmid)
}

/// Creates a test config path for a VM
///
/// # Arguments
/// * `vmid` - VM ID
/// * `vm_type` - "qemu-server" or "lxc"
///
/// # Returns
/// Config path in format `/{vm_type}/{vmid}.conf`
pub fn create_config_path(vmid: u32, vm_type: &str) -> String {
    format!("/{}/{}.conf", vm_type, vmid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_test_config() {
        let config = create_test_config(true);
        assert_eq!(config.nodename(), TEST_NODE_NAME);
        assert_eq!(config.cluster_name(), TEST_CLUSTER_NAME);
        assert!(config.is_local_mode());
    }

    #[test]
    fn test_create_test_db() -> Result<()> {
        let (_temp_dir, db) = create_test_db()?;

        // Verify standard directories exist
        assert!(db.exists("/nodes")?, "Should have /nodes");
        assert!(db.exists("/qemu-server")?, "Should have /qemu-server");
        assert!(db.exists("/priv/lock")?, "Should have /priv/lock");

        Ok(())
    }

    #[test]
    fn test_path_helpers() {
        assert_eq!(
            create_lock_path(100, "qemu-server"),
            "/priv/lock/qemu-server/100.conf"
        );
        assert_eq!(
            create_config_path(100, "qemu-server"),
            "/qemu-server/100.conf"
        );
    }
}
