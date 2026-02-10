/// Status information and monitoring
///
/// This module manages:
/// - Cluster membership (nodes, IPs, online status)
/// - RRD (Round Robin Database) data for metrics
/// - Cluster log
/// - Node status information
/// - VM/CT list tracking
mod status;
mod traits;
mod types;

// Re-export public types
pub use pmxcfs_api_types::{VmEntry, VmType};
pub use types::{ClusterInfo, ClusterLogEntry, ClusterNode, NodeStatus};

// Re-export Status struct and trait
pub use status::Status;
pub use traits::{BoxFuture, MockStatus, StatusOps};

use std::sync::Arc;

/// Initialize status subsystem without RRD persistence
///
/// DEPRECATED: Use init_with_config() instead. Config is required (matches C semantics).
/// This function is kept for backward compatibility but will be removed.
#[deprecated(note = "Use init_with_config() instead - config is required")]
pub fn init() -> Arc<Status> {
    // Create a default config for backward compatibility
    let config = pmxcfs_config::Config::shared(
        "localhost".to_string(),
        "127.0.0.1".parse().unwrap(),
        33,
        false,
        true, // local mode
        "pmxcfs".to_string(),
    );
    tracing::warn!("Using deprecated init() - config should be provided explicitly");
    Arc::new(Status::new(config, None))
}

/// Initialize status subsystem with configuration
///
/// Creates a Status instance with the global configuration.
/// Config is REQUIRED (matches C semantics where cfs is always present).
pub fn init_with_config(config: Arc<pmxcfs_config::Config>) -> Arc<Status> {
    tracing::info!("Status subsystem initialized with config");
    Arc::new(Status::new(config, None))
}

/// Initialize status subsystem with RRD file persistence
///
/// DEPRECATED: Use init_with_config_and_rrd() instead. Config is required (matches C semantics).
#[deprecated(note = "Use init_with_config_and_rrd() instead - config is required")]
pub async fn init_with_rrd<P: AsRef<std::path::Path>>(rrd_dir: P) -> Arc<Status> {
    let config = pmxcfs_config::Config::shared(
        "localhost".to_string(),
        "127.0.0.1".parse().unwrap(),
        33,
        false,
        true, // local mode
        "pmxcfs".to_string(),
    );
    tracing::warn!("Using deprecated init_with_rrd() - config should be provided explicitly");
    init_with_config_and_rrd(config, rrd_dir).await
}

/// Initialize status subsystem with full configuration and RRD persistence
///
/// Creates a Status instance with both configuration and RRD persistence.
/// This is the recommended initialization for production use.
/// Config is REQUIRED (matches C semantics where cfs is always present).
pub async fn init_with_config_and_rrd<P: AsRef<std::path::Path>>(
    config: Arc<pmxcfs_config::Config>,
    rrd_dir: P,
) -> Arc<Status> {
    let rrd_dir_path = rrd_dir.as_ref();
    let rrd_writer = match pmxcfs_rrd::RrdWriter::new(rrd_dir_path).await {
        Ok(writer) => {
            tracing::info!(
                directory = %rrd_dir_path.display(),
                "RRD file persistence enabled"
            );
            Some(writer)
        }
        Err(e) => {
            tracing::warn!(error = %e, "RRD file persistence disabled");
            None
        }
    };

    tracing::info!("Status subsystem initialized with config and RRD");
    Arc::new(Status::new(config, rrd_writer))
}
