//! Managed service state types
//!
//! Contains the internal state tracking for services managed by the ServiceManager,
//! including atomic state transitions, cached configuration, and file descriptor wrappers.

use crate::service::Service;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::unix::AsyncFd;
use tracing::error;

/// Service lifecycle state
///
/// State transitions:
/// ```text
/// Uninitialized ──► Initializing ──► Running
///       ▲                │               │
///       │                │               ▼
///       │                ▼          Finalizing
///       │              Failed       (on error/
///       │           (init fail,     reinit)
///       │            non-restart)      │
///       │                               │
///       └───────────────────────────────┘
///
/// Notes:
/// - Initializing → Failed: AsyncFd registration failure for non-restartable services
/// - Initializing → Uninitialized: Init failure for restartable services (retry)
/// - Running → Finalizing: Dispatch error or reinit request
/// - Finalizing → Uninitialized: For restartable services (retry)
/// - Finalizing → Failed: For non-restartable services (terminal)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub(crate) enum ServiceState {
    /// Service not yet initialized (or awaiting retry after init failure)
    Uninitialized = 0,
    /// Service currently initializing
    Initializing = 1,
    /// Service running successfully
    Running = 2,
    /// Service failed permanently (non-restartable)
    Failed = 3,
    /// Service is being finalized (transitional state)
    Finalizing = 4,
}

/// Wrapper for raw file descriptor to implement AsRawFd
///
/// The service retains ownership of the file descriptor.
/// This wrapper only monitors it; it does not close the fd on drop.
pub(crate) struct FdWrapper(pub(crate) RawFd);

impl AsRawFd for FdWrapper {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Drop for FdWrapper {
    fn drop(&mut self) {
        // File descriptor ownership is managed by the service.
        // We just monitor it, so don't close it here.
    }
}

/// Lock a mutex, recovering from poisoning by logging and taking the inner value.
///
/// Mutex poisoning occurs when another thread panics while holding the lock.
/// For the service manager, a poisoned lock should not crash the entire
/// service manager, so we log and continue with the inner state. Use this helper
/// when best-effort recovery is preferred over propagating a panic.
pub(crate) fn lock_or_recover<T>(mutex: &Mutex<T>, context: &str) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(error) => {
            error!(context, "Mutex poisoned, recovering");
            error.into_inner()
        }
    }
}

/// Cached service configuration, captured at registration time to avoid
/// repeated mutex acquisition during hot paths.
pub(crate) struct ServiceConfig {
    pub(crate) timer_period: Option<Duration>,
    pub(crate) dispatch_interval: Duration,
    pub(crate) is_restartable: bool,
    pub(crate) retry_interval: Duration,
}

/// Internal wrapper tracking the state of a managed service
pub(crate) struct ManagedService {
    /// The service implementation
    pub(crate) service: tokio::sync::Mutex<Box<dyn Service>>,
    /// Current service state (atomic for lock-free reads)
    state: AtomicU8,
    /// Consecutive error count (reset on successful initialization)
    pub(crate) error_count: AtomicU64,
    /// Last initialization attempt timestamp
    pub(crate) last_init_attempt: Mutex<Option<Instant>>,
    /// Async file descriptor for event monitoring (if applicable)
    pub(crate) async_fd: Mutex<Option<Arc<AsyncFd<FdWrapper>>>>,
    /// Last timer callback invocation
    pub(crate) last_timer_invoke: Mutex<Option<Instant>>,
    /// Cached configuration
    pub(crate) config: ServiceConfig,
}

impl ManagedService {
    pub(crate) fn new(service: Box<dyn Service>) -> Self {
        let config = ServiceConfig {
            timer_period: service.timer_period(),
            dispatch_interval: service.dispatch_interval(),
            is_restartable: service.is_restartable(),
            retry_interval: service.retry_interval(),
        };
        Self {
            service: tokio::sync::Mutex::new(service),
            state: AtomicU8::new(ServiceState::Uninitialized as u8),
            error_count: AtomicU64::new(0),
            last_init_attempt: Mutex::new(None),
            async_fd: Mutex::new(None),
            last_timer_invoke: Mutex::new(None),
            config,
        }
    }

    /// Load the current service state atomically
    pub(crate) fn load_state(&self) -> ServiceState {
        ServiceState::try_from(self.state.load(Ordering::Acquire))
            .expect("invalid ServiceState value in atomic")
    }

    /// Store a new service state atomically
    pub(crate) fn store_state(&self, state: ServiceState) {
        self.state.store(state as u8, Ordering::Release);
    }
}
