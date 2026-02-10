use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::sync::Arc;
use tracing::{debug, error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, reload, util::SubscriberInitExt};

use pmxcfs_rs::{
    cluster_config_service::ClusterConfigService,
    daemon::{Daemon, DaemonProcess},
    file_lock::FileLock,
    fuse,
    ipc::IpcHandler,
    memdb_callbacks::MemDbCallbacks,
    plugins,
    quorum_service::QuorumService,
    restart_flag::RestartFlag,
    status_callbacks::StatusCallbacks,
};

use pmxcfs_api_types::PmxcfsError;
use pmxcfs_config::Config;
use pmxcfs_dfsm::{
    Callbacks, ClusterDatabaseService, Dfsm, FuseMessage, KvStoreMessage, StatusSyncService,
};
use pmxcfs_memdb::MemDb;
use pmxcfs_services::ServiceManager;
use pmxcfs_status as status;

// Default paths matching the C version
const DEFAULT_MOUNT_DIR: &str = "/etc/pve";
const DEFAULT_DB_PATH: &str = "/var/lib/pve-cluster/config.db";
const DEFAULT_VARLIB_DIR: &str = "/var/lib/pve-cluster";
const DEFAULT_RUN_DIR: &str = "/run/pmxcfs";

/// Type alias for the cluster services tuple
type ClusterServices = (
    Arc<Dfsm<FuseMessage>>,
    Arc<Dfsm<KvStoreMessage>>,
    Arc<QuorumService>,
);

/// Proxmox Cluster File System - Rust implementation
///
/// This FUSE filesystem uses corosync and sqlite3 to provide a
/// cluster-wide, consistent view of config and other files.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Turn on debug messages
    #[arg(short = 'd', long = "debug")]
    debug: bool,

    /// Do not daemonize server
    #[arg(short = 'f', long = "foreground")]
    foreground: bool,

    /// Force local mode (ignore corosync.conf, force quorum)
    #[arg(short = 'l', long = "local")]
    local: bool,

    /// Test directory (sets all paths to subdirectories for isolated testing)
    #[arg(long = "test-dir")]
    test_dir: Option<std::path::PathBuf>,

    /// Custom mount point
    #[arg(long = "mount", default_value = DEFAULT_MOUNT_DIR)]
    mount: std::path::PathBuf,

    /// Custom database path
    #[arg(long = "db", default_value = DEFAULT_DB_PATH)]
    db: std::path::PathBuf,

    /// Custom runtime directory
    #[arg(long = "rundir", default_value = DEFAULT_RUN_DIR)]
    rundir: std::path::PathBuf,

    /// Cluster name (CPG group name for Corosync isolation)
    /// Must match C implementation's DCDB_CPG_GROUP_NAME
    #[arg(long = "cluster-name", default_value = "pve_dcdb_v1")]
    cluster_name: String,
}

/// Configuration for all filesystem paths used by pmxcfs
#[derive(Debug, Clone)]
struct PathConfig {
    dbfilename: std::path::PathBuf,
    lockfile: std::path::PathBuf,
    restart_flag_file: std::path::PathBuf,
    pid_file: std::path::PathBuf,
    mount_dir: std::path::PathBuf,
    varlib_dir: std::path::PathBuf,
    run_dir: std::path::PathBuf,
    pve2_socket_path: std::path::PathBuf, // IPC server socket (libqb-compatible)
    corosync_conf_path: std::path::PathBuf,
    rrd_dir: std::path::PathBuf,
}

impl PathConfig {
    /// Create PathConfig from command line arguments
    fn from_args(args: &Args) -> Self {
        if let Some(ref test_dir) = args.test_dir {
            // Test mode: all paths under test directory
            Self {
                dbfilename: test_dir.join("db/config.db"),
                lockfile: test_dir.join("db/.pmxcfs.lockfile"),
                restart_flag_file: test_dir.join("run/cfs-restart-flag"),
                pid_file: test_dir.join("run/pmxcfs.pid"),
                mount_dir: test_dir.join("pve"),
                varlib_dir: test_dir.join("db"),
                run_dir: test_dir.join("run"),
                pve2_socket_path: test_dir.join("run/pve2"),
                corosync_conf_path: test_dir.join("etc/corosync/corosync.conf"),
                rrd_dir: test_dir.join("rrd"),
            }
        } else {
            // Production mode: use provided args (which have defaults from clap)
            let varlib_dir = args
                .db
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_VARLIB_DIR));

            Self {
                dbfilename: args.db.clone(),
                lockfile: varlib_dir.join(".pmxcfs.lockfile"),
                restart_flag_file: args.rundir.join("cfs-restart-flag"),
                pid_file: args.rundir.join("pmxcfs.pid"),
                mount_dir: args.mount.clone(),
                varlib_dir,
                run_dir: args.rundir.clone(),
                pve2_socket_path: std::path::PathBuf::from(DEFAULT_PVE2_SOCKET),
                corosync_conf_path: std::path::PathBuf::from(HOST_CLUSTER_CONF_FN),
                rrd_dir: std::path::PathBuf::from(DEFAULT_RRD_DIR),
            }
        }
    }
}

const HOST_CLUSTER_CONF_FN: &str = "/etc/corosync/corosync.conf";

const DEFAULT_RRD_DIR: &str = "/var/lib/rrdcached/db";
const DEFAULT_PVE2_SOCKET: &str = "/var/run/pve2";

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Initialize logging
    init_logging(args.debug)?;

    // Create path configuration
    let paths = PathConfig::from_args(&args);

    info!("Starting pmxcfs (Rust version)");
    debug!("Debug mode: {}", args.debug);
    debug!("Foreground mode: {}", args.foreground);
    debug!("Local mode: {}", args.local);

    // Log test mode if enabled
    if args.test_dir.is_some() {
        info!("TEST MODE: Using isolated test directory");
        info!("  Mount: {}", paths.mount_dir.display());
        info!("  Database: {}", paths.dbfilename.display());
        info!("  QB-IPC Socket: {}", paths.pve2_socket_path.display());
        info!("  Run dir: {}", paths.run_dir.display());
        info!("  RRD dir: {}", paths.rrd_dir.display());
    }

    // Get node name (equivalent to uname in C version)
    let nodename = get_nodename()?;
    info!("Node name: {}", nodename);

    // Resolve node IP
    let node_ip = resolve_node_ip(&nodename)?;
    info!("Resolved node '{}' to IP '{}'", nodename, node_ip);

    // Get www-data group ID
    let www_data_gid = get_www_data_gid()?;
    debug!("www-data group ID: {}", www_data_gid);

    // Create configuration
    let config = Config::shared(
        nodename,
        node_ip,
        www_data_gid,
        args.debug,
        args.local,
        args.cluster_name.clone(),
    );

    // Set umask (027 = rwxr-x---)
    unsafe {
        libc::umask(0o027);
    }

    // Create required directories
    let is_test_mode = args.test_dir.is_some();
    create_directories(www_data_gid, &paths, is_test_mode)?;

    // Acquire lock
    let _lock = FileLock::acquire(paths.lockfile.clone()).await?;

    // Initialize status subsystem with config and RRD directory
    // This allows get_local_nodename() to work properly by accessing config.nodename()
    let status = status::init_with_config_and_rrd(config.clone(), &paths.rrd_dir).await;

    // Check if database exists
    let db_exists = paths.dbfilename.exists();

    // Open or create database
    let memdb = MemDb::open(&paths.dbfilename, !db_exists)?;

    // Check for corosync.conf in database
    let mut has_corosync_conf = memdb.exists("/corosync.conf")?;

    // Import corosync.conf if it exists on disk but not in database and not in local mode
    // This handles both new databases and existing databases that need the config imported
    if !has_corosync_conf && !args.local {
        // Try test-mode path first, then fall back to production path
        // This matches C behavior and handles test environments where only some nodes
        // have the test path set up (others use the shared /etc/corosync via volume)
        let import_path = if paths.corosync_conf_path.exists() {
            &paths.corosync_conf_path
        } else {
            std::path::Path::new(HOST_CLUSTER_CONF_FN)
        };

        if import_path.exists() {
            import_corosync_conf(&memdb, import_path)?;
            // Refresh the check after import
            has_corosync_conf = memdb.exists("/corosync.conf")?;
        }
    }

    // Initialize cluster services if needed (matching C's pmxcfs.c)
    let (dfsm, status_dfsm, quorum_service) = if has_corosync_conf && !args.local {
        info!("Initializing cluster services");
        let (db_dfsm, st_dfsm, quorum) = setup_cluster_services(
            &memdb,
            config.clone(),
            status.clone(),
            &paths.corosync_conf_path,
        )?;
        (Some(db_dfsm), Some(st_dfsm), Some(quorum))
    } else {
        if args.local {
            info!("Forcing local mode");
        } else {
            info!("Using local mode (corosync.conf does not exist)");
        }
        status.set_quorate(true);
        (None, None, None)
    };

    // Initialize cluster info in status
    status.init_cluster(config.cluster_name().to_string());

    // Initialize plugin registry
    let plugins = plugins::init_plugins(config.clone(), status.clone());

    // Note: Node registration from corosync is handled by ClusterConfigService during
    // its initialization, matching C's service_confdb behavior (confdb.c:276)

    // Daemonize if not in foreground mode (using builder pattern)
    let (daemon_guard, signal_handle) = if !args.foreground {
        let (process, handle) = Daemon::new()
            .pid_file(paths.pid_file.clone())
            .group(www_data_gid)
            .start_daemon_with_signal()?;

        match process {
            DaemonProcess::Parent => {
                // Parent exits here after child signals ready
                std::process::exit(0);
            }
            DaemonProcess::Child(guard) => (Some(guard), handle),
        }
    } else {
        (None, None)
    };

    // Mount FUSE filesystem
    let fuse_task = setup_fuse(
        &paths.mount_dir,
        memdb.clone(),
        config.clone(),
        dfsm.clone(),
        plugins,
        status.clone(),
    )?;

    // Start cluster services using ServiceManager (matching C's pmxcfs.c service initialization)
    // If this fails, abort the FUSE task to prevent orphaned mount
    let service_manager_handle = match setup_services(
        dfsm.as_ref(),
        status_dfsm.as_ref(),
        quorum_service,
        has_corosync_conf,
        args.local,
        status.clone(),
    ) {
        Ok(handle) => handle,
        Err(e) => {
            error!("Failed to setup services: {}", e);
            fuse_task.abort();
            return Err(e);
        }
    };

    // Scan VM list after database is loaded (matching C's memdb_open behavior)
    status.scan_vmlist(&memdb);

    // Setup signal handlers BEFORE starting IPC server to ensure signals are caught
    // during the startup sequence. This prevents a race where a signal arriving
    // between IPC start and signal handler setup would be missed.
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| anyhow::anyhow!("Failed to setup SIGTERM handler: {e}"))?;
    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| anyhow::anyhow!("Failed to setup SIGINT handler: {e}"))?;

    // Initialize and start IPC server (libqb-compatible IPC for C clients)
    // If this fails, abort FUSE task to prevent orphaned mount
    info!("Initializing IPC server (libqb-compatible)");
    let ipc_handler = IpcHandler::new(memdb.clone(), status.clone(), config.clone(), www_data_gid);
    let mut ipc_server = pmxcfs_ipc::Server::new("pve2", ipc_handler);
    if let Err(e) = ipc_server.start() {
        error!("Failed to start IPC server: {}", e);
        fuse_task.abort();
        return Err(e.into());
    }

    info!("pmxcfs started successfully");

    // Signal parent if daemonized, or write PID file in foreground mode
    let _pid_guard = if let Some(handle) = signal_handle {
        // Daemon mode: signal parent that we're ready (parent writes PID file and exits)
        handle.signal_ready()?;
        daemon_guard // Keep guard alive for cleanup on drop
    } else {
        // Foreground mode: write PID file now and retain guard for cleanup
        Some(
            Daemon::new()
                .pid_file(paths.pid_file.clone())
                .group(www_data_gid)
                .start_foreground()?,
        )
    };

    // Remove restart flag (matching C's timing - after all services started)
    let _ = fs::remove_file(&paths.restart_flag_file);

    // Wait for shutdown signal (using pre-registered handlers)
    tokio::select! {
        _ = sigterm.recv() => {
            info!("Received SIGTERM");
        }
        _ = sigint.recv() => {
            info!("Received SIGINT");
        }
    }

    info!("Shutting down pmxcfs");

    // Abort background tasks
    fuse_task.abort();

    // Create restart flag (signals restart, not permanent shutdown)
    let _restart_flag = RestartFlag::create(paths.restart_flag_file.clone(), www_data_gid);

    // Stop services
    ipc_server.stop();

    // Stop cluster services via ServiceManager
    if let Some(service_manager) = service_manager_handle {
        info!("Shutting down cluster services via ServiceManager");
        let _ = service_manager
            .shutdown(std::time::Duration::from_secs(5))
            .await;
    }

    // Unmount filesystem (matching C's fuse_unmount, using lazy unmount like umount -l)
    info!(
        "Unmounting FUSE filesystem from {}",
        paths.mount_dir.display()
    );
    let mount_path_cstr =
        std::ffi::CString::new(paths.mount_dir.to_string_lossy().as_ref()).unwrap();
    unsafe {
        libc::umount2(mount_path_cstr.as_ptr(), libc::MNT_DETACH);
    }

    info!("pmxcfs shutdown complete");

    Ok(())
}

fn init_logging(debug: bool) -> Result<()> {
    let filter_level = if debug { "debug" } else { "info" };
    let filter = EnvFilter::new(filter_level);

    // Create reloadable filter layer
    let (filter_layer, reload_handle) = reload::Layer::new(filter);

    // Create formatter layer for console output
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false);

    // Try to connect to journald (systemd journal / syslog integration)
    // Matches C implementation's openlog() call (status.c:1360)
    // Falls back to console-only logging if journald is unavailable
    let subscriber = tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer);

    match tracing_journald::layer() {
        Ok(journald_layer) => {
            // Successfully connected to journald
            subscriber.with(journald_layer).init();
            debug!("Logging to journald (syslog) enabled");
        }
        Err(e) => {
            // Journald not available (e.g., not running under systemd)
            // Continue with console logging only
            subscriber.init();
            debug!("Journald unavailable ({}), using console logging only", e);
        }
    }

    // Store reload handle for runtime adjustment (used by .debug plugin)
    pmxcfs_rs::logging::set_reload_handle(reload_handle)?;

    Ok(())
}

fn get_nodename() -> Result<String> {
    let mut utsname = libc::utsname {
        sysname: [0; 65],
        nodename: [0; 65],
        release: [0; 65],
        version: [0; 65],
        machine: [0; 65],
        domainname: [0; 65],
    };

    unsafe {
        if libc::uname(&mut utsname) != 0 {
            return Err(PmxcfsError::System("Unable to get node name".into()).into());
        }
    }

    let nodename_bytes = &utsname.nodename;
    let nodename_cstr = unsafe { std::ffi::CStr::from_ptr(nodename_bytes.as_ptr()) };
    let mut nodename = nodename_cstr.to_string_lossy().to_string();

    // Remove domain part if present (like C version)
    if let Some(dot_pos) = nodename.find('.') {
        nodename.truncate(dot_pos);
    }

    Ok(nodename)
}

fn resolve_node_ip(nodename: &str) -> Result<std::net::IpAddr> {
    use std::net::ToSocketAddrs;

    let addr_iter = (nodename, 0)
        .to_socket_addrs()
        .context("Failed to resolve node IP")?;

    for addr in addr_iter {
        let ip = addr.ip();
        // Skip loopback addresses
        if !ip.is_loopback() {
            return Ok(ip);
        }
    }

    Err(PmxcfsError::Configuration(format!(
        "Unable to resolve node name '{nodename}' to a non-loopback IP address"
    ))
    .into())
}

fn get_www_data_gid() -> Result<u32> {
    use users::get_group_by_name;

    let group = get_group_by_name("www-data")
        .ok_or_else(|| PmxcfsError::System("Unable to get www-data group".into()))?;

    Ok(group.gid())
}

fn create_directories(gid: u32, paths: &PathConfig, is_test_mode: bool) -> Result<()> {
    // Create varlib directory
    fs::create_dir_all(&paths.varlib_dir)
        .with_context(|| format!("Failed to create {}", paths.varlib_dir.display()))?;

    // Create run directory
    fs::create_dir_all(&paths.run_dir)
        .with_context(|| format!("Failed to create {}", paths.run_dir.display()))?;

    // Set ownership for run directory (skip in test mode - doesn't require root)
    if !is_test_mode {
        let run_dir_cstr =
            std::ffi::CString::new(paths.run_dir.to_string_lossy().as_ref()).unwrap();
        unsafe {
            if libc::chown(run_dir_cstr.as_ptr(), 0, gid as libc::gid_t) != 0 {
                return Err(PmxcfsError::System(format!(
                    "Failed to set ownership on {}",
                    paths.run_dir.display()
                ))
                .into());
            }
        }
    }

    Ok(())
}

fn import_corosync_conf(memdb: &MemDb, corosync_conf_path: &std::path::Path) -> Result<()> {
    if let Ok(content) = fs::read_to_string(corosync_conf_path) {
        info!("Importing corosync.conf from {}", corosync_conf_path.display());
        let mtime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as u32;

        memdb.create("/corosync.conf", 0, 0, mtime)?;
        memdb.write("/corosync.conf", 0, 0, mtime, content.as_bytes(), false)?;
    }

    Ok(())
}

/// Initialize cluster services (DFSM, QuorumService)
///
/// Returns (database_dfsm, status_dfsm, quorum_service) for cluster mode
fn setup_cluster_services(
    memdb: &MemDb,
    config: Arc<Config>,
    status: Arc<status::Status>,
    corosync_conf_path: &std::path::Path,
) -> Result<ClusterServices> {
    // Sync corosync configuration
    memdb.sync_corosync_conf(Some(corosync_conf_path.to_str().unwrap()), true)?;

    // Create main DFSM for database synchronization (pmxcfs_v1 CPG group)
    // Note: nodeid will be obtained via cpg_local_get() during init_cpg()
    info!("Creating main DFSM instance (pmxcfs_v1)");
    let database_callbacks = MemDbCallbacks::new(memdb.clone(), status.clone());
    let database_dfsm = Arc::new(Dfsm::new(
        config.cluster_name().to_string(),
        database_callbacks.clone(),
    )?);
    database_callbacks.set_dfsm(&database_dfsm);
    info!("Main DFSM created successfully");

    // Create status DFSM for ephemeral data synchronization (pve_kvstore_v1 CPG group)
    // Note: nodeid will be obtained via cpg_local_get() during init_cpg()
    // IMPORTANT: Use protocol version 0 to match C implementation's kvstore DFSM
    info!("Creating status DFSM instance (pve_kvstore_v1)");
    let status_callbacks: Arc<dyn Callbacks<Message = KvStoreMessage>> =
        Arc::new(StatusCallbacks::new(status.clone()));
    let status_dfsm = Arc::new(Dfsm::new_with_protocol_version(
        "pve_kvstore_v1".to_string(),
        status_callbacks,
        0, // Protocol version 0 to match C's kvstore
    )?);
    info!("Status DFSM created successfully");

    // Create QuorumService (owns quorum handle, matching C's service_quorum)
    info!("Creating QuorumService");
    let quorum_service = Arc::new(QuorumService::new(status));
    info!("QuorumService created successfully");

    Ok((database_dfsm, status_dfsm, quorum_service))
}

/// Setup and mount FUSE filesystem
///
/// Returns a task handle for the FUSE loop
fn setup_fuse(
    mount_path: &std::path::Path,
    memdb: MemDb,
    config: Arc<Config>,
    dfsm: Option<Arc<Dfsm<FuseMessage>>>,
    plugins: Arc<plugins::PluginRegistry>,
    status: Arc<status::Status>,
) -> Result<tokio::task::JoinHandle<()>> {
    // Unmount if already mounted (matching C's umount2(CFSDIR, MNT_FORCE))
    let mount_path_cstr = std::ffi::CString::new(mount_path.to_string_lossy().as_ref()).unwrap();
    unsafe {
        libc::umount2(mount_path_cstr.as_ptr(), libc::MNT_FORCE);
    }

    // Create mount directory
    fs::create_dir_all(mount_path)
        .with_context(|| format!("Failed to create mount point {}", mount_path.display()))?;

    // Spawn FUSE filesystem in background task
    let mount_path = mount_path.to_path_buf();
    let fuse_task = tokio::spawn(async move {
        if let Err(e) = fuse::mount_fuse(&mount_path, memdb, config, dfsm, plugins, status).await {
            tracing::error!("FUSE filesystem error: {}", e);
        }
    });

    Ok(fuse_task)
}

/// Setup cluster services (quorum, confdb, dcdb, status sync)
///
/// Returns a shutdown handle if services were started, None otherwise
fn setup_services(
    dfsm: Option<&Arc<Dfsm<FuseMessage>>>,
    status_dfsm: Option<&Arc<Dfsm<KvStoreMessage>>>,
    quorum_service: Option<Arc<pmxcfs_rs::quorum_service::QuorumService>>,
    has_corosync_conf: bool,
    force_local: bool,
    status: Arc<status::Status>,
) -> Result<Option<ServiceManagerHandle>> {
    if dfsm.is_none() && status_dfsm.is_none() && quorum_service.is_none() {
        return Ok(None);
    }

    let mut manager = ServiceManager::new();

    // Add ClusterDatabaseService (service_dcdb equivalent)
    if let Some(dfsm_instance) = dfsm {
        info!("Adding ClusterDatabaseService to ServiceManager");
        manager.add_service(Box::new(ClusterDatabaseService::new(Arc::clone(
            dfsm_instance,
        ))));
    }

    // Add StatusSyncService (service_status / kvstore equivalent)
    if let Some(status_dfsm_instance) = status_dfsm {
        info!("Adding StatusSyncService to ServiceManager");
        manager.add_service(Box::new(StatusSyncService::new(Arc::clone(
            status_dfsm_instance,
        ))));
    }

    // Add ClusterConfigService (service_confdb equivalent) - monitors Corosync configuration
    if has_corosync_conf && !force_local {
        info!("Adding ClusterConfigService to ServiceManager");
        manager.add_service(Box::new(ClusterConfigService::new(status)));
    }

    // Add QuorumService (service_quorum equivalent)
    if let Some(quorum_instance) = quorum_service {
        info!("Adding QuorumService to ServiceManager");
        // Extract QuorumService from Arc - ServiceManager will manage it
        match Arc::try_unwrap(quorum_instance) {
            Ok(service) => {
                manager.add_service(Box::new(service));
            }
            Err(_) => {
                anyhow::bail!("Cannot unwrap QuorumService Arc - multiple references exist");
            }
        }
    }

    // Get shutdown token before spawning (for graceful shutdown)
    let shutdown_token = manager.shutdown_token();

    // Spawn ServiceManager in background task
    let handle = manager.spawn();

    Ok(Some(ServiceManagerHandle {
        shutdown_token,
        task: handle,
    }))
}

/// Handle for managing ServiceManager lifecycle
struct ServiceManagerHandle {
    shutdown_token: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl ServiceManagerHandle {
    /// Gracefully shutdown the ServiceManager with timeout
    ///
    /// Signals shutdown via cancellation token, then awaits task completion
    /// with a timeout. Matches C's cfs_loop_stop_worker() behavior.
    async fn shutdown(self, timeout: std::time::Duration) -> Result<()> {
        // Signal graceful shutdown (matches C's stop_worker_flag)
        self.shutdown_token.cancel();

        // Await completion with timeout
        match tokio::time::timeout(timeout, self.task).await {
            Ok(Ok(())) => {
                info!("ServiceManager shut down cleanly");
                Ok(())
            }
            Ok(Err(e)) => {
                tracing::warn!("ServiceManager task panicked: {}", e);
                Err(anyhow::anyhow!("ServiceManager task panicked: {}", e))
            }
            Err(_) => {
                tracing::warn!("ServiceManager shutdown timed out after {:?}", timeout);
                Err(anyhow::anyhow!("ServiceManager shutdown timed out"))
            }
        }
    }
}
