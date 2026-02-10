/// RRD Backend: Fallback (Daemon + Direct)
///
/// Composite backend that tries daemon first, falls back to direct file writing.
/// This matches the C implementation's behavior in status.c:1405-1420 where
/// it attempts rrdc_update() first, then falls back to rrd_update_r().
use super::super::schema::RrdSchema;
use super::{RrdCachedBackend, RrdDirectBackend};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;

/// Composite backend that tries daemon first, falls back to direct
///
/// This provides the same behavior as the C implementation:
/// 1. Try to use rrdcached daemon for performance
/// 2. If daemon fails or is unavailable, fall back to direct file writes
pub struct RrdFallbackBackend {
    /// Optional daemon backend (None if daemon is unavailable/failed)
    daemon: Option<RrdCachedBackend>,
    /// Direct backend (always available)
    direct: RrdDirectBackend,
}

impl RrdFallbackBackend {
    /// Create a new fallback backend
    ///
    /// Attempts to connect to rrdcached daemon. If successful, will prefer daemon.
    /// If daemon is unavailable, will use direct mode only.
    ///
    /// # Arguments
    /// * `daemon_socket` - Path to rrdcached Unix socket
    pub async fn new(daemon_socket: &str) -> Self {
        let daemon = match RrdCachedBackend::connect(daemon_socket).await {
            Ok(backend) => {
                tracing::info!("RRD fallback backend: daemon available, will prefer daemon mode");
                Some(backend)
            }
            Err(e) => {
                tracing::warn!(
                    "RRD fallback backend: daemon unavailable ({}), using direct mode only",
                    e
                );
                None
            }
        };

        let direct = RrdDirectBackend::new();

        Self { daemon, direct }
    }

    /// Create a fallback backend with explicit daemon and direct backends
    ///
    /// Useful for testing or custom configurations
    #[allow(dead_code)] // Used in tests for custom backend configurations
    pub fn with_backends(daemon: Option<RrdCachedBackend>, direct: RrdDirectBackend) -> Self {
        Self { daemon, direct }
    }

    /// Check if daemon is currently being used
    #[allow(dead_code)] // Used for debugging/monitoring daemon status
    pub fn is_using_daemon(&self) -> bool {
        self.daemon.is_some()
    }

    /// Disable daemon mode and switch to direct mode only
    ///
    /// Called automatically when daemon operations fail
    fn disable_daemon(&mut self) {
        if self.daemon.is_some() {
            tracing::warn!("Disabling daemon mode, switching to direct file writes");
            self.daemon = None;
        }
    }
}

#[async_trait]
impl super::super::backend::RrdBackend for RrdFallbackBackend {
    async fn update(&mut self, file_path: &Path, data: &str) -> Result<()> {
        // Try daemon first if available
        if let Some(daemon) = &mut self.daemon {
            match daemon.update(file_path, data).await {
                Ok(()) => {
                    tracing::trace!("Updated RRD via daemon (fallback backend)");
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!("Daemon update failed, falling back to direct: {}", e);
                    self.disable_daemon();
                }
            }
        }

        // Fallback to direct
        self.direct
            .update(file_path, data)
            .await
            .context("Both daemon and direct update failed")
    }

    async fn create(
        &mut self,
        file_path: &Path,
        schema: &RrdSchema,
        start_timestamp: i64,
    ) -> Result<()> {
        // Try daemon first if available
        if let Some(daemon) = &mut self.daemon {
            match daemon.create(file_path, schema, start_timestamp).await {
                Ok(()) => {
                    tracing::trace!("Created RRD via daemon (fallback backend)");
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!("Daemon create failed, falling back to direct: {}", e);
                    self.disable_daemon();
                }
            }
        }

        // Fallback to direct
        self.direct
            .create(file_path, schema, start_timestamp)
            .await
            .context("Both daemon and direct create failed")
    }

    async fn flush(&mut self) -> Result<()> {
        // Only flush if using daemon
        if let Some(daemon) = &mut self.daemon {
            match daemon.flush().await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!("Daemon flush failed: {}", e);
                    self.disable_daemon();
                }
            }
        }

        // Direct backend flush is a no-op
        self.direct.flush().await
    }

    fn name(&self) -> &str {
        if self.daemon.is_some() {
            "fallback(daemon+direct)"
        } else {
            "fallback(direct-only)"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::RrdBackend;
    use crate::schema::{RrdFormat, RrdSchema};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Create a temporary directory for RRD files
    fn setup_temp_dir() -> TempDir {
        TempDir::new().expect("Failed to create temp directory")
    }

    /// Create a test RRD file path
    fn test_rrd_path(dir: &TempDir, name: &str) -> PathBuf {
        dir.path().join(format!("{}.rrd", name))
    }

    #[test]
    fn test_fallback_backend_without_daemon() {
        let direct = RrdDirectBackend::new();
        let backend = RrdFallbackBackend::with_backends(None, direct);

        assert!(!backend.is_using_daemon());
        assert_eq!(backend.name(), "fallback(direct-only)");
    }

    #[tokio::test]
    async fn test_fallback_backend_direct_mode_operations() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "fallback_test");

        // Create fallback backend without daemon (direct mode only)
        let direct = RrdDirectBackend::new();
        let mut backend = RrdFallbackBackend::with_backends(None, direct);

        assert!(!backend.is_using_daemon(), "Should not be using daemon");
        assert_eq!(backend.name(), "fallback(direct-only)");

        // Test create and update operations work in direct mode
        let schema = RrdSchema::storage(RrdFormat::Pve2);
        let start_time = 1704067200;

        let result = backend.create(&rrd_path, &schema, start_time).await;
        assert!(result.is_ok(), "Create should work in direct mode");

        let result = backend.update(&rrd_path, "N:1000:500").await;
        assert!(result.is_ok(), "Update should work in direct mode");
    }

    #[tokio::test]
    async fn test_fallback_backend_flush_without_daemon() {
        let direct = RrdDirectBackend::new();
        let mut backend = RrdFallbackBackend::with_backends(None, direct);

        // Flush should succeed even without daemon (no-op for direct)
        let result = backend.flush().await;
        assert!(result.is_ok(), "Flush should succeed without daemon");
    }
}
