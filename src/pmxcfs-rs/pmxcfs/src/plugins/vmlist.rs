/// .vmlist Plugin - Virtual Machine List
///
/// This plugin provides VM/CT list in JSON format:
/// {
///   "version": 1,
///   "ids": {
///     "100": { "node": "node1", "type": "qemu", "version": 1 },
///     "101": { "node": "node2", "type": "lxc", "version": 1 }
///   }
/// }
use pmxcfs_status::Status;
use serde_json::json;
use std::sync::Arc;

use super::Plugin;

/// Vmlist plugin - provides VM/CT list
pub struct VmlistPlugin {
    status: Arc<Status>,
}

impl VmlistPlugin {
    pub fn new(status: Arc<Status>) -> Self {
        Self { status }
    }

    /// Generate vmlist content
    fn generate_content(&self) -> String {
        let vmlist = self.status.get_vmlist();
        let vmlist_version = self.status.get_vmlist_version();

        // Convert to JSON format expected by Proxmox
        // Format: {"version":N,"ids":{vmid:{"node":"nodename","type":"qemu|lxc","version":M}}}
        let mut ids = serde_json::Map::new();

        for (vmid, entry) in vmlist {
            let vm_obj = json!({
                "node": entry.node,
                "type": entry.vmtype.to_string(),
                "version": entry.version
            });

            ids.insert(vmid.to_string(), vm_obj);
        }

        json!({
            "version": vmlist_version,
            "ids": ids
        })
        .to_string()
    }
}

impl Plugin for VmlistPlugin {
    fn name(&self) -> &str {
        ".vmlist"
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
    async fn test_vmlist_format() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

        let plugin = VmlistPlugin::new(status);
        let result = plugin.generate_content();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Should have version
        assert!(parsed["version"].is_number());

        // Should have ids object
        assert!(parsed["ids"].is_object());
    }

    #[tokio::test]
    async fn test_vmlist_versions() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

        // Register a VM
        status.register_vm(100, pmxcfs_status::VmType::Qemu, "node1".to_string());

        let plugin = VmlistPlugin::new(status.clone());
        let result = plugin.generate_content();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Root version should be >= 1
        assert!(parsed["version"].as_u64().unwrap() >= 1);

        // VM should have version 1
        assert_eq!(parsed["ids"]["100"]["version"], 1);
        assert_eq!(parsed["ids"]["100"]["type"], "qemu");
        assert_eq!(parsed["ids"]["100"]["node"], "node1");

        // Update the VM - version should increment
        status.register_vm(100, pmxcfs_status::VmType::Qemu, "node1".to_string());

        let result2 = plugin.generate_content();
        let parsed2: serde_json::Value = serde_json::from_str(&result2).unwrap();

        // Root version should have incremented
        assert!(parsed2["version"].as_u64().unwrap() > parsed["version"].as_u64().unwrap());

        // VM version should have incremented to 2
        assert_eq!(parsed2["ids"]["100"]["version"], 2);
    }
}
