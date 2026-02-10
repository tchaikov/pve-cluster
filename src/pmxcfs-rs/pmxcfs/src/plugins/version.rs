/// .version Plugin - Cluster Version Information
///
/// This plugin provides comprehensive version information in JSON format:
/// {
///   "starttime": 1234567890,
///   "clinfo": 5,
///   "vmlist": 12,
///   "qemu-server": 3,
///   "lxc": 2,
///   "nodes": 1
/// }
///
/// All version counters are now maintained in the Status module (status/mod.rs)
/// to match the C implementation where they are stored in cfs_status.
use pmxcfs_config::Config;
use pmxcfs_status::Status;
use serde_json::json;
use std::sync::Arc;

use super::Plugin;

/// Version plugin - provides cluster version information
pub struct VersionPlugin {
    config: Arc<Config>,
    status: Arc<Status>,
}

impl VersionPlugin {
    pub fn new(config: Arc<Config>, status: Arc<Status>) -> Self {
        Self { config, status }
    }

    /// Generate version information content
    fn generate_content(&self) -> String {
        // Get cluster state from status (matches C's cfs_status access)
        let members = self.status.get_members();
        let quorate = self.status.is_quorate();

        // Count unique nodes
        let mut unique_nodes = std::collections::HashSet::new();
        for member in &members {
            unique_nodes.insert(member.node_id);
        }
        let node_count = unique_nodes.len().max(1); // At least 1 (ourselves)

        // Build base response with all version counters
        let mut response = serde_json::Map::new();

        // Basic version info
        response.insert("version".to_string(), json!(env!("CARGO_PKG_VERSION")));
        response.insert("api".to_string(), json!(1));

        // Daemon start time (from Status)
        response.insert("starttime".to_string(), json!(self.status.get_start_time()));

        // Cluster info version (from Status)
        response.insert(
            "clinfo".to_string(),
            json!(self.status.get_cluster_version()),
        );

        // VM list version (from Status)
        response.insert(
            "vmlist".to_string(),
            json!(self.status.get_vmlist_version()),
        );

        // MemDB path versions (from Status)
        // These are the paths that clients commonly monitor for changes
        let path_versions = self.status.get_all_path_versions();
        for (path, version) in path_versions {
            if version > 0 {
                response.insert(path, json!(version));
            }
        }

        // Cluster info (legacy format for compatibility)
        response.insert(
            "cluster".to_string(),
            json!({
                "name": self.config.cluster_name(),
                "nodes": node_count,
                "quorate": if quorate { 1 } else { 0 }
            }),
        );

        serde_json::Value::Object(response).to_string()
    }
}

impl Plugin for VersionPlugin {
    fn name(&self) -> &str {
        ".version"
    }

    fn read(&self) -> anyhow::Result<Vec<u8>> {
        Ok(self.generate_content().into_bytes())
    }

    fn mode(&self) -> u32 {
        0o440
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_version_format() {
        // Create Status instance without RRD persistence (not needed for test)
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

        // Create Config instance
        let config = Arc::new(Config::new(
            "testnode".to_string(),
            "127.0.0.1".parse().unwrap(),
            33,
            false,
            false,
            "testcluster".to_string(),
        ));

        // Initialize cluster
        status.init_cluster("testcluster".to_string());

        let plugin = VersionPlugin::new(config, status);
        let result = plugin.generate_content();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Should have version
        assert!(parsed["version"].is_string());

        // Should have api
        assert_eq!(parsed["api"], 1);

        // Should have starttime
        assert!(parsed["starttime"].is_number());

        // Should have clinfo and vmlist
        assert!(parsed["clinfo"].is_number());
        assert!(parsed["vmlist"].is_number());

        // Should have cluster info
        assert_eq!(parsed["cluster"]["name"], "testcluster");
        assert!(parsed["cluster"]["nodes"].is_number());
        assert!(parsed["cluster"]["quorate"].is_number());
    }

    #[tokio::test]
    async fn test_increment_versions() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

        let initial_clinfo = status.get_cluster_version();
        status.increment_cluster_version();
        assert_eq!(status.get_cluster_version(), initial_clinfo + 1);

        let initial_vmlist = status.get_vmlist_version();
        status.increment_vmlist_version();
        assert_eq!(status.get_vmlist_version(), initial_vmlist + 1);
    }

    #[tokio::test]
    async fn test_path_versions() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

        // Use actual paths from memdb_change_array
        status.increment_path_version("corosync.conf");
        status.increment_path_version("corosync.conf");
        assert!(status.get_path_version("corosync.conf") >= 2);

        status.increment_path_version("user.cfg");
        assert!(status.get_path_version("user.cfg") >= 1);
    }
}
