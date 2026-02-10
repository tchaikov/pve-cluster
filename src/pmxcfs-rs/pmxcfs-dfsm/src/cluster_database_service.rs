//! Cluster Database Service
//!
//! This service synchronizes the distributed cluster database (pmxcfs-memdb) across
//! all cluster nodes using DFSM (Distributed Finite State Machine).
//!
//! Equivalent to C implementation's service_dcdb (Distributed Cluster DataBase).
//! Provides automatic retry, event-driven CPG dispatching, and periodic state verification.

use async_trait::async_trait;
use pmxcfs_services::{DispatchAction, InitResult, Service, ServiceError};
use rust_corosync::CsError;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::Dfsm;
use crate::message::Message;

/// Cluster Database Service
///
/// Synchronizes the distributed cluster database (pmxcfs-memdb) across all nodes.
/// Implements the Service trait to provide:
/// - Automatic retry if CPG initialization fails
/// - Event-driven CPG dispatching for database replication
/// - Periodic state verification via timer callback
///
/// This is equivalent to C implementation's service_dcdb (Distributed Cluster DataBase).
///
/// The generic parameter `M` specifies the message type this service handles.
pub struct ClusterDatabaseService<M> {
    dfsm: Arc<Dfsm<M>>,
    fd: Option<i32>,
}

impl<M: Message> ClusterDatabaseService<M> {
    /// Create a new cluster database service
    pub fn new(dfsm: Arc<Dfsm<M>>) -> Self {
        Self { dfsm, fd: None }
    }
}

#[async_trait]
impl<M: Message> Service for ClusterDatabaseService<M> {
    fn name(&self) -> &str {
        "cluster-database"
    }

    async fn initialize(&mut self) -> pmxcfs_services::Result<InitResult> {
        info!("Initializing cluster database service (dcdb)");

        // Initialize CPG connection (this also joins the group)
        self.dfsm.init_cpg().map_err(|e| {
            ServiceError::InitializationFailed(format!("DFSM CPG initialization failed: {e}"))
        })?;

        // Get file descriptor for event monitoring
        let fd = self.dfsm.fd_get().map_err(|e| {
            self.dfsm.stop_services().ok();
            ServiceError::InitializationFailed(format!("Failed to get DFSM fd: {e}"))
        })?;

        self.fd = Some(fd);

        info!(
            "Cluster database service initialized successfully with fd {}",
            fd
        );
        Ok(InitResult::WithFileDescriptor(fd))
    }

    async fn dispatch(&mut self) -> pmxcfs_services::Result<DispatchAction> {
        match self.dfsm.dispatch_events() {
            Ok(_) => Ok(DispatchAction::Continue),
            Err(CsError::CsErrLibrary) | Err(CsError::CsErrBadHandle) => {
                warn!("DFSM connection lost, requesting reinitialization");
                Ok(DispatchAction::Reinitialize)
            }
            Err(e) => {
                error!("DFSM dispatch failed: {}", e);
                Err(ServiceError::DispatchFailed(format!(
                    "DFSM dispatch failed: {e}"
                )))
            }
        }
    }

    async fn finalize(&mut self) -> pmxcfs_services::Result<()> {
        info!("Finalizing cluster database service");

        self.fd = None;

        if let Err(e) = self.dfsm.stop_services() {
            warn!("Error stopping cluster database services: {}", e);
        }

        info!("Cluster database service finalized");
        Ok(())
    }

    async fn timer_callback(&mut self) -> pmxcfs_services::Result<()> {
        debug!("Cluster database timer callback: initiating state verification");

        // Request state verification
        if let Err(e) = self.dfsm.verify_request() {
            warn!("DFSM state verification request failed: {}", e);
        }

        Ok(())
    }

    fn timer_period(&self) -> Option<Duration> {
        // Match C implementation's DCDB_VERIFY_TIME (60 * 60 seconds)
        // Periodic state verification happens once per hour
        Some(Duration::from_secs(3600))
    }
}
