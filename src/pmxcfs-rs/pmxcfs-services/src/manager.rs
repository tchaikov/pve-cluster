//! Service manager for orchestrating multiple managed services
//!
//! Each service gets one task that handles:
//! - Initialization with retry (5 second interval)
//! - Event dispatch when fd is readable
//! - Timer callbacks at configured intervals

use crate::service::Service;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::unix::AsyncFd;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Wrapper for raw fd that doesn't close on drop
struct FdWrapper(RawFd);

impl AsRawFd for FdWrapper {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

/// Service manager for orchestrating multiple services
pub struct ServiceManager {
    services: HashMap<String, Box<dyn Service>>,
    shutdown_token: CancellationToken,
}

impl ServiceManager {
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
            shutdown_token: CancellationToken::new(),
        }
    }

    pub fn add_service(&mut self, service: Box<dyn Service>) {
        let name = service.name().to_string();
        if self.services.contains_key(&name) {
            panic!("Service '{name}' is already registered");
        }
        self.services.insert(name, service);
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move { self.run().await })
    }

    async fn run(self) {
        info!("Starting ServiceManager with {} services", self.services.len());

        let mut handles = Vec::new();

        for (name, service) in self.services {
            let token = self.shutdown_token.clone();
            let handle = tokio::spawn(async move {
                run_service(name, service, token).await;
            });
            handles.push(handle);
        }

        // Wait for shutdown
        self.shutdown_token.cancelled().await;
        info!("ServiceManager shutting down...");

        // Wait for all services to stop
        for handle in handles {
            let _ = handle.await;
        }

        info!("ServiceManager stopped");
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Run a single service until shutdown
async fn run_service(name: String, mut service: Box<dyn Service>, token: CancellationToken) {
    // Service state
    let running = Arc::new(AtomicBool::new(false));
    let async_fd: Arc<Mutex<Option<Arc<AsyncFd<FdWrapper>>>>> = Arc::new(Mutex::new(None));
    let last_timer = Arc::new(Mutex::new(None::<Instant>));
    let mut last_init_attempt = None::<Instant>;

    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            _ = service_loop(&name, &mut service, &running, &async_fd, &last_timer, &mut last_init_attempt) => {}
        }
    }

    // Finalize on shutdown
    running.store(false, Ordering::Release);
    *async_fd.lock() = None;

    info!(service = %name, "Shutting down service");
    if let Err(e) = service.finalize().await {
        error!(service = %name, error = %e, "Error finalizing service");
    }
}

/// Main service loop
async fn service_loop(
    name: &str,
    service: &mut Box<dyn Service>,
    running: &Arc<AtomicBool>,
    async_fd: &Arc<Mutex<Option<Arc<AsyncFd<FdWrapper>>>>>,
    last_timer: &Arc<Mutex<Option<Instant>>>,
    last_init_attempt: &mut Option<Instant>,
) {
    if !running.load(Ordering::Acquire) {
        // Need to initialize
        if let Some(last) = last_init_attempt {
            let elapsed = Instant::now().duration_since(*last);
            if elapsed < std::time::Duration::from_secs(5) {
                // Wait for retry interval
                tokio::time::sleep(std::time::Duration::from_secs(5) - elapsed).await;
                return;
            }
        }

        *last_init_attempt = Some(Instant::now());

        match service.initialize().await {
            Ok(fd) => {
                match AsyncFd::new(FdWrapper(fd)) {
                    Ok(afd) => {
                        *async_fd.lock() = Some(Arc::new(afd));
                        running.store(true, Ordering::Release);
                        info!(service = %name, "Service initialized");
                    }
                    Err(e) => {
                        error!(service = %name, error = %e, "Failed to register fd");
                        let _ = service.finalize().await;
                    }
                }
            }
            Err(e) => {
                error!(service = %name, error = %e, "Initialization failed");
            }
        }
    } else {
        // Service is running - dispatch events and timers
        let fd = async_fd.lock().clone();
        if let Some(fd) = fd {
            dispatch_service(name, service, &fd, running, last_timer).await;
        }
    }
}

/// Dispatch events for a running service
async fn dispatch_service(
    name: &str,
    service: &mut Box<dyn Service>,
    async_fd: &Arc<AsyncFd<FdWrapper>>,
    running: &Arc<AtomicBool>,
    last_timer: &Arc<Mutex<Option<Instant>>>,
) {
    // Calculate timer deadline
    let timer_deadline = service.timer_period().and_then(|period| {
        let last = last_timer.lock();
        match *last {
            Some(t) => {
                let elapsed = Instant::now().duration_since(t);
                if elapsed >= period {
                    Some(Instant::now())
                } else {
                    Some(t + period)
                }
            }
            None => Some(Instant::now()),
        }
    });

    tokio::select! {
        biased;

        // Timer callback
        _ = async {
            if let Some(deadline) = timer_deadline {
                tokio::time::sleep_until(deadline.into()).await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            *last_timer.lock() = Some(Instant::now());
            debug!(service = %name, "Timer callback");
            if let Err(e) = service.timer_callback().await {
                warn!(service = %name, error = %e, "Timer callback failed");
            }
        }

        // Fd readable
        result = async_fd.readable() => {
            match result {
                Ok(mut guard) => {
                    match service.dispatch().await {
                        Ok(true) => {
                            guard.clear_ready();
                        }
                        Ok(false) => {
                            info!(service = %name, "Service requested reinitialization");
                            guard.clear_ready();
                            reinitialize(name, service, running).await;
                        }
                        Err(e) => {
                            error!(service = %name, error = %e, "Dispatch failed");
                            guard.clear_ready();
                            reinitialize(name, service, running).await;
                        }
                    }
                }
                Err(e) => {
                    warn!(service = %name, error = %e, "Error waiting for fd");
                    reinitialize(name, service, running).await;
                }
            }
        }
    }
}

/// Reinitialize a service
async fn reinitialize(name: &str, service: &mut Box<dyn Service>, running: &Arc<AtomicBool>) {
    debug!(service = %name, "Reinitializing service");
    running.store(false, Ordering::Release);

    if let Err(e) = service.finalize().await {
        warn!(service = %name, error = %e, "Error finalizing service");
    }
}
