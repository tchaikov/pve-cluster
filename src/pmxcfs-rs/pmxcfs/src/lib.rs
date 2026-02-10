// Library exports for testing and potential library usage

pub mod cluster_config_service; // Cluster configuration monitoring via CMAP (matching C's confdb.c)
pub mod daemon; // Unified daemon builder with integrated PID file management
pub mod file_lock; // File locking utilities
pub mod fuse;
pub mod ipc; // IPC subsystem (request handling and service)
pub mod logging; // Runtime-adjustable logging (for .debug plugin)
pub mod memdb_callbacks; // DFSM callbacks for memdb (glue between dfsm and memdb)
pub mod plugins;
pub mod quorum_service; // Quorum tracking service (matching C's quorum.c)
pub mod restart_flag; // Restart flag management
pub mod status_callbacks; // DFSM callbacks for status kvstore (glue between dfsm and status)
