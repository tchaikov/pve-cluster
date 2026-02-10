/// .clusterlog Plugin - Cluster Log Entries
///
/// This plugin provides cluster log entries in JSON format matching C implementation:
/// ```json
/// {
///   "data": [
///     {"uid": 1, "time": 1234567890, "pri": 6, "tag": "cluster", "pid": 0, "node": "node1", "user": "root", "msg": "starting cluster log"}
///   ]
/// }
/// ```
///
/// The format is compatible with the C implementation which uses clog_dump_json
/// to write JSON data to clients.
///
/// Default max_entries: 50 (matching C implementation)
use pmxcfs_status::Status;
use serde_json::json;
use std::sync::Arc;

use super::Plugin;

/// Clusterlog plugin - provides cluster log entries
pub struct ClusterlogPlugin {
    status: Arc<Status>,
    max_entries: usize,
}

impl ClusterlogPlugin {
    pub fn new(status: Arc<Status>) -> Self {
        Self {
            status,
            max_entries: 50,
        }
    }

    /// Create with custom entry limit
    #[allow(dead_code)] // Used in tests for custom entry limits
    pub fn new_with_limit(status: Arc<Status>, max_entries: usize) -> Self {
        Self {
            status,
            max_entries,
        }
    }

    /// Generate clusterlog content (C-compatible JSON format)
    fn generate_content(&self) -> String {
        let entries = self.status.get_log_entries(self.max_entries);

        // Convert to JSON format matching C implementation
        // C format: {"data": [{"uid": ..., "time": ..., "pri": ..., "tag": ..., "pid": ..., "node": ..., "user": ..., "msg": ...}]}
        let data: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                json!({
                    "uid": idx + 1,                    // Sequential ID starting from 1
                    "time": entry.timestamp,           // Unix timestamp
                    "pri": entry.priority,             // Priority level (numeric)
                    "tag": entry.tag,                  // Tag field
                    "pid": 0,                          // Process ID (we don't track this, set to 0)
                    "node": entry.node,                // Node name
                    "user": entry.ident,               // User/ident field
                    "msg": entry.message               // Log message
                })
            })
            .collect();

        let result = json!({
            "data": data
        });

        // Convert to JSON string with formatting
        serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
    }
}

impl Plugin for ClusterlogPlugin {
    fn name(&self) -> &str {
        ".clusterlog"
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
    use pmxcfs_status as status;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Test helper: add a log message to the cluster log
    fn add_log_message(
        status: &status::Status,
        node: String,
        priority: u8,
        ident: String,
        tag: String,
        message: String,
    ) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let entry = status::ClusterLogEntry {
            uid: 0,
            timestamp,
            priority,
            tag,
            pid: 0,
            node,
            ident,
            message,
        };
        status.add_log_entry(entry);
    }

    #[tokio::test]
    async fn test_clusterlog_format() {
        // Initialize status subsystem without RRD persistence (not needed for test)
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = status::init_with_config(config);

        // Test that it returns valid JSON
        let plugin = ClusterlogPlugin::new(status);
        let result = plugin.generate_content();

        // Should be valid JSON
        assert!(
            serde_json::from_str::<serde_json::Value>(&result).is_ok(),
            "Should return valid JSON"
        );
    }

    #[tokio::test]
    async fn test_clusterlog_with_entries() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = status::init_with_config(config);

        // Clear any existing log entries from other tests
        status.clear_cluster_log();

        // Add some log entries
        add_log_message(
            &status,
            "node1".to_string(),
            6, // Info priority
            "pmxcfs".to_string(),
            "cluster".to_string(),
            "Node joined cluster".to_string(),
        );

        add_log_message(
            &status,
            "node2".to_string(),
            4, // Warning priority
            "pvestatd".to_string(),
            "status".to_string(),
            "High load detected".to_string(),
        );

        // Get clusterlog
        let plugin = ClusterlogPlugin::new(status);
        let result = plugin.generate_content();

        // Parse JSON
        let json: serde_json::Value = serde_json::from_str(&result).expect("Should be valid JSON");

        // Verify structure
        assert!(json.get("data").is_some(), "Should have 'data' field");
        let data = json["data"].as_array().expect("data should be array");

        // Should have at least 2 entries
        assert!(data.len() >= 2, "Should have at least 2 entries");

        // Verify first entry has all required fields
        let first_entry = &data[0];
        assert!(first_entry.get("uid").is_some(), "Should have uid");
        assert!(first_entry.get("time").is_some(), "Should have time");
        assert!(first_entry.get("pri").is_some(), "Should have pri");
        assert!(first_entry.get("tag").is_some(), "Should have tag");
        assert!(first_entry.get("pid").is_some(), "Should have pid");
        assert!(first_entry.get("node").is_some(), "Should have node");
        assert!(first_entry.get("user").is_some(), "Should have user");
        assert!(first_entry.get("msg").is_some(), "Should have msg");

        // Verify uid starts at 1
        assert_eq!(
            first_entry["uid"].as_u64().unwrap(),
            1,
            "First uid should be 1"
        );
    }

    #[tokio::test]
    async fn test_clusterlog_entry_limit() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = status::init_with_config(config);

        // Add 10 log entries
        for i in 0..10 {
            add_log_message(
                &status,
                format!("node{i}"),
                6,
                "test".to_string(),
                "test".to_string(),
                format!("Test message {i}"),
            );
        }

        // Request only 5 entries
        let plugin = ClusterlogPlugin::new_with_limit(status, 5);
        let result = plugin.generate_content();
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        let data = json["data"].as_array().unwrap();

        // Should have at most 5 entries
        assert!(data.len() <= 5, "Should respect entry limit");
    }

    #[tokio::test]
    async fn test_clusterlog_field_types() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = status::init_with_config(config);

        add_log_message(
            &status,
            "testnode".to_string(),
            5,
            "testident".to_string(),
            "testtag".to_string(),
            "Test message content".to_string(),
        );

        let plugin = ClusterlogPlugin::new(status);
        let result = plugin.generate_content();
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        let data = json["data"].as_array().unwrap();

        if let Some(entry) = data.first() {
            // uid should be number
            assert!(entry["uid"].is_u64(), "uid should be number");

            // time should be number
            assert!(entry["time"].is_u64(), "time should be number");

            // pri should be number
            assert!(entry["pri"].is_u64(), "pri should be number");

            // tag should be string
            assert!(entry["tag"].is_string(), "tag should be string");
            assert_eq!(entry["tag"].as_str().unwrap(), "testtag");

            // pid should be number (0)
            assert!(entry["pid"].is_u64(), "pid should be number");
            assert_eq!(entry["pid"].as_u64().unwrap(), 0);

            // node should be string
            assert!(entry["node"].is_string(), "node should be string");
            assert_eq!(entry["node"].as_str().unwrap(), "testnode");

            // user should be string
            assert!(entry["user"].is_string(), "user should be string");
            assert_eq!(entry["user"].as_str().unwrap(), "testident");

            // msg should be string
            assert!(entry["msg"].is_string(), "msg should be string");
            assert_eq!(entry["msg"].as_str().unwrap(), "Test message content");
        }
    }

    #[tokio::test]
    async fn test_clusterlog_empty() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = status::init_with_config(config);

        // Get clusterlog without any entries (or clear existing ones)
        let plugin = ClusterlogPlugin::new_with_limit(status, 0);
        let result = plugin.generate_content();
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Should have data field with empty array
        assert!(json.get("data").is_some());
        let data = json["data"].as_array().unwrap();
        assert_eq!(data.len(), 0, "Should have empty data array");
    }
}
