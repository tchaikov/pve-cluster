//! IPC Service implementation
//!
//! This module implements the IPC service handler that processes requests
//! from client applications via libqb-compatible IPC.

use super::IpcRequest;
use async_trait::async_trait;
use pmxcfs_config::Config;
use pmxcfs_ipc::{Handler, Permissions, Request, Response};
use pmxcfs_memdb::MemDb;
use pmxcfs_status as status;
use std::io::Error as IoError;
use std::sync::Arc;

/// IPC handler for pmxcfs protocol operations
pub struct IpcHandler {
    memdb: MemDb,
    status: Arc<status::Status>,
    config: Arc<Config>,
    www_data_gid: u32,
}

impl IpcHandler {
    /// Create a new IPC handler
    pub fn new(
        memdb: MemDb,
        status: Arc<status::Status>,
        config: Arc<Config>,
        www_data_gid: u32,
    ) -> Self {
        Self {
            memdb,
            status,
            config,
            www_data_gid,
        }
    }
}

impl IpcHandler {
    /// Handle an IPC request and return (error_code, response_data)
    async fn handle_request(&self, request: IpcRequest, is_read_only: bool) -> (i32, Vec<u8>) {
        let result = match request {
            IpcRequest::GetFsVersion => self.handle_get_fs_version(),
            IpcRequest::GetClusterInfo => self.handle_get_cluster_info(),
            IpcRequest::GetGuestList => self.handle_get_guest_list(),
            IpcRequest::GetConfig { path } => self.handle_get_config(&path, is_read_only),
            IpcRequest::GetStatus { name, node_name } => {
                self.handle_get_status(&name, &node_name)
            }
            IpcRequest::SetStatus { name, data } => {
                if is_read_only {
                    Err(IoError::from_raw_os_error(libc::EPERM))
                } else {
                    self.handle_set_status(&name, &data).await
                }
            }
            IpcRequest::LogClusterMsg {
                priority,
                ident,
                tag,
                message,
            } => {
                if is_read_only {
                    Err(IoError::from_raw_os_error(libc::EPERM))
                } else {
                    self.handle_log_cluster_msg(priority, &ident, &tag, &message)
                }
            }
            IpcRequest::GetClusterLog { max_entries, user } => {
                self.handle_get_cluster_log(max_entries, &user)
            }
            IpcRequest::GetRrdDump => self.handle_get_rrd_dump(),
            IpcRequest::GetGuestConfigProperty { vmid, property } => {
                self.handle_get_guest_config_property(vmid, &property)
            }
            IpcRequest::VerifyToken { token } => self.handle_verify_token(&token),
            IpcRequest::GetGuestConfigProperties { vmid, properties } => {
                self.handle_get_guest_config_properties(vmid, &properties)
            }
        };

        match result {
            Ok(response_data) => (0, response_data),
            Err(e) => {
                let error_code = if let Some(os_error) = e.raw_os_error() {
                    -os_error
                } else {
                    -libc::EIO
                };
                tracing::debug!("Request error: {}", e);
                (error_code, Vec::new())
            }
        }
    }

    /// GET_FS_VERSION: Return filesystem version information
    fn handle_get_fs_version(&self) -> Result<Vec<u8>, IoError> {
        let version = serde_json::json!({
            "version": 1,
            "protocol": 1,
            "cluster": self.status.is_quorate(),
        });
        Ok(version.to_string().into_bytes())
    }

    /// GET_CLUSTER_INFO: Return cluster member list
    fn handle_get_cluster_info(&self) -> Result<Vec<u8>, IoError> {
        let members = self.status.get_members();
        let member_list: Vec<serde_json::Value> = members
            .iter()
            .map(|m| {
                serde_json::json!({
                    "nodeid": m.node_id,
                    "name": format!("node{}", m.node_id),
                    "ip": "127.0.0.1",
                    "online": true,
                })
            })
            .collect();

        let info = serde_json::json!({
            "nodelist": member_list,
            "quorate": self.status.is_quorate(),
        });
        Ok(info.to_string().into_bytes())
    }

    /// GET_GUEST_LIST: Return VM/CT list
    fn handle_get_guest_list(&self) -> Result<Vec<u8>, IoError> {
        let vmlist_data = self.status.get_vmlist();

        // Convert VM list to JSON format matching C implementation
        let mut ids = serde_json::Map::new();
        for (vmid, vm_entry) in vmlist_data {
            ids.insert(
                vmid.to_string(),
                serde_json::json!({
                    "node": vm_entry.node,
                    "type": vm_entry.vmtype.to_string(),
                    "version": vm_entry.version,
                }),
            );
        }

        let vmlist = serde_json::json!({
            "version": 1,
            "ids": ids,
        });

        Ok(vmlist.to_string().into_bytes())
    }

    /// GET_CONFIG: Read configuration file
    fn handle_get_config(&self, path: &str, is_read_only: bool) -> Result<Vec<u8>, IoError> {
        // Check if read-only client is trying to access private path
        if is_read_only && path.starts_with("priv/") {
            return Err(IoError::from_raw_os_error(libc::EPERM));
        }

        // Read from memdb
        match self.memdb.read(path, 0, 1024 * 1024) {
            Ok(data) => Ok(data),
            Err(_) => Err(IoError::from_raw_os_error(libc::ENOENT)),
        }
    }

    /// GET_STATUS: Get node status
    ///
    /// Matches C implementation: cfs_create_status_msg(outbuf, nodename, name)
    /// where nodename is the node to query and name is the specific status key.
    ///
    /// C implementation (server.c:233, status.c:1640-1668):
    /// - If name is empty: return ENOENT
    /// - Local node: look up bare `name` key in node_status (cfs_status.kvhash)
    /// - Remote node: resolve nodename→nodeid, look up in kvstore (clnode->kvhash)
    fn handle_get_status(&self, name: &str, nodename: &str) -> Result<Vec<u8>, IoError> {
        if name.is_empty() {
            return Err(IoError::from_raw_os_error(libc::ENOENT));
        }

        let is_local = nodename.is_empty() || nodename == self.config.nodename();

        if is_local {
            // Local node: look up bare key in node_status (matches C: cfs_status.kvhash[key])
            if let Some(ns) = self.status.get_node_status(name) {
                return Ok(ns.data);
            }
        } else {
            // Remote node: resolve nodename→nodeid, look up in kvstore
            // (matches C: clnode->kvhash[key] via clinfo->nodes_byname)
            if let Some(info) = self.status.get_cluster_info() {
                if let Some(&nodeid) = info.nodes_by_name.get(nodename) {
                    if let Some(data) = self.status.get_node_kv(nodeid, name) {
                        return Ok(data);
                    }
                }
            }
        }

        Err(IoError::from_raw_os_error(libc::ENOENT))
    }

    /// SET_STATUS: Update node status
    async fn handle_set_status(&self, name: &str, status_data: &[u8]) -> Result<Vec<u8>, IoError> {
        self.status
            .set_node_status(name.to_string(), status_data.to_vec())
            .await
            .map_err(|_| IoError::from_raw_os_error(libc::EIO))?;

        Ok(Vec::new())
    }

    /// LOG_CLUSTER_MSG: Write to cluster log
    fn handle_log_cluster_msg(
        &self,
        priority: u8,
        ident: &str,
        tag: &str,
        message: &str,
    ) -> Result<Vec<u8>, IoError> {
        // Get node name from config (matches C implementation's cfs.nodename)
        let node = self.config.nodename().to_string();

        // Add log entry to cluster log
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| IoError::from_raw_os_error(libc::EIO))?
            .as_secs();

        let entry = status::ClusterLogEntry {
            uid: 0, // Will be assigned by cluster log
            timestamp,
            priority,
            tag: tag.to_string(),
            pid: std::process::id(),
            node,
            ident: ident.to_string(),
            message: message.to_string(),
        };

        self.status.add_log_entry(entry);

        Ok(Vec::new())
    }

    /// GET_CLUSTER_LOG: Read cluster log
    ///
    /// The `user` parameter is used for filtering log entries by user.
    /// Matches C implementation: cfs_cluster_log_dump(outbuf, user, max)
    /// Returns JSON format: {"data": [{entry1}, {entry2}, ...]}
    fn handle_get_cluster_log(&self, max_entries: usize, user: &str) -> Result<Vec<u8>, IoError> {
        let entries = self.status.get_log_entries_filtered(max_entries, user);

        // Format as JSON object with "data" array (matches C implementation)
        let json_entries: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "uid": entry.uid,
                    "time": entry.timestamp,
                    "pri": entry.priority,
                    "tag": entry.tag,
                    "pid": entry.pid,
                    "node": entry.node,
                    "user": entry.ident,
                    "msg": entry.message,
                })
            })
            .collect();

        let response = serde_json::json!({
            "data": json_entries
        });

        Ok(response.to_string().into_bytes())
    }

    /// GET_RRD_DUMP: Get RRD data dump in C-compatible text format
    fn handle_get_rrd_dump(&self) -> Result<Vec<u8>, IoError> {
        let rrd_dump = self.status.get_rrd_dump();
        Ok(rrd_dump.into_bytes())
    }

    /// GET_GUEST_CONFIG_PROPERTY: Get guest config property
    fn handle_get_guest_config_property(
        &self,
        vmid: u32,
        property: &str,
    ) -> Result<Vec<u8>, IoError> {
        // Delegate to multi-property handler with single property
        self.handle_get_guest_config_properties_impl(&[property], vmid)
    }

    /// VERIFY_TOKEN: Verify authentication token
    ///
    /// Matches C implementation (server.c:399-448):
    /// - Empty token → EINVAL
    /// - Token containing newline → EINVAL
    /// - Exact line match (no trimming), splitting on '\n' only
    fn handle_verify_token(&self, token: &str) -> Result<Vec<u8>, IoError> {
        // Reject empty tokens
        if token.is_empty() {
            return Err(IoError::from_raw_os_error(libc::EINVAL));
        }

        // Reject tokens containing newlines (would break line-based matching)
        if token.contains('\n') {
            return Err(IoError::from_raw_os_error(libc::EINVAL));
        }

        // Read token.cfg from database
        match self.memdb.read("priv/token.cfg", 0, 1024 * 1024) {
            Ok(token_data) => {
                // Check if token exists in file (one token per line)
                // C splits on '\n' only (not '\r\n') and does exact match (no trim)
                let token_str = String::from_utf8_lossy(&token_data);
                for line in token_str.split('\n') {
                    if line == token {
                        return Ok(Vec::new()); // Success
                    }
                }
                Err(IoError::from_raw_os_error(libc::ENOENT))
            }
            Err(_) => Err(IoError::from_raw_os_error(libc::ENOENT)),
        }
    }

    /// GET_GUEST_CONFIG_PROPERTIES: Get multiple guest config properties
    fn handle_get_guest_config_properties(
        &self,
        vmid: u32,
        properties: &[String],
    ) -> Result<Vec<u8>, IoError> {
        // Convert Vec<String> to &[&str] for the impl function
        let property_refs: Vec<&str> = properties.iter().map(|s| s.as_str()).collect();
        self.handle_get_guest_config_properties_impl(&property_refs, vmid)
    }

    /// Core implementation for getting guest config properties
    fn handle_get_guest_config_properties_impl(
        &self,
        properties: &[&str],
        vmid: u32,
    ) -> Result<Vec<u8>, IoError> {
        // Validate vmid range
        if vmid > 0 && vmid < 100 {
            tracing::debug!("vmid out of range: {}", vmid);
            return Err(IoError::from_raw_os_error(libc::EINVAL));
        }

        // Build response as a map: vmid -> {property -> value}
        let mut response_map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        if vmid >= 100 {
            // Get specific VM
            let vmlist = self.status.get_vmlist();

            if !vmlist.contains_key(&vmid) {
                return Err(IoError::from_raw_os_error(libc::ENOENT));
            }

            let vm_entry = vmlist.get(&vmid).unwrap();

            // Get config path for this VM
            let config_path = format!(
                "nodes/{}/{}/{}.conf",
                &vm_entry.node,
                vm_entry.vmtype.config_dir(),
                vmid
            );

            // Read config from memdb
            match self.memdb.read(&config_path, 0, 1024 * 1024) {
                Ok(config_data) => {
                    let config_str = String::from_utf8_lossy(&config_data);
                    let values = extract_properties(&config_str, properties);

                    if !values.is_empty() {
                        response_map
                            .insert(vmid.to_string(), serde_json::to_value(&values).unwrap());
                    }
                }
                Err(e) => {
                    tracing::debug!("Failed to read config for VM {}: {}", vmid, e);
                    return Err(IoError::from_raw_os_error(libc::EIO));
                }
            }
        } else {
            // vmid == 0: Get properties from all VMs
            let vmlist = self.status.get_vmlist();

            for (vm_id, vm_entry) in vmlist.iter() {
                let config_path = format!(
                    "nodes/{}/{}/{}.conf",
                    &vm_entry.node,
                    vm_entry.vmtype.config_dir(),
                    vm_id
                );

                // Read config from memdb
                if let Ok(config_data) = self.memdb.read(&config_path, 0, 1024 * 1024) {
                    let config_str = String::from_utf8_lossy(&config_data);
                    let values = extract_properties(&config_str, properties);

                    if !values.is_empty() {
                        response_map
                            .insert(vm_id.to_string(), serde_json::to_value(&values).unwrap());
                    }
                }
            }
        }

        // Serialize to JSON with pretty printing (matches C output format)
        let json_str = serde_json::to_string_pretty(&response_map).map_err(|e| {
            tracing::error!("Failed to serialize JSON: {}", e);
            IoError::from_raw_os_error(libc::EIO)
        })?;

        Ok(json_str.into_bytes())
    }
}

/// Extract property values from a VM config file
///
/// Parses config file line-by-line looking for "property: value" patterns.
/// Matches the C implementation's parsing behavior from status.c:767-796.
///
/// Format: `^([a-z][a-z_]*\d*):\s*(.+?)\s*$`
/// - Property name must start with lowercase letter
/// - Followed by colon and optional whitespace
/// - Value is trimmed of leading/trailing whitespace
/// - Stops at snapshot sections (lines starting with '[')
///
/// Returns a map of property names to their values.
fn extract_properties(
    config: &str,
    properties: &[&str],
) -> std::collections::HashMap<String, String> {
    let mut values = std::collections::HashMap::new();

    // Parse config line by line
    for line in config.lines() {
        // Stop at snapshot or pending section markers (matches C implementation)
        if line.starts_with('[') {
            break;
        }

        // Skip empty lines
        if line.is_empty() {
            continue;
        }

        // Find colon separator (required in VM config format)
        let Some(colon_pos) = line.find(':') else {
            continue;
        };

        // Extract key (property name)
        let key = &line[..colon_pos];

        // Property must start with lowercase letter (matches C regex check)
        if key.is_empty() || !key.chars().next().unwrap().is_ascii_lowercase() {
            continue;
        }

        // Extract value after colon
        let value = &line[colon_pos + 1..];

        // Trim leading and trailing whitespace from value (matches C implementation)
        let value = value.trim();

        // Skip if value is empty after trimming
        if value.is_empty() {
            continue;
        }

        // Check if this is one of the requested properties
        if properties.contains(&key) {
            values.insert(key.to_string(), value.to_string());
        }
    }

    values
}

#[async_trait]
impl Handler for IpcHandler {
    fn authenticate(&self, uid: u32, gid: u32) -> Option<Permissions> {
        // Root with gid 0 gets read-write access
        // Matches C: (uid == 0 && gid == 0) branch in server.c:111
        if uid == 0 && gid == 0 {
            tracing::debug!(
                "IPC authentication: uid={}, gid={} - granted ReadWrite (root)",
                uid,
                gid
            );
            return Some(Permissions::ReadWrite);
        }

        // www-data group gets read-only access (regardless of uid)
        // Matches C: (gid == cfs.gid) branch in server.c:111
        if gid == self.www_data_gid {
            tracing::debug!(
                "IPC authentication: uid={}, gid={} - granted ReadOnly (www-data group)",
                uid,
                gid
            );
            return Some(Permissions::ReadOnly);
        }

        // Reject all other connections with security logging
        tracing::warn!(
            "IPC authentication failed: uid={}, gid={} - access denied (not root or www-data group)",
            uid,
            gid
        );
        None
    }

    async fn handle(&self, request: Request) -> Response {
        // Deserialize IPC request from message ID and data
        let ipc_request = match IpcRequest::deserialize(request.msg_id, &request.data) {
            Ok(req) => req,
            Err(e) => {
                tracing::warn!(
                    "Failed to deserialize IPC request (msg_id={}): {}",
                    request.msg_id,
                    e
                );
                return Response::err(-libc::EINVAL);
            }
        };

        let (error_code, data) = self.handle_request(ipc_request, request.is_read_only).await;

        Response { error_code, data }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_properties() {
        let config = r#"
# VM Configuration
memory: 2048
cores: 4
sockets: 1
cpu: host
boot: order=scsi0;net0
name: test-vm
onboot: 1
"#;

        let properties = vec!["memory", "cores", "name", "nonexistent"];
        let result = extract_properties(config, &properties);

        assert_eq!(result.get("memory"), Some(&"2048".to_string()));
        assert_eq!(result.get("cores"), Some(&"4".to_string()));
        assert_eq!(result.get("name"), Some(&"test-vm".to_string()));
        assert_eq!(result.get("nonexistent"), None);
    }

    #[test]
    fn test_extract_properties_empty_config() {
        let config = "";
        let properties = vec!["memory"];
        let result = extract_properties(config, &properties);
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_properties_stops_at_snapshot() {
        let config = r#"
memory: 2048
cores: 4
[snapshot]
memory: 4096
name: snapshot-value
"#;
        let properties = vec!["memory", "cores", "name"];
        let result = extract_properties(config, &properties);

        // Should stop at [snapshot] marker
        assert_eq!(result.get("memory"), Some(&"2048".to_string()));
        assert_eq!(result.get("cores"), Some(&"4".to_string()));
        assert_eq!(result.get("name"), None); // After [snapshot], should not be parsed
    }

    #[test]
    fn test_extract_properties_with_special_chars() {
        let config = r#"
name: test"vm
description: Line1\nLine2
path: /path/to\file
"#;

        let properties = vec!["name", "description", "path"];
        let result = extract_properties(config, &properties);

        assert_eq!(result.get("name"), Some(&r#"test"vm"#.to_string()));
        assert_eq!(
            result.get("description"),
            Some(&r#"Line1\nLine2"#.to_string())
        );
        assert_eq!(result.get("path"), Some(&r#"/path/to\file"#.to_string()));
    }

    #[test]
    fn test_extract_properties_whitespace_handling() {
        let config = r#"
memory:  2048
cores:4
name:   test-vm
"#;

        let properties = vec!["memory", "cores", "name"];
        let result = extract_properties(config, &properties);

        // Values should be trimmed of leading/trailing whitespace
        assert_eq!(result.get("memory"), Some(&"2048".to_string()));
        assert_eq!(result.get("cores"), Some(&"4".to_string()));
        assert_eq!(result.get("name"), Some(&"test-vm".to_string()));
    }

    #[test]
    fn test_extract_properties_invalid_format() {
        let config = r#"
Memory: 2048
CORES: 4
_private: value
123: value
name value
"#;

        let properties = vec!["Memory", "CORES", "_private", "123", "name"];
        let result = extract_properties(config, &properties);

        // None should match because:
        // - "Memory" starts with uppercase
        // - "CORES" starts with uppercase
        // - "_private" starts with underscore
        // - "123" starts with digit
        // - "name value" has no colon
        assert!(result.is_empty());
    }

    #[test]
    fn test_json_serialization_with_serde() {
        // Verify that serde_json properly handles escaping
        let mut values = std::collections::HashMap::new();
        values.insert("name".to_string(), r#"test"vm"#.to_string());
        values.insert("description".to_string(), "Line1\nLine2".to_string());

        let json = serde_json::to_string(&values).unwrap();

        // serde_json should properly escape quotes and newlines
        assert!(json.contains(r#"\"test\\\"vm\""#) || json.contains(r#""test\"vm""#));
        assert!(json.contains(r#"\n"#));
    }

    #[test]
    fn test_json_pretty_format() {
        // Verify pretty printing works
        let mut response_map = serde_json::Map::new();
        let mut vm_props = std::collections::HashMap::new();
        vm_props.insert("memory".to_string(), "2048".to_string());
        vm_props.insert("cores".to_string(), "4".to_string());

        response_map.insert("100".to_string(), serde_json::to_value(&vm_props).unwrap());

        let json_str = serde_json::to_string_pretty(&response_map).unwrap();

        // Pretty format should have newlines
        assert!(json_str.contains('\n'));
        // Should contain the VM ID and properties
        assert!(json_str.contains("100"));
        assert!(json_str.contains("memory"));
        assert!(json_str.contains("2048"));
    }
}
