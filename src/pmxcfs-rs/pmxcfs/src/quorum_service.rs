//! Quorum service for cluster membership tracking
//!
//! This service tracks quorum status via Corosync quorum API and updates Status.
//! It implements the Service trait for automatic retry and lifecycle management.

use async_trait::async_trait;
use parking_lot::RwLock;
use pmxcfs_services::{Service, ServiceError};
use rust_corosync::{self as corosync, CsError, NodeId, quorum};
use std::sync::Arc;

use pmxcfs_status::Status;

/// Quorum service (matching C's service_quorum)
///
/// Tracks cluster quorum status and member list changes. Automatically
/// retries connection if Corosync is unavailable or restarts.
pub struct QuorumService {
    quorum_handle: RwLock<Option<quorum::Handle>>,
    status: Arc<Status>,
    /// Context pointer for callbacks (leaked Arc)
    context_ptr: RwLock<Option<u64>>,
}

impl QuorumService {
    /// Create a new quorum service
    pub fn new(status: Arc<Status>) -> Self {
        Self {
            quorum_handle: RwLock::new(None),
            status,
            context_ptr: RwLock::new(None),
        }
    }

    /// Check if cluster is quorate (delegates to Status)
    pub fn is_quorate(&self) -> bool {
        self.status.is_quorate()
    }
}

#[async_trait]
impl Service for QuorumService {
    fn name(&self) -> &str {
        "quorum"
    }

    async fn initialize(&mut self) -> pmxcfs_services::Result<std::os::unix::io::RawFd> {
        tracing::info!("Initializing quorum tracking");

        // Quorum notification callback
        fn quorum_notification(
            handle: &quorum::Handle,
            quorate: bool,
            ring_id: quorum::RingId,
            member_list: Vec<NodeId>,
        ) {
            tracing::info!(
                "Quorum notification: quorate={}, ring_id=({},{}), members={:?}",
                quorate,
                u32::from(ring_id.nodeid),
                ring_id.seq,
                member_list
            );

            if quorate {
                tracing::info!("Cluster is now quorate with {} members", member_list.len());
            } else {
                tracing::warn!("Cluster lost quorum");
            }

            // Retrieve QuorumService from handle context
            let context = match quorum::context_get(*handle) {
                Ok(ctx) => ctx,
                Err(e) => {
                    tracing::error!(
                        "Failed to get quorum context: {} - quorum status not updated",
                        e
                    );
                    return;
                }
            };

            if context == 0 {
                tracing::error!("BUG: Quorum context is null - quorum status not updated");
                return;
            }

            // Safety: We stored a valid Arc<QuorumService> pointer in initialize()
            unsafe {
                let service_ptr = context as *const QuorumService;
                let service = &*service_ptr;
                service.status.set_quorate(quorate);
            }
        }

        // Nodelist change notification callback
        fn nodelist_notification(
            _handle: &quorum::Handle,
            ring_id: quorum::RingId,
            member_list: Vec<NodeId>,
            joined_list: Vec<NodeId>,
            left_list: Vec<NodeId>,
        ) {
            tracing::info!(
                "Nodelist change: ring_id=({},{}), members={:?}, joined={:?}, left={:?}",
                u32::from(ring_id.nodeid),
                ring_id.seq,
                member_list,
                joined_list,
                left_list
            );
        }

        let model_data = quorum::ModelData::ModelV1(quorum::Model1Data {
            flags: quorum::Model1Flags::None,
            quorum_notification_fn: Some(quorum_notification),
            nodelist_notification_fn: Some(nodelist_notification),
        });

        // Initialize quorum connection
        let (handle, _quorum_type) = quorum::initialize(&model_data, 0).map_err(|e| {
            ServiceError::InitializationFailed(format!("quorum_initialize failed: {e:?}"))
        })?;

        // Store self pointer as context for callbacks
        // We create a stable pointer that won't move - it's a pointer to self
        // which is already on the heap as part of the Box<dyn Service>
        let self_ptr = self as *const Self as u64;
        quorum::context_set(handle, self_ptr).map_err(|e| {
            quorum::finalize(handle).ok();
            ServiceError::InitializationFailed(format!("Failed to set quorum context: {e:?}"))
        })?;

        *self.context_ptr.write() = Some(self_ptr);
        tracing::debug!("Stored QuorumService context: 0x{:x}", self_ptr);

        // Start tracking
        quorum::trackstart(handle, corosync::TrackFlags::Changes).map_err(|e| {
            quorum::finalize(handle).ok();
            ServiceError::InitializationFailed(format!("quorum_trackstart failed: {e:?}"))
        })?;

        // Get file descriptor for event monitoring
        let fd = quorum::fd_get(handle).map_err(|e| {
            quorum::finalize(handle).ok();
            ServiceError::InitializationFailed(format!("quorum_fd_get failed: {e:?}"))
        })?;

        // Dispatch once to get initial state
        if let Err(e) = quorum::dispatch(handle, corosync::DispatchFlags::One) {
            tracing::warn!("Initial quorum dispatch failed: {:?}", e);
        }

        *self.quorum_handle.write() = Some(handle);

        tracing::info!("Quorum tracking initialized successfully with fd {}", fd);
        Ok(fd)
    }

    async fn dispatch(&mut self) -> pmxcfs_services::Result<bool> {
        let handle = self.quorum_handle.read().ok_or_else(|| {
            ServiceError::DispatchFailed("Quorum handle not initialized".to_string())
        })?;

        // Dispatch all pending events
        match quorum::dispatch(handle, corosync::DispatchFlags::All) {
            Ok(_) => Ok(true),
            Err(CsError::CsErrTryAgain) => {
                // TRY_AGAIN is expected, continue normally
                Ok(true)
            }
            Err(CsError::CsErrLibrary) | Err(CsError::CsErrBadHandle) => {
                // Connection lost, need to reinitialize
                tracing::warn!(
                    "Quorum connection lost (library error), requesting reinitialization"
                );
                Ok(false)
            }
            Err(e) => {
                tracing::error!("Quorum dispatch failed: {:?}", e);
                Err(ServiceError::DispatchFailed(format!(
                    "quorum_dispatch failed: {e:?}"
                )))
            }
        }
    }

    async fn finalize(&mut self) -> pmxcfs_services::Result<()> {
        tracing::info!("Finalizing quorum service");

        // Clear quorate status
        self.status.set_quorate(false);

        // Finalize quorum handle
        if let Some(handle) = self.quorum_handle.write().take()
            && let Err(e) = quorum::finalize(handle)
        {
            tracing::warn!("Error finalizing quorum: {:?}", e);
        }

        // Clear context pointer
        *self.context_ptr.write() = None;

        tracing::info!("Quorum service finalized");
        Ok(())
    }
}
