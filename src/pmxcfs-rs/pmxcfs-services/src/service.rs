//! Service trait and related types
//!
//! This module provides the core abstraction for managed services that can
//! automatically retry initialization, handle errors gracefully, and provide
//! timer-based periodic callbacks.

use crate::error::Result;
use async_trait::async_trait;
use std::os::unix::io::RawFd;
use std::time::Duration;

/// A managed service that can be monitored and restarted automatically
///
/// This trait provides the core abstraction for services in the pmxcfs daemon.
/// Services implementing this trait gain automatic retry on failure, graceful
/// error handling, and optional periodic timer callbacks.
///
/// ## Lifecycle
///
/// 1. **Uninitialized** - Service created but not yet initialized
/// 2. **Initializing** - `initialize()` in progress
/// 3. **Running** - Service initialized successfully, dispatching events
/// 4. **Failed** - Service encountered an error, will retry if restartable
#[async_trait]
pub trait Service: Send + Sync {
    /// Service name for logging and identification
    ///
    /// Should be a short, descriptive identifier (e.g., "quorum", "dfsm", "confdb")
    fn name(&self) -> &str;

    /// Initialize the service
    ///
    /// Called when the service is first started or after a failure (if restartable).
    /// Returns an `InitResult` indicating whether the service needs file descriptor
    /// monitoring.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails. The ServiceManager will automatically
    /// retry initialization based on `retry_interval()` if `is_restartable()` returns true.
    ///
    /// # File Descriptor Ownership
    ///
    /// **CRITICAL:** If returning `InitResult::WithFileDescriptor(fd)`:
    /// - The service **retains ownership** of the file descriptor
    /// - The ServiceManager only **monitors** the fd for readability
    /// - The service **must keep the fd valid** until `finalize()` is called
    /// - The service **must close the fd** in `finalize()`, not before
    /// - Closing the fd before `finalize()` may cause use-after-close errors
    ///
    /// **Lifecycle guarantee:**
    /// ```text
    /// initialize() returns fd
    ///     ↓
    /// ServiceManager monitors fd (AsyncFd)
    ///     ↓
    /// dispatch() called when fd is readable
    ///     ↓
    /// finalize() called (service must close fd here)
    ///     ↓
    /// ServiceManager stops monitoring fd
    /// ```
    ///
    /// # Implementation Notes
    ///
    /// - Initialize connections to external services (Corosync, CPG, etc.)
    /// - Set up internal state
    /// - Return file descriptor if the service needs event-driven dispatching
    /// - Keep initialization lightweight - heavy work should be in `dispatch()`
    async fn initialize(&mut self) -> Result<InitResult>;

    /// Handle events for this service
    ///
    /// Called when:
    /// - The file descriptor returned by `initialize()` becomes readable (if WithFileDescriptor)
    /// - Periodically for services without file descriptors (if NoFileDescriptor)
    ///
    /// # Returns
    ///
    /// - `DispatchAction::Continue` - Continue normal operation
    /// - `DispatchAction::Reinitialize` - Request reinitialization (triggers `finalize()` then `initialize()`)
    ///
    /// # Errors
    ///
    /// Errors automatically trigger reinitialization if the service is restartable.
    /// The service will be finalized and reinitialized according to `retry_interval()`.
    async fn dispatch(&mut self) -> Result<DispatchAction>;

    /// Clean up service resources
    ///
    /// Called when:
    /// - Service is being shut down
    /// - Service is being reinitialized after dispatch failure
    /// - ServiceManager is shutting down
    ///
    /// # File Descriptor Cleanup
    ///
    /// **CRITICAL:** If `initialize()` returned `InitResult::WithFileDescriptor(fd)`:
    /// - The service **must close the fd** in this method
    /// - The ServiceManager will have already stopped monitoring the fd
    /// - The fd will not be accessed by the ServiceManager after this call
    /// - Failure to close the fd will leak file descriptors
    ///
    /// # Implementation Notes
    ///
    /// - Close connections and file descriptors
    /// - Release resources
    /// - Should not fail - log errors but return Ok(())
    /// - Must be idempotent (safe to call multiple times)
    async fn finalize(&mut self) -> Result<()>;

    /// Optional periodic callback
    ///
    /// Called at the interval specified by `timer_period()` if the service is running.
    /// Useful for periodic maintenance tasks like state verification or cleanup.
    ///
    /// # Default Implementation
    ///
    /// Does nothing by default. Override to implement periodic behavior.
    async fn timer_callback(&mut self) -> Result<()> {
        Ok(())
    }

    /// Timer period for periodic callbacks
    ///
    /// If `Some(duration)`, `timer_callback()` will be invoked every `duration`.
    /// If `None`, timer callbacks are disabled.
    ///
    /// # Default
    ///
    /// Returns `None` (no timer callbacks)
    fn timer_period(&self) -> Option<Duration> {
        None
    }

    /// Whether to automatically retry initialization after failure
    ///
    /// If `true`, the ServiceManager will automatically retry `initialize()`
    /// after failures using the interval specified by `retry_interval()`.
    ///
    /// If `false`, the service will remain in a failed state after the first
    /// initialization failure.
    ///
    /// # Default
    ///
    /// Returns `true` (auto-retry enabled)
    fn is_restartable(&self) -> bool {
        true
    }

    /// Minimum interval between retry attempts
    ///
    /// When `initialize()` fails, the ServiceManager will wait at least this
    /// long before attempting to reinitialize.
    ///
    /// # Default
    ///
    /// Returns 5 seconds (matching C implementation)
    fn retry_interval(&self) -> Duration {
        Duration::from_secs(5)
    }

    /// Dispatch interval for services without file descriptors
    ///
    /// For services that return `InitResult::NoFileDescriptor`, this determines
    /// how often `dispatch()` is called.
    ///
    /// # Default
    ///
    /// Returns 100ms (matching current Rust implementation)
    fn dispatch_interval(&self) -> Duration {
        Duration::from_millis(100)
    }
}

/// Result of service initialization
#[derive(Debug, Clone, Copy)]
pub enum InitResult {
    /// Service uses a file descriptor for event notification
    ///
    /// The ServiceManager will use tokio's AsyncFd to monitor this file descriptor
    /// and call `dispatch()` when it becomes readable. This is the most efficient
    /// mode for services that interact with Corosync (quorum, CPG, cmap).
    WithFileDescriptor(RawFd),

    /// Service does not use a file descriptor
    ///
    /// The ServiceManager will call `dispatch()` periodically at the interval
    /// specified by `dispatch_interval()`. Use this for services that poll
    /// or have no external event source.
    NoFileDescriptor,
}

/// Action requested by service dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchAction {
    /// Continue normal operation
    Continue,

    /// Request reinitialization
    ///
    /// The service will be finalized and reinitialized. This is useful when
    /// the underlying connection is lost or becomes invalid.
    Reinitialize,
}
