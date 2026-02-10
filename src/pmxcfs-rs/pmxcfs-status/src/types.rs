/// Data types for the status module
use std::collections::HashMap;

/// Cluster node information (matches C implementation's cfs_clnode_t)
#[derive(Debug, Clone)]
pub struct ClusterNode {
    pub name: String,
    pub node_id: u32,
    pub ip: String,
    pub online: bool,
}

/// Cluster information (matches C implementation's cfs_clinfo_t)
#[derive(Debug, Clone)]
pub struct ClusterInfo {
    pub cluster_name: String,
    /// Configuration version from corosync (matches C's cman_version)
    pub config_version: u64,
    pub nodes_by_id: HashMap<u32, ClusterNode>,
    /// Index mapping node name to node_id (safer than duplicating ClusterNode)
    pub nodes_by_name: HashMap<String, u32>,
}

impl ClusterInfo {
    pub(crate) fn new(cluster_name: String, config_version: u64) -> Self {
        Self {
            cluster_name,
            config_version,
            nodes_by_id: HashMap::new(),
            nodes_by_name: HashMap::new(),
        }
    }

    /// Add or update a node in the cluster
    pub(crate) fn add_node(&mut self, node: ClusterNode) {
        let node_id = node.node_id;
        let name = node.name.clone();
        self.nodes_by_id.insert(node_id, node);
        self.nodes_by_name.insert(name, node_id);
    }

    /// Get node by name
    pub fn get_node_by_name(&self, name: &str) -> Option<&ClusterNode> {
        let node_id = self.nodes_by_name.get(name)?;
        self.nodes_by_id.get(node_id)
    }
}

/// Node status data
#[derive(Clone, Debug)]
pub struct NodeStatus {
    pub name: String,
    pub data: Vec<u8>,
    pub timestamp: u64,
}

/// Cluster log entry
/// Field order matches C output: uid, time, pri, tag, pid, node, user, msg
#[derive(Clone, Debug)]
pub struct ClusterLogEntry {
    pub uid: u32,
    pub timestamp: u64,
    pub priority: u8,
    pub tag: String,
    pub pid: u32,
    pub node: String,
    pub ident: String,
    pub message: String,
}

/// RRD (Round Robin Database) entry
#[derive(Clone, Debug)]
pub(crate) struct RrdEntry {
    pub key: String,
    pub data: String,
    pub timestamp: u64,
}
