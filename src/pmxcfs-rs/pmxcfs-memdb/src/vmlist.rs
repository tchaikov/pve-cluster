/// VM list recreation from memdb structure
///
/// This module implements memdb_recreate_vmlist() from the C version (memdb.c:415),
/// which scans the nodes/*/qemu-server/ and nodes/*/lxc/ directories to build
/// a complete VM/CT registry.
use super::database::MemDb;
use anyhow::Result;
use pmxcfs_api_types::{VmEntry, VmType};
use std::collections::HashMap;

/// Recreate VM list by scanning memdb structure
///
/// Equivalent to C's `memdb_recreate_vmlist()` (memdb.c:415)
///
/// Scans the memdb tree structure:
/// - `nodes/*/qemu-server/*.conf` - QEMU VMs
/// - `nodes/*/lxc/*.conf` - LXC containers
///
/// Returns a HashMap of vmid -> VmEntry with node ownership information.
///
/// # Errors
///
/// Returns an error if duplicate VMIDs are found across different nodes.
pub fn recreate_vmlist(memdb: &MemDb) -> Result<HashMap<u32, VmEntry>> {
    let mut vmlist = HashMap::new();
    let mut duplicates = Vec::new();

    // Check if nodes directory exists
    let Ok(nodes_entries) = memdb.readdir("nodes") else {
        // No nodes directory, return empty vmlist
        tracing::debug!("No 'nodes' directory found, returning empty vmlist");
        return Ok(vmlist);
    };

    // Iterate through each node directory
    for node_entry in &nodes_entries {
        if !node_entry.is_dir() {
            continue;
        }

        let node_name = node_entry.name.clone();

        // Validate node name (simple check for valid hostname)
        if !is_valid_nodename(&node_name) {
            tracing::warn!("Skipping invalid node name: {}", node_name);
            continue;
        }

        tracing::debug!("Scanning node: {}", node_name);

        // Scan qemu-server directory
        let qemu_path = format!("nodes/{node_name}/qemu-server");
        if let Ok(qemu_entries) = memdb.readdir(&qemu_path) {
            for vm_entry in qemu_entries {
                if let Some(vmid) = parse_vm_config_name(&vm_entry.name) {
                    if let Some(existing) = vmlist.get(&vmid) {
                        // Duplicate VMID found
                        tracing::error!(
                            vmid,
                            node = %node_name,
                            vmtype = "qemu",
                            existing_node = %existing.node,
                            existing_type = %existing.vmtype,
                            "Duplicate VMID found"
                        );
                        duplicates.push(vmid);
                    } else {
                        vmlist.insert(
                            vmid,
                            VmEntry {
                                vmid,
                                vmtype: VmType::Qemu,
                                node: node_name.clone(),
                                version: vm_entry.version as u32,
                            },
                        );
                        tracing::debug!(vmid, node = %node_name, "Found QEMU VM");
                    }
                }
            }
        }

        // Scan lxc directory
        let lxc_path = format!("nodes/{node_name}/lxc");
        if let Ok(lxc_entries) = memdb.readdir(&lxc_path) {
            for ct_entry in lxc_entries {
                if let Some(vmid) = parse_vm_config_name(&ct_entry.name) {
                    if let Some(existing) = vmlist.get(&vmid) {
                        // Duplicate VMID found
                        tracing::error!(
                            vmid,
                            node = %node_name,
                            vmtype = "lxc",
                            existing_node = %existing.node,
                            existing_type = %existing.vmtype,
                            "Duplicate VMID found"
                        );
                        duplicates.push(vmid);
                    } else {
                        vmlist.insert(
                            vmid,
                            VmEntry {
                                vmid,
                                vmtype: VmType::Lxc,
                                node: node_name.clone(),
                                version: ct_entry.version as u32,
                            },
                        );
                        tracing::debug!(vmid, node = %node_name, "Found LXC CT");
                    }
                }
            }
        }
    }

    if !duplicates.is_empty() {
        tracing::warn!(
            count = duplicates.len(),
            ?duplicates,
            "Found duplicate VMIDs"
        );
    }

    tracing::info!(
        vms = vmlist.len(),
        nodes = nodes_entries.len(),
        "VM list recreation complete"
    );

    Ok(vmlist)
}

/// Parse VM config filename to extract VMID
///
/// Expects format: "{vmid}.conf"
/// Returns Some(vmid) if valid, None otherwise
pub fn parse_vm_config_name(name: &str) -> Option<u32> {
    if let Some(vmid_str) = name.strip_suffix(".conf") {
        // Reject vmid=0 (M2: memdb.c:189 requires first digit is '1'..'9')
        if vmid_str.starts_with('0') {
            return None;
        }
        vmid_str.parse::<u32>().ok()
    } else {
        None
    }
}

/// Validate node name (LDH rule - Letters, Digits, Hyphens)
///
/// Matches C version's valid_nodename() check (memdb.c:222-228)
/// - Only ASCII letters, digits, and hyphens
/// - Cannot start or end with hyphen
/// - No dots allowed (unlike the previous implementation)
pub fn is_valid_nodename(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }

    // Cannot start or end with hyphen
    if name.starts_with('-') || name.ends_with('-') {
        return false;
    }

    // All characters must be alphanumeric or hyphen (no dots)
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Parse a path to check if it contains a VM config
///
/// Returns (nodename, vmtype, vmid) if the path is a VM config, None otherwise
/// Matches C's path_contain_vm_config() (memdb.c:267)
pub fn parse_vm_config_path(path: &str) -> Option<(String, VmType, u32)> {
    // Path format: nodes/{nodename}/qemu-server/{vmid}.conf
    //           or nodes/{nodename}/lxc/{vmid}.conf
    let path = path.trim_start_matches('/');

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 4 || parts[0] != "nodes" {
        return None;
    }

    let nodename = parts[1];
    let vmtype_dir = parts[2];
    let filename = parts[3];

    if !is_valid_nodename(nodename) {
        return None;
    }

    let vmtype = match vmtype_dir {
        "qemu-server" => VmType::Qemu,
        "lxc" => VmType::Lxc,
        _ => return None,
    };

    let vmid = parse_vm_config_name(filename)?;

    Some((nodename.to_string(), vmtype, vmid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vm_config_name() {
        assert_eq!(parse_vm_config_name("100.conf"), Some(100));
        assert_eq!(parse_vm_config_name("999.conf"), Some(999));
        assert_eq!(parse_vm_config_name("123"), None);
        assert_eq!(parse_vm_config_name("abc.conf"), None);
        assert_eq!(parse_vm_config_name(""), None);
        // Reject vmid=0
        assert_eq!(parse_vm_config_name("0.conf"), None);
        assert_eq!(parse_vm_config_name("00.conf"), None);
        assert_eq!(parse_vm_config_name("001.conf"), None);
    }

    #[test]
    fn test_is_valid_nodename() {
        // Valid names
        assert!(is_valid_nodename("node1"));
        assert!(is_valid_nodename("pve-node-01"));
        assert!(is_valid_nodename("a"));
        assert!(is_valid_nodename("node123"));

        // Invalid names
        assert!(!is_valid_nodename("")); // empty
        assert!(!is_valid_nodename("-invalid")); // starts with hyphen
        assert!(!is_valid_nodename("invalid-")); // ends with hyphen
        assert!(!is_valid_nodename("node_1")); // underscore not allowed
        // Dots not allowed (LDH rule)
        assert!(!is_valid_nodename("server.example.com"));
        assert!(!is_valid_nodename(".invalid")); // starts with dot
    }

    #[test]
    fn test_parse_vm_config_path() {
        // Valid paths
        assert_eq!(
            parse_vm_config_path("/nodes/node1/qemu-server/100.conf"),
            Some(("node1".to_string(), VmType::Qemu, 100))
        );
        assert_eq!(
            parse_vm_config_path("nodes/node1/lxc/200.conf"),
            Some(("node1".to_string(), VmType::Lxc, 200))
        );

        // Invalid paths
        assert_eq!(parse_vm_config_path("/nodes/node1/qemu-server/0.conf"), None); // vmid=0
        assert_eq!(parse_vm_config_path("/nodes/node1/qemu-server/abc.conf"), None); // non-numeric
        assert_eq!(parse_vm_config_path("/nodes/node1/other/100.conf"), None); // wrong dir
        assert_eq!(parse_vm_config_path("/other/node1/qemu-server/100.conf"), None); // not under nodes
        assert_eq!(parse_vm_config_path("/nodes/node1/qemu-server/100.txt"), None); // wrong extension
    }
}
