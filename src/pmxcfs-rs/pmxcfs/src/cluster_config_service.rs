//! Cluster Configuration Service
//!
//! This service monitors Corosync cluster configuration changes via the CMAP API.
//! It tracks nodelist changes and configuration version updates, matching the C
//! implementation's service_confdb functionality.

use async_trait::async_trait;
use pmxcfs_services::{Service, ServiceError};
use rust_corosync::{self as corosync, CsError, cmap};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use pmxcfs_status::Status;

/// Cluster configuration service (matching C's service_confdb)
///
/// Monitors Corosync CMAP for:
/// - Nodelist changes (`nodelist.node.*`)
/// - Configuration version changes (`totem.config_version`)
///
/// Updates cluster info when configuration changes are detected.
pub struct ClusterConfigService {
    /// CMAP handle (None when not initialized)
    cmap_handle: parking_lot::RwLock<Option<cmap::Handle>>,
    /// Nodelist track handle
    nodelist_track_handle: parking_lot::RwLock<Option<cmap::TrackHandle>>,
    /// Config version track handle
    version_track_handle: parking_lot::RwLock<Option<cmap::TrackHandle>>,
    /// Status instance for cluster info updates
    status: Arc<Status>,
    /// Flag indicating configuration changes detected
    changes_detected: parking_lot::RwLock<bool>,
}

impl ClusterConfigService {
    /// Create a new cluster configuration service
    pub fn new(status: Arc<Status>) -> Self {
        Self {
            cmap_handle: parking_lot::RwLock::new(None),
            nodelist_track_handle: parking_lot::RwLock::new(None),
            version_track_handle: parking_lot::RwLock::new(None),
            status,
            changes_detected: parking_lot::RwLock::new(false),
        }
    }

    /// Read cluster configuration from CMAP
    fn read_cluster_config(&self, handle: &cmap::Handle) -> Result<(), anyhow::Error> {
        // Read config version
        let config_version = match cmap::get(*handle, &"totem.config_version".to_string()) {
            Ok(cmap::Data::UInt64(v)) => v,
            Ok(cmap::Data::UInt32(v)) => v as u64,
            Ok(cmap::Data::UInt16(v)) => v as u64,
            Ok(cmap::Data::UInt8(v)) => v as u64,
            Ok(_) => {
                warn!("Unexpected data type for totem.config_version");
                0
            }
            Err(e) => {
                warn!("Failed to read totem.config_version: {:?}", e);
                0
            }
        };

        // Read cluster name
        let cluster_name = match cmap::get(*handle, &"totem.cluster_name".to_string()) {
            Ok(cmap::Data::String(s)) => s,
            Ok(_) => {
                error!("totem.cluster_name has unexpected type");
                return Err(anyhow::anyhow!("Invalid cluster_name type"));
            }
            Err(e) => {
                error!("Failed to read totem.cluster_name: {:?}", e);
                return Err(anyhow::anyhow!("Failed to read cluster_name"));
            }
        };

        info!(
            "Cluster configuration: name='{}', version={}",
            cluster_name, config_version
        );

        // Read cluster nodes
        self.read_cluster_nodes(handle, &cluster_name, config_version)?;

        Ok(())
    }

    /// Read cluster nodes from CMAP nodelist
    fn read_cluster_nodes(
        &self,
        handle: &cmap::Handle,
        cluster_name: &str,
        config_version: u64,
    ) -> Result<(), anyhow::Error> {
        let mut nodes = Vec::new();

        // Iterate through nodelist (nodelist.node.0, nodelist.node.1, etc.)
        for node_idx in 0..256 {
            let nodeid_key = format!("nodelist.node.{node_idx}.nodeid");
            let name_key = format!("nodelist.node.{node_idx}.name");
            let ring0_key = format!("nodelist.node.{node_idx}.ring0_addr");

            // Try to read node ID - if it doesn't exist, we've reached the end
            let nodeid = match cmap::get(*handle, &nodeid_key) {
                Ok(cmap::Data::UInt32(id)) => id,
                Ok(cmap::Data::UInt8(id)) => id as u32,
                Ok(cmap::Data::UInt16(id)) => id as u32,
                Err(CsError::CsErrNotExist) => break, // No more nodes
                Err(e) => {
                    debug!("Error reading {}: {:?}", nodeid_key, e);
                    continue;
                }
                Ok(_) => {
                    warn!("Unexpected type for {}", nodeid_key);
                    continue;
                }
            };

            let name = match cmap::get(*handle, &name_key) {
                Ok(cmap::Data::String(s)) => s,
                _ => {
                    debug!("No name for node {}", nodeid);
                    format!("node{nodeid}")
                }
            };

            let ip = match cmap::get(*handle, &ring0_key) {
                Ok(cmap::Data::String(s)) => s,
                _ => String::new(),
            };

            debug!(
                "Found cluster node: id={}, name={}, ip={}",
                nodeid, name, ip
            );
            nodes.push((nodeid, name, ip));
        }

        info!("Found {} cluster nodes", nodes.len());

        // Update cluster info in Status
        self.status
            .update_cluster_info(cluster_name.to_string(), config_version, nodes)?;

        Ok(())
    }
}

/// CMAP track callback (matches C's track_callback)
///
/// This function is called by Corosync whenever a tracked CMAP key changes.
/// We use user_data to pass a pointer to the ClusterConfigService.
fn track_callback(
    _handle: &cmap::Handle,
    _track_handle: &cmap::TrackHandle,
    _event: cmap::TrackType,
    key_name: &String, // Note: rust-corosync API uses &String not &str
    _new_value: &cmap::Data,
    _old_value: &cmap::Data,
    user_data: u64,
) {
    debug!("CMAP track callback: key_name={}", key_name);

    if user_data == 0 {
        error!("BUG: CMAP track callback called with null user_data");
        return;
    }

    // Safety: user_data contains a valid pointer to ClusterConfigService
    // The pointer remains valid because ServiceManager holds the service
    unsafe {
        let service_ptr = user_data as *const ClusterConfigService;
        let service = &*service_ptr;
        *service.changes_detected.write() = true;
    }
}

#[async_trait]
impl Service for ClusterConfigService {
    fn name(&self) -> &str {
        "cluster-config"
    }

    async fn initialize(&mut self) -> pmxcfs_services::Result<std::os::unix::io::RawFd> {
        info!("Initializing cluster configuration service");

        // Initialize CMAP connection
        let handle = cmap::initialize(cmap::Map::Icmap).map_err(|e| {
            ServiceError::InitializationFailed(format!("cmap_initialize failed: {e:?}"))
        })?;

        // Store self pointer as user_data for callbacks
        let self_ptr = self as *const Self as u64;

        // Create callback struct
        let callback = cmap::NotifyCallback {
            notify_fn: Some(track_callback),
        };

        // Set up nodelist tracking (matches C's CMAP_TRACK_PREFIX | CMAP_TRACK_ADD | ...)
        let nodelist_track = cmap::track_add(
            handle,
            &"nodelist.node.".to_string(),
            cmap::TrackType::PREFIX
                | cmap::TrackType::ADD
                | cmap::TrackType::DELETE
                | cmap::TrackType::MODIFY,
            &callback,
            self_ptr,
        )
        .map_err(|e| {
            cmap::finalize(handle).ok();
            ServiceError::InitializationFailed(format!("cmap_track_add (nodelist) failed: {e:?}"))
        })?;

        // Set up config version tracking
        let version_track = cmap::track_add(
            handle,
            &"totem.config_version".to_string(),
            cmap::TrackType::ADD | cmap::TrackType::DELETE | cmap::TrackType::MODIFY,
            &callback,
            self_ptr,
        )
        .map_err(|e| {
            cmap::track_delete(handle, nodelist_track).ok();
            cmap::finalize(handle).ok();
            ServiceError::InitializationFailed(format!(
                "cmap_track_add (config_version) failed: {e:?}"
            ))
        })?;

        // Get file descriptor for event monitoring
        let fd = cmap::fd_get(handle).map_err(|e| {
            cmap::track_delete(handle, version_track).ok();
            cmap::track_delete(handle, nodelist_track).ok();
            cmap::finalize(handle).ok();
            ServiceError::InitializationFailed(format!("cmap_fd_get failed: {e:?}"))
        })?;

        // Read initial configuration
        if let Err(e) = self.read_cluster_config(&handle) {
            warn!("Failed to read initial cluster configuration: {}", e);
            // Don't fail initialization - we'll try again on next change
        }

        // Store handles
        *self.cmap_handle.write() = Some(handle);
        *self.nodelist_track_handle.write() = Some(nodelist_track);
        *self.version_track_handle.write() = Some(version_track);

        info!(
            "Cluster configuration service initialized successfully with fd {}",
            fd
        );
        Ok(fd)
    }

    async fn dispatch(&mut self) -> pmxcfs_services::Result<bool> {
        let handle = *self.cmap_handle.read().as_ref().ok_or_else(|| {
            ServiceError::DispatchFailed("CMAP handle not initialized".to_string())
        })?;

        // Dispatch CMAP events (matches C's cmap_dispatch with CS_DISPATCH_ALL)
        match cmap::dispatch(handle, corosync::DispatchFlags::All) {
            Ok(_) => {
                // Check if changes were detected (matches C implementation)
                if *self.changes_detected.read() {
                    *self.changes_detected.write() = false;

                    // Re-read cluster configuration
                    if let Err(e) = self.read_cluster_config(&handle) {
                        warn!("Failed to update cluster configuration: {}", e);
                    }
                }
                Ok(true)
            }
            Err(CsError::CsErrTryAgain) => {
                // TRY_AGAIN is expected, continue normally
                Ok(true)
            }
            Err(CsError::CsErrLibrary) | Err(CsError::CsErrBadHandle) => {
                // Connection lost, need to reinitialize
                warn!("CMAP connection lost, requesting reinitialization");
                Ok(false)
            }
            Err(e) => {
                error!("CMAP dispatch failed: {:?}", e);
                Err(ServiceError::DispatchFailed(format!(
                    "cmap_dispatch failed: {e:?}"
                )))
            }
        }
    }

    async fn finalize(&mut self) -> pmxcfs_services::Result<()> {
        info!("Finalizing cluster configuration service");

        if let Some(handle) = self.cmap_handle.write().take() {
            // Remove track handles
            if let Some(version_track) = self.version_track_handle.write().take() {
                cmap::track_delete(handle, version_track).ok();
            }
            if let Some(nodelist_track) = self.nodelist_track_handle.write().take() {
                cmap::track_delete(handle, nodelist_track).ok();
            }

            // Finalize CMAP connection
            if let Err(e) = cmap::finalize(handle) {
                warn!("Error finalizing CMAP: {:?}", e);
            }
        }

        info!("Cluster configuration service finalized");
        Ok(())
    }
}
