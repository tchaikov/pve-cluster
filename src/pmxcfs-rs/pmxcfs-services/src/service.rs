//! Service trait and related types
//!
//! Simplified design based on actual usage patterns.
//! All production services use file descriptors and are restartable.

use crate::error::Result;
use async_trait::async_trait;
use std::os::unix::io::RawFd;
use std::time::Duration;

/// A managed service with automatic retry and event-driven dispatch
///
/// All services are:
/// - Event-driven (use file descriptors)
/// - Restartable (automatic retry on failure)
/// - Optionally have timer callbacks
#[async_trait]
pub trait Service: Send + Sync {
    /// Service name for logging
    fn name(&self) -> &str;

    /// Initialize the service and return a file descriptor to monitor
    ///
    /// The service retains ownership of the fd and must close it in finalize().
    /// The ServiceManager monitors the fd and calls dispatch() when readable.
    ///
    /// On error, the service will be automatically retried after 5 seconds.
    async fn initialize(&mut self) -> Result<RawFd>;

    /// Handle events when the file descriptor becomes readable
    ///
    /// Return `Ok(true)` to continue, or `Ok(false)` to request reinitialization.
    async fn dispatch(&mut self) -> Result<bool>;

    /// Clean up resources (called on shutdown or before reinitialization)
    ///
    /// Must close the file descriptor returned by initialize().
    /// Should be idempotent (safe to call multiple times).
    async fn finalize(&mut self) -> Result<()>;

    /// Optional timer callback invoked periodically
    ///
    /// Return None to disable timer callbacks.
    fn timer_period(&self) -> Option<Duration> {
        None
    }

    /// Optional periodic callback
    async fn timer_callback(&mut self) -> Result<()> {
        Ok(())
    }
}
