mod error;

pub use error::{PmxcfsError, Result};

/// Maximum size for status data (matches C implementation)
/// From status.h: #define CFS_MAX_STATUS_SIZE (32 * 1024)
pub const CFS_MAX_STATUS_SIZE: usize = 32 * 1024;

/// VM/CT types
///
/// Note: OpenVZ was historically supported (VMTYPE_OPENVZ = 2 in C implementation)
/// but was removed in PVE 4.0 in favor of LXC. Only QEMU and LXC are currently supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmType {
    Qemu,
    Lxc,
}

impl VmType {
    /// Returns the directory name where config files are stored
    pub fn config_dir(&self) -> &'static str {
        match self {
            VmType::Qemu => "qemu-server",
            VmType::Lxc => "lxc",
        }
    }
}

impl std::fmt::Display for VmType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmType::Qemu => write!(f, "qemu"),
            VmType::Lxc => write!(f, "lxc"),
        }
    }
}

/// VM/CT entry for vmlist
#[derive(Debug, Clone)]
pub struct VmEntry {
    pub vmid: u32,
    pub vmtype: VmType,
    pub node: String,
    /// Per-VM version counter (increments when this VM's config changes)
    pub version: u32,
}

/// Information about a cluster member
///
/// This is a shared type used by both cluster and DFSM modules
#[derive(Debug, Clone)]
pub struct MemberInfo {
    pub node_id: u32,
    pub pid: u32,
    pub joined_at: u64,
}

/// Node synchronization info for DFSM state sync
///
/// Used during DFSM synchronization to track which nodes have provided state
#[derive(Debug, Clone)]
pub struct NodeSyncInfo {
    pub node_id: u32,
    pub pid: u32,
    pub state: Option<Vec<u8>>,
    pub synced: bool,
}
