//! Status Sync Service
//!
//! This service synchronizes ephemeral status data across the cluster using a separate
//! DFSM instance with the "pve_kvstore_v1" CPG group.
//!
//! Equivalent to C implementation's service_status (the kvstore DFSM).
//! Handles synchronization of:
//! - RRD data (performance metrics from each node)
//! - Node IP addresses
//! - Cluster log entries
//! - Other ephemeral status key-value data

use async_trait::async_trait;
use pmxcfs_services::{Service, ServiceError};
use rust_corosync::CsError;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

use crate::Dfsm;
use crate::message::Message;

/// Status Sync Service
///
/// Synchronizes ephemeral status data across all nodes using a separate DFSM instance.
/// Uses CPG group "pve_kvstore_v1" (separate from main config database "pmxcfs_v1").
///
/// This implements the Service trait to provide:
/// - Automatic retry if CPG initialization fails
/// - Event-driven CPG dispatching for status replication
/// - Separation of status data from config data for better performance
///
/// This is equivalent to C implementation's service_status (the kvstore DFSM).
///
/// The generic parameter `M` specifies the message type this service handles.
pub struct StatusSyncService<M> {
    dfsm: Arc<Dfsm<M>>,
    fd: Option<i32>,
}

impl<M: Message> StatusSyncService<M> {
    /// Create a new status sync service
    pub fn new(dfsm: Arc<Dfsm<M>>) -> Self {
        Self { dfsm, fd: None }
    }
}

#[async_trait]
impl<M: Message> Service for StatusSyncService<M> {
    fn name(&self) -> &str {
        "status-sync"
    }

    async fn initialize(&mut self) -> pmxcfs_services::Result<std::os::unix::io::RawFd> {
        info!("Initializing status sync service (kvstore)");

        // Initialize CPG connection for kvstore group
        self.dfsm.init_cpg().map_err(|e| {
            ServiceError::InitializationFailed(format!(
                "Status sync CPG initialization failed: {e}"
            ))
        })?;

        // Get file descriptor for event monitoring
        let fd = self.dfsm.fd_get().map_err(|e| {
            self.dfsm.stop_services().ok();
            ServiceError::InitializationFailed(format!("Failed to get status sync fd: {e}"))
        })?;

        self.fd = Some(fd);

        info!(
            "Status sync service initialized successfully with fd {}",
            fd
        );
        Ok(fd)
    }

    async fn dispatch(&mut self) -> pmxcfs_services::Result<bool> {
        match self.dfsm.dispatch_events() {
            Ok(_) => Ok(true),
            Err(CsError::CsErrLibrary) | Err(CsError::CsErrBadHandle) => {
                warn!("Status sync connection lost, requesting reinitialization");
                Ok(false)
            }
            Err(e) => {
                error!("Status sync dispatch failed: {}", e);
                Err(ServiceError::DispatchFailed(format!(
                    "Status sync dispatch failed: {e}"
                )))
            }
        }
    }

    async fn finalize(&mut self) -> pmxcfs_services::Result<()> {
        info!("Finalizing status sync service");

        self.fd = None;

        if let Err(e) = self.dfsm.stop_services() {
            warn!("Error stopping status sync services: {}", e);
        }

        info!("Status sync service finalized");
        Ok(())
    }

    async fn timer_callback(&mut self) -> pmxcfs_services::Result<()> {
        // Status sync doesn't need periodic verification like the main database
        // Status data is ephemeral and doesn't require the same consistency guarantees
        Ok(())
    }

    fn timer_period(&self) -> Option<Duration> {
        // No periodic timer needed for status sync
        None
    }
}
