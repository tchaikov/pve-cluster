//! Service manager for orchestrating multiple managed services
//!
//! The [`ServiceManager`] handles automatic retry, error tracking, event dispatching,
//! and timer callbacks for all registered services. It uses tokio for async I/O
//! and provides graceful shutdown via a [`CancellationToken`].
//!
//! ## Shutdown sequence
//!
//! 1. `shutdown_token.cancel()` signals all background tasks to stop
//! 2. Retry, timer, and dispatch tasks observe cancellation and exit their loops
//! 3. All tasks are awaited to completion
//! 4. `finalize()` is called on every service regardless of state (idempotent)

mod dispatch;
mod retry;
pub(crate) mod state;
mod timer;

use state::{ManagedService, ServiceState, lock_or_recover};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Service manager for orchestrating multiple services
///
/// Provides:
/// - Automatic retry of failed initializations
/// - Event-driven dispatching for file descriptor-based services
/// - Periodic polling for services without file descriptors
/// - Timer callbacks for periodic maintenance
/// - Error tracking and throttled logging
/// - Graceful shutdown with finalization of all services
pub struct ServiceManager {
    /// Registered services by name
    services: HashMap<String, Arc<ManagedService>>,
    /// Cancellation token for graceful shutdown
    shutdown_token: CancellationToken,
}

/// Handle for querying service manager state while it runs.
#[derive(Clone)]
pub struct ServiceManagerHandle {
    services: Arc<HashMap<String, Arc<ManagedService>>>,
}

impl ServiceManagerHandle {
    /// Check if a service is currently in the Failed state.
    pub fn is_failed(&self, name: &str) -> Option<bool> {
        self.services
            .get(name)
            .map(|managed| managed.load_state() == ServiceState::Failed)
    }
}

impl ServiceManager {
    /// Create a new service manager
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
            shutdown_token: CancellationToken::new(),
        }
    }

    /// Add a service to be managed
    ///
    /// Service configuration (timer period, dispatch interval, restartability,
    /// retry interval) is cached at registration time.
    ///
    /// # Panics
    ///
    /// Panics if a service with the same name is already registered.
    pub fn add_service(&mut self, service: Box<dyn crate::service::Service>) {
        let name = service.name().to_string();

        if self.services.contains_key(&name) {
            panic!("Service '{name}' is already registered");
        }

        let managed = Arc::new(ManagedService::new(service));
        self.services.insert(name, managed);
    }

    /// Get a handle to trigger shutdown
    ///
    /// Call `cancel()` on the returned token to initiate graceful shutdown.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// Spawn the service manager in a background task
    ///
    /// Returns a `JoinHandle` that can be used to await completion.
    /// To gracefully shut down, call `shutdown_token().cancel()` then await the handle.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let shutdown_token = manager.shutdown_token();
    /// let handle = manager.spawn();
    /// // ... later ...
    /// shutdown_token.cancel();  // Signal graceful shutdown
    /// handle.await;             // Wait for all services to finalize
    /// ```
    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move { self.run().await })
    }

    /// Spawn the service manager and return a handle for querying state.
    pub fn spawn_with_handle(self) -> (JoinHandle<()>, ServiceManagerHandle) {
        // Clone the map to share state safely; values are Arcs to ManagedService.
        let services = Arc::new(self.services.clone());
        let handle = tokio::spawn(async move { self.run().await });
        (handle, ServiceManagerHandle { services })
    }

    /// Run the service manager until shutdown is requested.
    ///
    /// Spawns retry, timer, and dispatch tasks. On cancellation, stops all
    /// tasks first, then finalizes every service.
    async fn run(self) {
        info!(
            "Starting ServiceManager with {} services",
            self.services.len()
        );

        let services = Arc::new(self.services);
        let token = self.shutdown_token.clone();

        // Spawn background tasks, each observing the cancellation token
        let retry_handle = retry::spawn_retry_task(Arc::clone(&services), token.clone());
        let timer_handle = timer::spawn_timer_task(Arc::clone(&services), token.clone());
        let dispatch_handles =
            dispatch::spawn_dispatch_tasks(Arc::clone(&services), token.clone());

        // Wait for shutdown signal
        self.shutdown_token.cancelled().await;

        info!("ServiceManager shutting down...");

        // 1. Wait for all background tasks to stop (they observe the token)
        let _ = retry_handle.await;
        let _ = timer_handle.await;
        for handle in dispatch_handles {
            let _ = handle.await;
        }

        // 2. Finalize all services (after tasks have stopped, to avoid races)
        shutdown_all_services(&services).await;

        info!("ServiceManager stopped");
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Finalize all services regardless of current state.
///
/// In the C implementation, finalize is called unconditionally for all services.
/// Services should implement idempotent finalize() to handle being called
/// in any state.
async fn shutdown_all_services(services: &HashMap<String, Arc<ManagedService>>) {
    for (name, managed) in services {
        let state = managed.load_state();

        // Mark as finalizing to prevent any races with lingering task activity
        managed.store_state(ServiceState::Finalizing);
        *lock_or_recover(&managed.async_fd, "async_fd") = None;

        if state == ServiceState::Running || state == ServiceState::Initializing {
            info!(service = %name, "Shutting down service");
        } else {
            info!(service = %name, ?state, "Finalizing service");
        }

        let mut service = managed.service.lock().await;

        if let Err(e) = service.finalize().await {
            if state == ServiceState::Running || state == ServiceState::Initializing {
                error!(service = %name, error = %e, "Error finalizing service");
            } else {
                warn!(service = %name, error = %e, "Error finalizing service (was {:?})", state);
            }
        }
    }
}
