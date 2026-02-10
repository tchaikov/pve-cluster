/// .members Plugin - Cluster Member Information
///
/// This plugin provides information about cluster members in JSON format:
/// {
///   "nodename": "node1",
///   "version": 5,
///   "cluster": {
///     "name": "mycluster",
///     "version": 1,
///     "nodes": 3,
///     "quorate": 1
///   },
///   "nodelist": {
///     "node1": { "id": 1, "online": 1, "ip": "192.168.1.10" },
///     "node2": { "id": 2, "online": 1, "ip": "192.168.1.11" }
///   }
/// }
use pmxcfs_config::Config;
use pmxcfs_status::Status;
use serde_json::json;
use std::sync::Arc;

use super::Plugin;

/// Members plugin - provides cluster member information
pub struct MembersPlugin {
    config: Arc<Config>,
    status: Arc<Status>,
}

impl MembersPlugin {
    pub fn new(config: Arc<Config>, status: Arc<Status>) -> Self {
        Self { config, status }
    }

    /// Generate members information content
    fn generate_content(&self) -> String {
        let nodename = self.config.nodename();
        let cluster_name = self.config.cluster_name();

        // Get cluster info from status (matches C's cfs_status access)
        let cluster_info = self.status.get_cluster_info();
        let cluster_version = self.status.get_cluster_version();

        // Get quorum status and members from status
        let quorate = self.status.is_quorate();

        // Get cluster members (for online status tracking)
        let members = self.status.get_members();

        // Create a set of online node IDs from current members
        let mut online_nodes = std::collections::HashSet::new();
        for member in &members {
            online_nodes.insert(member.node_id);
        }

        // Count unique nodes
        let node_count = online_nodes.len();

        // Build nodelist from cluster_info
        let mut nodelist = serde_json::Map::new();

        if let Some(cluster_info) = cluster_info {
            // Add all registered nodes to nodelist
            for (name, node_id) in &cluster_info.nodes_by_name {
                if let Some(node) = cluster_info.nodes_by_id.get(node_id) {
                    let is_online = online_nodes.contains(&node.node_id);
                    let node_info = json!({
                        "id": node.node_id,
                        "online": if is_online { 1 } else { 0 },
                        "ip": node.ip
                    });
                    nodelist.insert(name.clone(), node_info);
                }
            }

            // Build the complete response
            let response = json!({
                "nodename": nodename,
                "version": cluster_version,
                "cluster": {
                    "name": cluster_info.cluster_name,
                    "version": 1,  // Cluster format version (always 1)
                    "nodes": node_count.max(1),  // At least 1 (ourselves)
                    "quorate": if quorate { 1 } else { 0 }
                },
                "nodelist": nodelist
            });

            response.to_string()
        } else {
            // No cluster info yet, return minimal response with just local node
            let node_info = json!({
                "id": 0,  // Unknown ID
                "online": 1,  // Assume online since we're running
                "ip": self.config.node_ip()
            });

            let mut nodelist = serde_json::Map::new();
            nodelist.insert(nodename.to_string(), node_info);

            let response = json!({
                "nodename": nodename,
                "version": cluster_version,
                "cluster": {
                    "name": cluster_name,
                    "version": 1,
                    "nodes": 1,
                    "quorate": if quorate { 1 } else { 0 }
                },
                "nodelist": nodelist
            });

            response.to_string()
        }
    }
}

impl Plugin for MembersPlugin {
    fn name(&self) -> &str {
        ".members"
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
    async fn test_members_format() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

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

        let plugin = MembersPlugin::new(config, status);
        let result = plugin.generate_content();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Should have nodename
        assert_eq!(parsed["nodename"], "testnode");

        // Should have version
        assert!(parsed["version"].is_number());

        // Should have cluster info
        assert_eq!(parsed["cluster"]["name"], "testcluster");
        assert!(parsed["cluster"]["nodes"].is_number());
        assert!(parsed["cluster"]["quorate"].is_number());

        // Should have nodelist (might be empty without actual cluster members)
        assert!(parsed["nodelist"].is_object());
    }

    #[tokio::test]
    async fn test_members_no_cluster() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

        let config = Arc::new(Config::new(
            "standalone".to_string(),
            "192.168.1.100".parse().unwrap(),
            33,
            false,
            false,
            "testcluster".to_string(),
        ));

        // Don't set cluster info - should still work
        let plugin = MembersPlugin::new(config, status);
        let result = plugin.generate_content();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Should have minimal response
        assert_eq!(parsed["nodename"], "standalone");
        assert!(parsed["cluster"].is_object());
        assert!(parsed["nodelist"].is_object());
        assert!(parsed["nodelist"]["standalone"].is_object());
    }
}
