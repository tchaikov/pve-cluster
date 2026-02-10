/// Cluster Log Implementation
///
/// This module provides a cluster-wide log system compatible with the C implementation.
/// It maintains a ring buffer of log entries that can be merged from multiple nodes,
/// deduplicated, and exported to JSON.
///
/// Key features:
/// - Ring buffer storage for efficient memory usage
/// - FNV-1a hashing for node and ident tracking
/// - Deduplication across nodes
/// - Time-based sorting
/// - Multi-node log merging
/// - JSON export for web UI
// Internal modules (not exposed)
mod cluster_log;
mod entry;
mod hash;
mod ring_buffer;

// Public API - only expose what's needed externally
pub use cluster_log::ClusterLog;

// Re-export types only for testing or internal crate use
#[doc(hidden)]
pub use entry::LogEntry;
#[doc(hidden)]
pub use ring_buffer::RingBuffer;
