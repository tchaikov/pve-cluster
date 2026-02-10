/// Distributed Finite State Machine (DFSM) for cluster state synchronization
///
/// This crate implements the state machine for synchronizing configuration
/// changes across the cluster nodes using Corosync CPG.
///
/// The DFSM handles:
/// - State synchronization between nodes
/// - Message ordering and queuing
/// - Leader-based state updates
/// - Split-brain prevention
/// - Membership change handling
mod callbacks;
pub mod cluster_database_service;
mod cpg_service;
mod dfsm_message;
mod fuse_message;
mod kv_store_message;
mod message;
mod state_machine;
pub mod status_sync_service;
mod types;
mod wire_format;

// Re-export public API
pub use callbacks::Callbacks;
pub use cluster_database_service::ClusterDatabaseService;
pub use cpg_service::{CpgHandler, CpgService};
pub use fuse_message::FuseMessage;
pub use kv_store_message::KvStoreMessage;
pub use state_machine::{Dfsm, DfsmBroadcast};
pub use status_sync_service::StatusSyncService;
pub use types::NodeSyncInfo;
