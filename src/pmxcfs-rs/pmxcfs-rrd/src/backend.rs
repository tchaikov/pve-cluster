/// RRD Backend Trait and Implementations
///
/// This module provides an abstraction over different RRD writing mechanisms:
/// - Daemon-based (via rrdcached) for performance and batching
/// - Direct file writing for reliability and fallback scenarios
/// - Fallback composite that tries daemon first, then falls back to direct
///
/// This design matches the C implementation's behavior in status.c where
/// it attempts daemon update first, then falls back to direct file writes.
use super::schema::RrdSchema;
use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;

/// Constants for RRD configuration
pub const DEFAULT_SOCKET_PATH: &str = "/var/run/rrdcached.sock";
pub const RRD_STEP_SECONDS: u64 = 60;

/// Trait for RRD backend implementations
///
/// Provides abstraction over different RRD writing mechanisms.
/// All methods are async to support both async (daemon) and sync (direct file) operations.
#[async_trait]
pub trait RrdBackend: Send + Sync {
    /// Update RRD file with new data
    ///
    /// # Arguments
    /// * `file_path` - Full path to the RRD file
    /// * `data` - Update data in format "timestamp:value1:value2:..."
    async fn update(&mut self, file_path: &Path, data: &str) -> Result<()>;

    /// Create new RRD file with schema
    ///
    /// # Arguments
    /// * `file_path` - Full path where RRD file should be created
    /// * `schema` - RRD schema defining data sources and archives
    /// * `start_timestamp` - Start time for the RRD file (Unix timestamp)
    async fn create(
        &mut self,
        file_path: &Path,
        schema: &RrdSchema,
        start_timestamp: i64,
    ) -> Result<()>;

    /// Flush pending updates to disk
    ///
    /// For daemon backends, this sends a FLUSH command.
    /// For direct backends, this is a no-op (writes are immediate).
    async fn flush(&mut self) -> Result<()>;

    /// Get a human-readable name for this backend
    fn name(&self) -> &str;
}

// Backend implementations
mod backend_daemon;
mod backend_direct;
mod backend_fallback;

pub use backend_daemon::RrdCachedBackend;
pub use backend_direct::RrdDirectBackend;
pub use backend_fallback::RrdFallbackBackend;
