//! Test utilities for pmxcfs integration and unit tests
//!
//! This crate provides:
//! - Common test setup and helper functions
//! - TestEnv builder for standard test configurations
//! - Mock implementations (MockStatus, MockMemDb for isolated testing)
//! - Test constants and utilities

use anyhow::Result;
use pmxcfs_config::Config;
use pmxcfs_memdb::MemDb;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// Re-export MockStatus for easy test access
pub use pmxcfs_status::{MockStatus, StatusOps};

// Mock implementations
mod mock_memdb;
pub use mock_memdb::MockMemDb;

// Re-export MemDbOps for convenience in tests
pub use pmxcfs_memdb::MemDbOps;

// Test constants
pub const TEST_MTIME: u32 = 1234567890;
pub const TEST_NODE_NAME: &str = "testnode";
pub const TEST_CLUSTER_NAME: &str = "test-cluster";
pub const TEST_WWW_DATA_GID: u32 = 33;

/// Test environment builder for standard test setups
///
/// This builder provides a fluent interface for creating test environments
/// with optional components (database, status, config).
///
/// # Example
/// ```
/// use pmxcfs_test_utils::TestEnv;
///
/// # fn example() -> anyhow::Result<()> {
/// let env = TestEnv::new()
///     .with_database()?
///     .with_mock_status()
///     .build();
///
/// // Use env.db, env.status, etc.
/// # Ok(())
/// # }
/// ```
pub struct TestEnv {
    pub config: Arc<Config>,
    pub db: Option<MemDb>,
    pub status: Option<Arc<dyn StatusOps>>,
    pub temp_dir: Option<TempDir>,
}

impl TestEnv {
    /// Create a new test environment builder with default config
    pub fn new() -> Self {
        Self::new_with_config(false)
    }

    /// Create a new test environment builder with local mode config
    pub fn new_local() -> Self {
        Self::new_with_config(true)
    }

    /// Create a new test environment builder with custom local_mode setting
    pub fn new_with_config(local_mode: bool) -> Self {
        let config = create_test_config(local_mode);
        Self {
            config,
            db: None,
            status: None,
            temp_dir: None,
        }
    }

    /// Add a database with standard directory structure
    pub fn with_database(mut self) -> Result<Self> {
        let (temp_dir, db) = create_test_db()?;
        self.temp_dir = Some(temp_dir);
        self.db = Some(db);
        Ok(self)
    }

    /// Add a minimal database (no standard directories)
    pub fn with_minimal_database(mut self) -> Result<Self> {
        let (temp_dir, db) = create_minimal_test_db()?;
        self.temp_dir = Some(temp_dir);
        self.db = Some(db);
        Ok(self)
    }

    /// Add a MockStatus instance for isolated testing
    pub fn with_mock_status(mut self) -> Self {
        self.status = Some(Arc::new(MockStatus::new()));
        self
    }

    /// Add the real Status instance with test config
    pub fn with_status(mut self) -> Self {
        self.status = Some(pmxcfs_status::init_with_config(self.config.clone()));
        self
    }

    /// Build and return the test environment
    pub fn build(self) -> Self {
        self
    }

    /// Get a reference to the database (panics if not configured)
    pub fn db(&self) -> &MemDb {
        self.db
            .as_ref()
            .expect("Database not configured. Call with_database() first")
    }

    /// Get a reference to the status (panics if not configured)
    pub fn status(&self) -> &Arc<dyn StatusOps> {
        self.status
            .as_ref()
            .expect("Status not configured. Call with_status() or with_mock_status() first")
    }
}

impl Default for TestEnv {
    fn default() -> Self {
        Self::new()
    }
}

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
pub fn create_minimal_test_db() -> Result<(TempDir, MemDb)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let db = MemDb::open(&db_path, true)?;
    Ok((temp_dir, db))
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
/// * `vm_type` - "qemu-server" or "lxc"
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

/// Clears all VMs from a status instance
///
/// Useful for ensuring clean state before tests that register VMs.
///
/// # Arguments
/// * `status` - The status instance to clear
pub fn clear_test_vms(status: &dyn StatusOps) {
    let existing_vms: Vec<u32> = status.get_vmlist().keys().copied().collect();
    for vmid in existing_vms {
        status.delete_vm(vmid);
    }
}

/// Wait for a condition to become true, polling at regular intervals
///
/// This is a replacement for sleep-based synchronization in integration tests.
/// Instead of sleeping for an arbitrary duration and hoping the condition is met,
/// this function polls the condition and returns as soon as it becomes true.
///
/// # Arguments
/// * `predicate` - Function that returns true when the condition is met
/// * `timeout` - Maximum time to wait for the condition
/// * `check_interval` - How often to check the condition
///
/// # Returns
/// * `true` if condition was met within timeout
/// * `false` if timeout was reached without condition being met
///
/// # Example
/// ```no_run
/// use pmxcfs_test_utils::wait_for_condition;
/// use std::time::Duration;
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use std::sync::Arc;
///
/// # async fn example() {
/// let ready = Arc::new(AtomicBool::new(false));
///
/// // Wait for service to be ready (with timeout)
/// let result = wait_for_condition(
///     || ready.load(Ordering::SeqCst),
///     Duration::from_secs(5),
///     Duration::from_millis(10),
/// ).await;
///
/// assert!(result, "Service should be ready within 5 seconds");
/// # }
/// ```
pub async fn wait_for_condition<F>(
    predicate: F,
    timeout: Duration,
    check_interval: Duration,
) -> bool
where
    F: Fn() -> bool,
{
    let start = Instant::now();
    loop {
        if predicate() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(check_interval).await;
    }
}

/// Wait for a condition with a custom error message
///
/// Similar to `wait_for_condition`, but returns a Result with a custom error message
/// if the timeout is reached.
///
/// # Arguments
/// * `predicate` - Function that returns true when the condition is met
/// * `timeout` - Maximum time to wait for the condition
/// * `check_interval` - How often to check the condition
/// * `error_msg` - Error message to return if timeout is reached
///
/// # Returns
/// * `Ok(())` if condition was met within timeout
/// * `Err(anyhow::Error)` with custom message if timeout was reached
///
/// # Example
/// ```no_run
/// use pmxcfs_test_utils::wait_for_condition_or_fail;
/// use std::time::Duration;
/// use std::sync::atomic::{AtomicU64, Ordering};
/// use std::sync::Arc;
///
/// # async fn example() -> anyhow::Result<()> {
/// let counter = Arc::new(AtomicU64::new(0));
///
/// wait_for_condition_or_fail(
///     || counter.load(Ordering::SeqCst) >= 1,
///     Duration::from_secs(5),
///     Duration::from_millis(10),
///     "Service should initialize within 5 seconds",
/// ).await?;
///
/// # Ok(())
/// # }
/// ```
pub async fn wait_for_condition_or_fail<F>(
    predicate: F,
    timeout: Duration,
    check_interval: Duration,
    error_msg: &str,
) -> Result<()>
where
    F: Fn() -> bool,
{
    if wait_for_condition(predicate, timeout, check_interval).await {
        Ok(())
    } else {
        anyhow::bail!("{}", error_msg)
    }
}

/// Blocking version of wait_for_condition for synchronous tests
///
/// Similar to `wait_for_condition`, but works in synchronous contexts.
/// Polls the condition and returns as soon as it becomes true or timeout is reached.
///
/// # Arguments
/// * `predicate` - Function that returns true when the condition is met
/// * `timeout` - Maximum time to wait for the condition
/// * `check_interval` - How often to check the condition
///
/// # Returns
/// * `true` if condition was met within timeout
/// * `false` if timeout was reached without condition being met
///
/// # Example
/// ```no_run
/// use pmxcfs_test_utils::wait_for_condition_blocking;
/// use std::time::Duration;
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use std::sync::Arc;
///
/// let ready = Arc::new(AtomicBool::new(false));
///
/// // Wait for service to be ready (with timeout)
/// let result = wait_for_condition_blocking(
///     || ready.load(Ordering::SeqCst),
///     Duration::from_secs(5),
///     Duration::from_millis(10),
/// );
///
/// assert!(result, "Service should be ready within 5 seconds");
/// ```
pub fn wait_for_condition_blocking<F>(
    predicate: F,
    timeout: Duration,
    check_interval: Duration,
) -> bool
where
    F: Fn() -> bool,
{
    let start = Instant::now();
    loop {
        if predicate() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(check_interval);
    }
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

    #[test]
    fn test_env_builder_basic() {
        let env = TestEnv::new().build();
        assert_eq!(env.config.nodename(), TEST_NODE_NAME);
        assert!(env.db.is_none());
        assert!(env.status.is_none());
    }

    #[test]
    fn test_env_builder_with_database() -> Result<()> {
        let env = TestEnv::new().with_database()?.build();
        assert!(env.db.is_some());
        assert!(env.db().exists("/nodes")?);
        Ok(())
    }

    #[test]
    fn test_env_builder_with_mock_status() {
        let env = TestEnv::new().with_mock_status().build();
        assert!(env.status.is_some());

        // Test that MockStatus works
        let status = env.status();
        status.set_quorate(true);
        assert!(status.is_quorate());
    }

    #[test]
    fn test_env_builder_full() -> Result<()> {
        let env = TestEnv::new().with_database()?.with_mock_status().build();

        assert!(env.db.is_some());
        assert!(env.status.is_some());
        assert!(env.config.nodename() == TEST_NODE_NAME);

        Ok(())
    }

    // NOTE: Tokio tests for wait_for_condition functions are REMOVED because they
    // cause the test runner to hang when running `cargo test --lib --workspace`.
    // Root cause: tokio multi-threaded runtime doesn't shut down properly when
    // these async tests complete, blocking the entire test suite.
    //
    // These utility functions work correctly and are verified in integration tests
    // that actually use them (e.g., integration-tests/).
}
