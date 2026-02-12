//! Retry logic for failed service initializations
//!
//! Periodically checks for services in Uninitialized or Failed state
//! and attempts to reinitialize them according to their retry configuration.

use super::state::{FdWrapper, ManagedService, ServiceState, lock_or_recover};
use crate::service::InitResult;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use libc;
use tokio::io::unix::AsyncFd;
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Spawn a task that periodically retries initialization of failed services.
///
/// The task exits when `token` is cancelled.
pub(crate) fn spawn_retry_task(
    services: Arc<HashMap<String, Arc<ManagedService>>>,
    token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut retry_interval = interval(Duration::from_secs(1));
        retry_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = retry_interval.tick() => {
                    retry_failed_services(&services).await;
                }
            }
        }
    })
}

/// Attempt initialization for services that need retry.
///
/// Handles both `Uninitialized` (init never succeeded or previous init failed)
/// and `Failed` (post-init failure, e.g. AsyncFd registration) states.
async fn retry_failed_services(services: &HashMap<String, Arc<ManagedService>>) {
    for (name, managed) in services {
        let state = managed.load_state();
        if state != ServiceState::Uninitialized && state != ServiceState::Failed {
            continue;
        }

        let config = &managed.config;

        // Check if this is a retry or first attempt
        let now = Instant::now();
        let is_first_attempt = lock_or_recover(&managed.last_init_attempt, "last_init_attempt")
            .is_none();

        // Allow first attempt for all services, but block retries for non-restartable services
        if !is_first_attempt && !config.is_restartable {
            continue;
        }

        // Check retry throttle (only for retries)
        if let Some(last) = *lock_or_recover(&managed.last_init_attempt, "last_init_attempt") {
            if now.duration_since(last) < config.retry_interval {
                continue;
            }
        }

        // Attempt initialization
        *lock_or_recover(&managed.last_init_attempt, "last_init_attempt") = Some(now);
        managed.store_state(ServiceState::Initializing);

        debug!(service = %name, "Attempting to initialize service");

        let mut service = managed.service.lock().await;

        // Check state again after acquiring service lock to prevent race with finalization
        // Service could have transitioned to Finalizing between state check and lock acquisition
        if managed.load_state() == ServiceState::Finalizing {
            debug!(service = %name, "Service finalizing during init attempt, aborting");
            continue;
        }

        match service.initialize().await {
            Ok(InitResult::WithFileDescriptor(fd)) => match AsyncFd::new(FdWrapper(fd)) {
                Ok(async_fd) => {
                    *lock_or_recover(&managed.async_fd, "async_fd") = Some(Arc::new(async_fd));
                    managed.store_state(ServiceState::Running);
                    managed
                        .error_count
                        .store(0, std::sync::atomic::Ordering::SeqCst);
                    info!(service = %name, fd, "Service initialized successfully");
                }
                Err(e) => {
                    error!(service = %name, fd, error = %e, "Failed to register fd");
                    // Finalize to avoid resource leak before marking failed
                    let finalize_error_occurred = if let Err(fe) = service.finalize().await {
                        error!(
                            service = %name,
                            error = %fe,
                            "Error finalizing after fd registration failure"
                        );
                        true
                    } else {
                        false
                    };
                    // Restartable services will retry initialization, so we avoid
                    // forcing a best-effort close that might race with cleanup.
                    if finalize_error_occurred && !config.is_restartable {
                        if fd < 0 {
                            debug!(service = %name, fd, "Skipping close for invalid fd");
                        } else if fd <= libc::STDERR_FILENO {
                            warn!(
                                service = %name,
                                fd,
                                "Refusing to close standard fd after registration failure"
                            );
                        } else {
                            // SAFETY: finalize() failed, the service is non-restartable, and the
                            // fd is not a standard stream. Best-effort close is acceptable here
                            // because the service will not be restarted.
                            let close_result = unsafe { libc::close(fd) };
                            if close_result == -1 {
                                error!(
                                    service = %name,
                                    fd,
                                    error = %std::io::Error::last_os_error(),
                                    "Failed to close fd after registration failure"
                                );
                            }
                        }
                    }
                    managed.error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    // Restartable services go back to Uninitialized for retry;
                    // non-restartable go to Failed (terminal).
                    if config.is_restartable {
                        managed.store_state(ServiceState::Uninitialized);
                    } else {
                        managed.store_state(ServiceState::Failed);
                    }
                }
            },
            Ok(InitResult::NoFileDescriptor) => {
                managed.store_state(ServiceState::Running);
                managed
                    .error_count
                    .store(0, std::sync::atomic::Ordering::SeqCst);
                info!(service = %name, "Service initialized successfully (no fd)");
            }
            Err(e) => {
                let err_count = managed
                    .error_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    + 1;

                // Only log first failure to avoid spam
                if err_count == 1 {
                    error!(service = %name, error = %e, "Failed to initialize service");
                } else {
                    debug!(service = %name, attempt = err_count, error = %e, "Service initialization failed");
                }

                managed.store_state(ServiceState::Uninitialized);
            }
        }
    }
}
