//! Event dispatching for managed services
//!
//! Handles both file-descriptor-driven (event-based) and polling-based dispatch
//! modes. Each service gets its own dispatch task.

use super::state::{FdWrapper, ManagedService, ServiceState, lock_or_recover};
use crate::service::DispatchAction;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::unix::AsyncFd;
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Spawn a dispatch task for each registered service.
///
/// Each task waits for its service to reach Running state, then dispatches
/// events using the appropriate mode (fd-driven or polling). Tasks exit
/// when `token` is cancelled.
pub(crate) fn spawn_dispatch_tasks(
    services: Arc<HashMap<String, Arc<ManagedService>>>,
    token: CancellationToken,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::new();

    for (name, managed) in services.iter() {
        let name = name.clone();
        let managed = Arc::clone(managed);
        let token = token.clone();

        let handle = tokio::spawn(async move {
            loop {
                // Wait for service to be running (or shutdown)
                loop {
                    tokio::select! {
                        _ = token.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {
                            if managed.load_state() == ServiceState::Running {
                                break;
                            }
                        }
                    }
                }

                // Dispatch based on service type
                let async_fd = lock_or_recover(&managed.async_fd, "async_fd").clone();

                if let Some(fd) = async_fd {
                    dispatch_with_fd(&name, &managed, &fd, &token).await;
                } else {
                    dispatch_polling(&name, &managed, &token).await;
                }
            }
        });

        handles.push(handle);
    }

    handles
}

/// Dispatch events for a service with a file descriptor.
///
/// Waits for the fd to become readable and calls `dispatch()`. On error
/// or reinitialize request, the service is finalized and marked for retry.
async fn dispatch_with_fd(
    name: &str,
    managed: &Arc<ManagedService>,
    async_fd: &Arc<AsyncFd<FdWrapper>>,
    token: &CancellationToken,
) {
    loop {
        if managed.load_state() != ServiceState::Running {
            debug!(service = %name, "Service no longer running, stopping dispatch");
            break;
        }

        let readable = tokio::select! {
            _ = token.cancelled() => return,
            result = async_fd.readable() => {
                match result {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(service = %name, error = %e, "Error waiting for fd readability");
                        // Reinitialize on fd error to avoid getting stuck
                        reinitialize_service(name, managed).await;
                        break;
                    }
                }
            }
        };

        // Check state again after waiting (service might have been finalized)
        if managed.load_state() != ServiceState::Running {
            debug!(service = %name, "Service finalized while waiting, stopping dispatch");
            break;
        }

        let mut guard = readable;
        let mut service = managed.service.lock().await;

        // Check state once more after acquiring service lock to prevent dispatch during finalization
        // Service could have transitioned between state check and lock acquisition
        if managed.load_state() != ServiceState::Running {
            debug!(service = %name, "Service no longer running after lock acquisition, aborting dispatch");
            break;
        }

        match service.dispatch().await {
            Ok(DispatchAction::Continue) => {
                guard.clear_ready();
            }
            Ok(DispatchAction::Reinitialize) => {
                info!(service = %name, "Service requested reinitialization");
                guard.clear_ready();
                drop(service);
                reinitialize_service(name, managed).await;
                break;
            }
            Err(e) => {
                error!(service = %name, error = %e, "Service dispatch failed");
                guard.clear_ready();
                drop(service);
                reinitialize_service(name, managed).await;
                break;
            }
        }
    }
}

/// Dispatch events for a service without a file descriptor (polling mode).
///
/// Calls `dispatch()` at the service's configured `dispatch_interval`.
async fn dispatch_polling(
    name: &str,
    managed: &Arc<ManagedService>,
    token: &CancellationToken,
) {
    let dispatch_interval = managed.config.dispatch_interval;
    let mut interval_timer = interval(dispatch_interval);
    interval_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = token.cancelled() => return,
            _ = interval_timer.tick() => {}
        }

        if managed.load_state() != ServiceState::Running {
            break;
        }

        let mut service = managed.service.lock().await;

        match service.dispatch().await {
            Ok(DispatchAction::Continue) => {}
            Ok(DispatchAction::Reinitialize) => {
                info!(service = %name, "Service requested reinitialization");
                drop(service);
                reinitialize_service(name, managed).await;
                break;
            }
            Err(e) => {
                error!(service = %name, error = %e, "Service dispatch failed");
                drop(service);
                reinitialize_service(name, managed).await;
                break;
            }
        }
    }
}

/// Reinitialize a service: set transitional state, finalize, then mark for retry.
///
/// For non-restartable services, the service is moved to Failed (terminal)
/// instead of Uninitialized.
pub(super) async fn reinitialize_service(name: &str, managed: &Arc<ManagedService>) {
    debug!(service = %name, "Reinitializing service");

    // Set state to Finalizing and clear async_fd BEFORE calling finalize()
    // to prevent race conditions:
    // 1. Dispatch tasks see non-Running state and stop
    // 2. No new fd reads can start after we clear async_fd
    // 3. Timer callbacks won't fire (they check for Running state)
    managed.store_state(ServiceState::Finalizing);
    *lock_or_recover(&managed.async_fd, "async_fd") = None;

    let mut service = managed.service.lock().await;

    if let Err(e) = service.finalize().await {
        warn!(service = %name, error = %e, "Error finalizing service");
    }

    drop(service);

    // Restartable services go back to Uninitialized for retry;
    // non-restartable go to Failed (terminal).
    if managed.config.is_restartable {
        managed.store_state(ServiceState::Uninitialized);
        managed
            .error_count
            .store(0, std::sync::atomic::Ordering::SeqCst);
    } else {
        managed.store_state(ServiceState::Failed);
    }
}
