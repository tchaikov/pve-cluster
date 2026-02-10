# pmxcfs-status

**Cluster Status** tracking and monitoring for pmxcfs.

This crate manages all runtime cluster state information including membership, VM lists, node status, RRD metrics, and cluster logs. It serves as the central repository for dynamic cluster information that changes during runtime.

## Overview

The Status subsystem tracks:
- **Cluster membership**: Which nodes are in the cluster and their states
- **VM/CT tracking**: Registry of all virtual machines and containers
- **Node status**: Per-node health and resource information
- **RRD data**: Performance metrics (CPU, memory, disk, network)
- **Cluster log**: Centralized log aggregation
- **Quorum state**: Whether cluster has quorum
- **Version tracking**: Monitors configuration file changes

## Usage

### Initialization

```rust
use pmxcfs_status;

// For tests or when RRD persistence is not needed
let status = pmxcfs_status::init();

// For production with RRD file persistence
let status = pmxcfs_status::init_with_rrd("/var/lib/rrdcached/db").await;
```

The default `init()` is synchronous and doesn't require a directory parameter, making tests simpler. Use `init_with_rrd()` for production deployments that need RRD persistence.

### Integration with Other Components

**FUSE Plugins**:
- `.version` plugin reads from Status
- `.vmlist` plugin generates VM list from Status
- `.members` plugin generates member list from Status
- `.rrd` plugin accesses RRD data from Status
- `.clusterlog` plugin reads cluster log from Status

**DFSM Status Sync**:
- `StatusSyncService` (pmxcfs-dfsm) broadcasts status updates
- Uses `pve_kvstore_v1` CPG group
- KV store data synchronized across nodes

**IPC Server**:
- `set_status` IPC call updates Status
- Used by `pvecm`/`pvenode` tools
- RRD data received via IPC

**MemDb Integration**:
- Scans VM configs to populate vmlist
- Tracks version changes on file modifications
- Used for `.version` plugin timestamps

## Architecture

### Module Structure

| Module | Purpose |
|--------|---------|
| `lib.rs` | Public API and initialization |
| `status.rs` | Core Status struct and operations |
| `types.rs` | Type definitions (ClusterNode, ClusterInfo, etc.) |

### Key Features

**Thread-Safe**: All operations use `RwLock` or `AtomicU64` for concurrent access
**Version Tracking**: Monotonically increasing counters for change detection
**Structured Logging**: Field-based tracing for better observability
**Optional RRD**: RRD persistence is opt-in, simplifying testing

## C to Rust Mapping

### Data Structures

| C Type | Rust Type | Notes |
|--------|-----------|-------|
| `cfs_status_t` | `Status` | Main status container |
| `cfs_clinfo_t` | `ClusterInfo` | Cluster membership info |
| `cfs_clnode_t` | `ClusterNode` | Individual node info |
| `vminfo_t` | `VmEntry` | VM/CT registry entry (in pmxcfs-api-types) |
| `clog_entry_t` | `ClusterLogEntry` | Cluster log entry |

### Core Functions

| C Function | Rust Equivalent | Notes |
|-----------|-----------------|-------|
| `cfs_status_init()` | `init()` or `init_with_rrd()` | Two variants for flexibility |
| `cfs_set_quorate()` | `Status::set_quorate()` | Quorum tracking |
| `cfs_is_quorate()` | `Status::is_quorate()` | Quorum checking |
| `vmlist_register_vm()` | `Status::register_vm()` | VM registration |
| `vmlist_delete_vm()` | `Status::delete_vm()` | VM deletion |
| `cfs_status_set()` | `Status::set_node_status()` | Status updates (including RRD) |

## Key Differences from C Implementation

### RRD Decoupling

**C Version (status.c)**:
- RRD code embedded in status.c
- Async initialization always required

**Rust Version**:
- Separate `pmxcfs-rrd` crate
- `init()` is synchronous (no RRD)
- `init_with_rrd()` is async (with RRD)
- Tests don't need temp directories

### Concurrency

**C Version**:
- Single `GMutex` for entire status structure

**Rust Version**:
- Fine-grained `RwLock` for different data structures
- `AtomicU64` for version counters
- Better read parallelism

## Configuration File Tracking

Status tracks version numbers for these common Proxmox config files:

- `corosync.conf`, `corosync.conf.new`
- `storage.cfg`, `user.cfg`, `domains.cfg`
- `datacenter.cfg`, `vzdump.cron`, `vzdump.conf`
- `ha/` directory files (crm_commands, manager_status, resources.cfg, etc.)
- `sdn/` directory files (vnets.cfg, zones.cfg, controllers.cfg, etc.)
- And many more (see `Status::new()` in status.rs for complete list)

## References

### C Implementation
- `src/pmxcfs/status.c` / `status.h` - Status tracking

### Related Crates
- **pmxcfs-rrd**: RRD file persistence
- **pmxcfs-dfsm**: Status synchronization via StatusSyncService
- **pmxcfs-logger**: Cluster log implementation
- **pmxcfs**: FUSE plugins that read from Status
