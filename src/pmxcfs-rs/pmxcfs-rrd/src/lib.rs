/// RRD (Round-Robin Database) Persistence Module
///
/// This module provides RRD file persistence compatible with the C pmxcfs implementation.
/// It handles:
/// - RRD file creation with proper schemas (node, VM, storage)
/// - RRD file updates (writing metrics to disk)
/// - Multiple backend strategies:
///   - Daemon mode: High-performance batched updates via rrdcached
///   - Direct mode: Reliable fallback using direct file writes
///   - Fallback mode: Tries daemon first, falls back to direct (matches C behavior)
/// - Version management (pve2 vs pve-9.0 formats)
///
/// The implementation matches the C behavior in status.c where it attempts
/// daemon updates first, then falls back to direct file operations.
mod backend;
mod key_type;
mod parse;
#[cfg(feature = "rrdcached")]
mod rrdcached;
pub(crate) mod schema;
mod writer;

pub use writer::RrdWriter;
