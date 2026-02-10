/// DFSM application callbacks
///
/// This module defines the callback trait that application layers implement
/// to integrate with the DFSM state machine.
use crate::NodeSyncInfo;

/// Callback trait for DFSM operations
///
/// The application layer implements this to receive DFSM events.
/// The associated type `Message` specifies the message type this callback handles:
/// - `FuseMessage` for main database operations
/// - `KvStoreMessage` for status synchronization
///
/// This provides type safety by ensuring each DFSM instance only delivers
/// the correct message type to its callbacks.
pub trait Callbacks: Send + Sync {
    /// The message type this callback handles
    type Message: crate::message::Message;

    /// Deliver an application message
    ///
    /// The message type is determined by the associated type:
    /// - FuseMessage for main database operations
    /// - KvStoreMessage for status synchronization
    fn deliver_message(
        &self,
        nodeid: u32,
        pid: u32,
        message: Self::Message,
        timestamp: u64,
    ) -> anyhow::Result<(i32, bool)>;

    /// Compute state checksum for verification
    fn compute_checksum(&self, output: &mut [u8; 32]) -> anyhow::Result<()>;

    /// Get current state for synchronization
    ///
    /// Called when we need to send our state to other nodes during sync.
    fn get_state(&self) -> anyhow::Result<Vec<u8>>;

    /// Process state update during synchronization
    fn process_state_update(&self, states: &[NodeSyncInfo]) -> anyhow::Result<bool>;

    /// Process incremental update from leader
    ///
    /// The leader sends individual TreeEntry updates during synchronization.
    /// The data is serialized TreeEntry in C-compatible wire format.
    fn process_update(&self, nodeid: u32, pid: u32, data: &[u8]) -> anyhow::Result<()>;

    /// Commit synchronized state
    fn commit_state(&self) -> anyhow::Result<()>;

    /// Called when cluster becomes synced
    fn on_synced(&self);

    /// Clean up sync resources (matches C's dfsm_cleanup_fn)
    ///
    /// Called to release resources allocated during state synchronization.
    /// This is called when sync resources are being released, typically during
    /// membership changes or when transitioning out of sync mode.
    ///
    /// Default implementation does nothing (Rust's RAII handles most cleanup).
    fn cleanup_sync_resources(&self, _states: &[NodeSyncInfo]) {
        // Default: no-op, Rust's Drop trait handles cleanup
    }

    /// Called on membership changes (matches C's dfsm_confchg_fn)
    ///
    /// Notifies the application layer when cluster membership changes.
    /// This can be used for logging, monitoring, or application-specific
    /// membership tracking.
    ///
    /// # Arguments
    /// * `member_list` - Current list of cluster members after the change
    ///
    /// Default implementation does nothing (membership handled internally).
    fn on_membership_change(&self, _member_list: &[pmxcfs_api_types::MemberInfo]) {
        // Default: no-op, membership changes handled internally
    }
}
