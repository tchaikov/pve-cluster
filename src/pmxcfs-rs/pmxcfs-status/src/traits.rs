use crate::types::{ClusterInfo, ClusterLogEntry, NodeStatus};
use anyhow::Result;
use parking_lot::RwLock;
use pmxcfs_api_types::{VmEntry, VmType};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Traits for Status operations to enable mocking and testing
///
/// Boxed future type for async trait methods
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Trait for Status operations
///
/// This trait abstracts all Status operations to enable:
/// - Dependency injection in production code
/// - Easy mocking in unit tests
/// - Test isolation without global singleton
///
/// The real `Status` struct implements this trait for production use.
/// `MockStatus` implements this trait for testing.
pub trait StatusOps: Send + Sync {
    // Node status operations
    fn get_node_status(&self, name: &str) -> Option<NodeStatus>;
    fn set_node_status<'a>(&'a self, name: String, data: Vec<u8>) -> BoxFuture<'a, Result<()>>;

    // Cluster log operations
    fn add_log_entry(&self, entry: ClusterLogEntry);
    fn get_log_entries(&self, max: usize) -> Vec<ClusterLogEntry>;
    fn clear_cluster_log(&self);
    fn add_cluster_log(&self, timestamp: u32, priority: u8, tag: String, node: String, msg: String);
    fn get_cluster_log_state(&self) -> Result<Vec<u8>>;
    fn merge_cluster_log_states(&self, states: &[pmxcfs_api_types::NodeSyncInfo]) -> Result<()>;
    fn add_remote_cluster_log(
        &self,
        time: u32,
        priority: u8,
        node: String,
        ident: String,
        tag: String,
        message: String,
    ) -> Result<()>;

    // RRD operations
    fn set_rrd_data<'a>(&'a self, key: String, data: String) -> BoxFuture<'a, Result<()>>;
    fn remove_old_rrd_data(&self);
    fn get_rrd_dump(&self) -> String;

    // VM list operations
    fn register_vm(&self, vmid: u32, vmtype: VmType, node: String);
    fn delete_vm(&self, vmid: u32);
    fn vm_exists(&self, vmid: u32) -> bool;
    fn different_vm_exists(&self, vmid: u32, vmtype: VmType, node: &str) -> bool;
    fn get_vmlist(&self) -> HashMap<u32, VmEntry>;
    fn scan_vmlist(&self, memdb: &pmxcfs_memdb::MemDb);

    // Cluster info operations
    fn init_cluster(&self, cluster_name: String);
    fn register_node(&self, node_id: u32, name: String, ip: String);
    fn get_cluster_info(&self) -> Option<ClusterInfo>;
    fn get_cluster_version(&self) -> u64;
    fn increment_cluster_version(&self);
    fn update_cluster_info(
        &self,
        cluster_name: String,
        config_version: u64,
        nodes: Vec<(u32, String, String)>,
    ) -> Result<()>;
    fn set_node_online(&self, node_id: u32, online: bool);

    // Quorum operations
    fn is_quorate(&self) -> bool;
    fn set_quorate(&self, quorate: bool);

    // Members operations
    fn get_members(&self) -> Vec<pmxcfs_api_types::MemberInfo>;
    fn update_members(&self, members: Vec<pmxcfs_api_types::MemberInfo>);
    fn update_member_status(&self, member_list: &[u32]);

    // Version/timestamp operations
    fn get_start_time(&self) -> u64;
    fn increment_vmlist_version(&self);
    fn get_vmlist_version(&self) -> u64;
    fn increment_path_version(&self, path: &str);
    fn get_path_version(&self, path: &str) -> u64;
    fn get_all_path_versions(&self) -> HashMap<String, u64>;
    fn increment_all_path_versions(&self);

    // KV store operations
    fn set_node_kv(&self, nodeid: u32, key: String, value: Vec<u8>);
    fn get_node_kv(&self, nodeid: u32, key: &str) -> Option<Vec<u8>>;
}

/// Mock implementation of StatusOps for testing
///
/// This provides a lightweight, isolated Status implementation for unit tests.
/// Unlike the real Status, MockStatus:
/// - Can be created independently without global singleton
/// - Has no RRD writer or async dependencies
/// - Is completely isolated between test instances
/// - Can be easily reset or configured for specific test scenarios
///
/// # Example
/// ```
/// use pmxcfs_status::{MockStatus, StatusOps};
/// use std::sync::Arc;
///
/// # fn test_example() {
/// let status: Arc<dyn StatusOps> = Arc::new(MockStatus::new());
/// status.set_quorate(true);
/// assert!(status.is_quorate());
/// # }
/// ```
pub struct MockStatus {
    vmlist: RwLock<HashMap<u32, VmEntry>>,
    quorate: RwLock<bool>,
    cluster_info: RwLock<Option<ClusterInfo>>,
    members: RwLock<Vec<pmxcfs_api_types::MemberInfo>>,
    cluster_version: Arc<std::sync::atomic::AtomicU64>,
    vmlist_version: Arc<std::sync::atomic::AtomicU64>,
    path_versions: RwLock<HashMap<String, u64>>,
    kvstore: RwLock<HashMap<u32, HashMap<String, Vec<u8>>>>,
    cluster_log: RwLock<Vec<ClusterLogEntry>>,
    rrd_data: RwLock<HashMap<String, String>>,
    node_status: RwLock<HashMap<String, NodeStatus>>,
    start_time: u64,
}

impl MockStatus {
    /// Create a new MockStatus instance for testing
    pub fn new() -> Self {
        Self {
            vmlist: RwLock::new(HashMap::new()),
            quorate: RwLock::new(false),
            cluster_info: RwLock::new(None),
            members: RwLock::new(Vec::new()),
            cluster_version: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            vmlist_version: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            path_versions: RwLock::new(HashMap::new()),
            kvstore: RwLock::new(HashMap::new()),
            cluster_log: RwLock::new(Vec::new()),
            rrd_data: RwLock::new(HashMap::new()),
            node_status: RwLock::new(HashMap::new()),
            start_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    /// Reset all mock state (useful for test cleanup)
    pub fn reset(&self) {
        self.vmlist.write().clear();
        *self.quorate.write() = false;
        *self.cluster_info.write() = None;
        self.members.write().clear();
        self.cluster_version
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.vmlist_version
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.path_versions.write().clear();
        self.kvstore.write().clear();
        self.cluster_log.write().clear();
        self.rrd_data.write().clear();
        self.node_status.write().clear();
    }
}

impl Default for MockStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusOps for MockStatus {
    fn get_node_status(&self, name: &str) -> Option<NodeStatus> {
        self.node_status.read().get(name).cloned()
    }

    fn set_node_status<'a>(&'a self, name: String, data: Vec<u8>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Simplified mock - just store the data
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            self.node_status.write().insert(
                name.clone(),
                NodeStatus {
                    name,
                    data,
                    timestamp: now,
                },
            );
            Ok(())
        })
    }

    fn add_log_entry(&self, entry: ClusterLogEntry) {
        self.cluster_log.write().push(entry);
    }

    fn get_log_entries(&self, max: usize) -> Vec<ClusterLogEntry> {
        let log = self.cluster_log.read();
        log.iter().take(max).cloned().collect()
    }

    fn clear_cluster_log(&self) {
        self.cluster_log.write().clear();
    }

    fn add_cluster_log(
        &self,
        timestamp: u32,
        priority: u8,
        tag: String,
        node: String,
        msg: String,
    ) {
        let entry = ClusterLogEntry {
            uid: 0,
            timestamp: timestamp as u64,
            priority,
            tag,
            pid: 0,
            node,
            ident: "mock".to_string(),
            message: msg,
        };
        self.add_log_entry(entry);
    }

    fn get_cluster_log_state(&self) -> Result<Vec<u8>> {
        // Simplified mock
        Ok(Vec::new())
    }

    fn merge_cluster_log_states(&self, _states: &[pmxcfs_api_types::NodeSyncInfo]) -> Result<()> {
        // Simplified mock
        Ok(())
    }

    fn add_remote_cluster_log(
        &self,
        time: u32,
        priority: u8,
        node: String,
        ident: String,
        tag: String,
        message: String,
    ) -> Result<()> {
        let entry = ClusterLogEntry {
            uid: 0,
            timestamp: time as u64,
            priority,
            tag,
            pid: 0,
            node,
            ident,
            message,
        };
        self.add_log_entry(entry);
        Ok(())
    }

    fn set_rrd_data<'a>(&'a self, key: String, data: String) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.rrd_data.write().insert(key, data);
            Ok(())
        })
    }

    fn remove_old_rrd_data(&self) {
        // Mock does nothing
    }

    fn get_rrd_dump(&self) -> String {
        let data = self.rrd_data.read();
        data.iter().map(|(k, v)| format!("{k}: {v}\n")).collect()
    }

    fn register_vm(&self, vmid: u32, vmtype: VmType, node: String) {
        // Get existing version or start at 1
        let version = self
            .vmlist
            .read()
            .get(&vmid)
            .map(|vm| vm.version + 1)
            .unwrap_or(1);

        self.vmlist.write().insert(
            vmid,
            VmEntry {
                vmtype,
                node,
                vmid,
                version,
            },
        );
        self.increment_vmlist_version();
    }

    fn delete_vm(&self, vmid: u32) {
        self.vmlist.write().remove(&vmid);
        self.increment_vmlist_version();
    }

    fn vm_exists(&self, vmid: u32) -> bool {
        self.vmlist.read().contains_key(&vmid)
    }

    fn different_vm_exists(&self, vmid: u32, vmtype: VmType, node: &str) -> bool {
        if let Some(entry) = self.vmlist.read().get(&vmid) {
            entry.vmtype != vmtype || entry.node != node
        } else {
            false
        }
    }

    fn get_vmlist(&self) -> HashMap<u32, VmEntry> {
        self.vmlist.read().clone()
    }

    fn scan_vmlist(&self, _memdb: &pmxcfs_memdb::MemDb) {
        // Mock does nothing - real implementation scans /qemu-server and /lxc
    }

    fn init_cluster(&self, cluster_name: String) {
        *self.cluster_info.write() = Some(ClusterInfo {
            cluster_name,
            config_version: 0,
            nodes_by_id: HashMap::new(),
            nodes_by_name: HashMap::new(),
        });
        self.increment_cluster_version();
    }

    fn register_node(&self, node_id: u32, name: String, ip: String) {
        let mut info = self.cluster_info.write();
        if let Some(cluster) = info.as_mut() {
            let node = crate::types::ClusterNode {
                name: name.clone(),
                node_id,
                ip,
                online: false, // Match real Status behavior - updated by cluster module
            };
            cluster.add_node(node);
        }
        self.increment_cluster_version();
    }

    fn get_cluster_info(&self) -> Option<ClusterInfo> {
        self.cluster_info.read().clone()
    }

    fn get_cluster_version(&self) -> u64 {
        self.cluster_version
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn increment_cluster_version(&self) {
        self.cluster_version
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn update_cluster_info(
        &self,
        cluster_name: String,
        config_version: u64,
        nodes: Vec<(u32, String, String)>,
    ) -> Result<()> {
        let mut cluster_info = self.cluster_info.write();

        // Create or update cluster info
        let mut info = cluster_info.take().unwrap_or_else(|| ClusterInfo {
            cluster_name: cluster_name.clone(),
            config_version,
            nodes_by_id: HashMap::new(),
            nodes_by_name: HashMap::new(),
        });

        // Update cluster name if changed
        if info.cluster_name != cluster_name {
            info.cluster_name = cluster_name;
        }

        // Clear existing nodes
        info.nodes_by_id.clear();
        info.nodes_by_name.clear();

        // Add updated nodes
        for (nodeid, name, ip) in nodes {
            let node = crate::types::ClusterNode {
                name,
                node_id: nodeid,
                ip,
                online: false,
            };
            info.add_node(node);
        }

        *cluster_info = Some(info);

        // Update version to reflect configuration change
        self.cluster_version
            .store(config_version, std::sync::atomic::Ordering::SeqCst);

        Ok(())
    }

    fn set_node_online(&self, node_id: u32, online: bool) {
        let mut info = self.cluster_info.write();
        if let Some(cluster) = info.as_mut()
            && let Some(node) = cluster.nodes_by_id.get_mut(&node_id)
        {
            node.online = online;
        }
    }

    fn is_quorate(&self) -> bool {
        *self.quorate.read()
    }

    fn set_quorate(&self, quorate: bool) {
        *self.quorate.write() = quorate;
    }

    fn get_members(&self) -> Vec<pmxcfs_api_types::MemberInfo> {
        self.members.read().clone()
    }

    fn update_members(&self, members: Vec<pmxcfs_api_types::MemberInfo>) {
        *self.members.write() = members;
    }

    fn update_member_status(&self, _member_list: &[u32]) {
        // Mock does nothing - real implementation updates online status
    }

    fn get_start_time(&self) -> u64 {
        self.start_time
    }

    fn increment_vmlist_version(&self) {
        self.vmlist_version
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn get_vmlist_version(&self) -> u64 {
        self.vmlist_version
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn increment_path_version(&self, path: &str) {
        let mut versions = self.path_versions.write();
        let version = versions.entry(path.to_string()).or_insert(0);
        *version += 1;
    }

    fn get_path_version(&self, path: &str) -> u64 {
        *self.path_versions.read().get(path).unwrap_or(&0)
    }

    fn get_all_path_versions(&self) -> HashMap<String, u64> {
        self.path_versions.read().clone()
    }

    fn increment_all_path_versions(&self) {
        let mut versions = self.path_versions.write();
        for version in versions.values_mut() {
            *version += 1;
        }
    }

    fn set_node_kv(&self, nodeid: u32, key: String, value: Vec<u8>) {
        let mut kvstore = self.kvstore.write();
        let node_kv = kvstore.entry(nodeid).or_default();

        // Remove entry if value is empty (matches real Status behavior)
        if value.is_empty() {
            node_kv.remove(&key);
        } else {
            node_kv.insert(key, value);
        }
    }

    fn get_node_kv(&self, nodeid: u32, key: &str) -> Option<Vec<u8>> {
        self.kvstore.read().get(&nodeid)?.get(key).cloned()
    }
}
