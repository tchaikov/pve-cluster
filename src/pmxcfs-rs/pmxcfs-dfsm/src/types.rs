/// DFSM type definitions
///
/// This module contains all type definitions used by the DFSM state machine.
/// DFSM operating modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DfsmMode {
    /// Initial state - starting cluster connection
    Start = 0,

    /// Starting data synchronization
    StartSync = 1,

    /// All data is up to date
    Synced = 2,

    /// Waiting for updates from leader
    Update = 3,

    /// Error states (>= 128)
    Leave = 253,
    VersionError = 254,
    Error = 255,
}

impl DfsmMode {
    /// Check if this is an error mode
    pub fn is_error(&self) -> bool {
        (*self as u8) >= 128
    }
}

impl std::fmt::Display for DfsmMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DfsmMode::Start => write!(f, "start cluster connection"),
            DfsmMode::StartSync => write!(f, "starting data synchronization"),
            DfsmMode::Synced => write!(f, "all data is up to date"),
            DfsmMode::Update => write!(f, "waiting for updates from leader"),
            DfsmMode::Leave => write!(f, "leaving cluster"),
            DfsmMode::VersionError => write!(f, "protocol version mismatch"),
            DfsmMode::Error => write!(f, "serious internal error"),
        }
    }
}

/// DFSM message types (internal protocol messages)
/// Matches C's dfsm_message_t enum values
#[derive(Debug, Clone, Copy, PartialEq, Eq, num_enum::TryFromPrimitive)]
#[repr(u16)]
pub enum DfsmMessageType {
    Normal = 0,
    SyncStart = 1,
    State = 2,
    Update = 3,
    UpdateComplete = 4,
    VerifyRequest = 5,
    Verify = 6,
}

/// Sync epoch - identifies a synchronization session
/// Matches C's dfsm_sync_epoch_t structure (16 bytes total)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyncEpoch {
    pub epoch: u32,
    pub time: u32,
    pub nodeid: u32,
    pub pid: u32,
}

impl SyncEpoch {
    /// Serialize to C-compatible wire format (16 bytes)
    /// Format: [epoch: u32][time: u32][nodeid: u32][pid: u32]
    pub fn serialize(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&self.epoch.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.time.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.nodeid.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.pid.to_le_bytes());
        bytes
    }

    /// Deserialize from C-compatible wire format (16 bytes)
    pub fn deserialize(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 16 {
            return Err("SyncEpoch requires 16 bytes");
        }
        Ok(SyncEpoch {
            epoch: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            time: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            nodeid: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            pid: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        })
    }
}

/// Queued message awaiting delivery
#[derive(Debug, Clone)]
pub(super) struct QueuedMessage<M> {
    pub nodeid: u32,
    pub pid: u32,
    pub _msg_count: u64,
    pub message: M,
    pub timestamp: u64,
}

// Re-export NodeSyncInfo from pmxcfs-api-types for use in Callbacks trait
pub use pmxcfs_api_types::NodeSyncInfo;
