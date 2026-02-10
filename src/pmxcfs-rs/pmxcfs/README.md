# pmxcfs Rust Implementation

This directory contains the Rust reimplementation of pmxcfs (Proxmox Cluster File System).

## Architecture Overview

pmxcfs is a FUSE-based cluster filesystem that provides:
- **Cluster-wide configuration storage** via replicated database (pmxcfs-memdb)
- **State synchronization** across nodes via Corosync CPG (pmxcfs-dfsm)
- **Virtual files** for runtime status (plugins: .version, .members, .vmlist, .rrd)
- **Quorum enforcement** for write protection
- **IPC server** for management tools (pvecm, pvenode)

### Component Architecture

### FUSE Plugin System

Virtual files that appear in `/etc/pve` but don't exist in the database:

| Plugin | File | Purpose | C Equivalent |
|--------|------|---------|--------------|
| `version.rs` | `.version` | Cluster version info | `cfs-plug-func.c` (cfs_plug_version_read) |
| `members.rs` | `.members` | Cluster member list | `cfs-plug-func.c` (cfs_plug_members_read) |
| `vmlist.rs` | `.vmlist` | VM/CT registry | `cfs-plug-func.c` (cfs_plug_vmlist_read) |
| `rrd.rs` | `.rrd` | RRD dump (all metrics) | `cfs-plug-func.c` (cfs_plug_rrd_read) |
| `clusterlog.rs` | `.clusterlog` | Cluster log viewer | `cfs-plug-func.c` (cfs_plug_clusterlog_read) |
| `debug.rs` | `.debug` | Runtime debug control | `cfs-plug-func.c` (cfs_plug_debug) |

#### Plugin Trait

Plugins are registered in `plugins/registry.rs` and integrated into the FUSE filesystem.

### C File Mapping

| C Source | Rust Equivalent | Description |
|----------|-----------------|-------------|
| `pmxcfs.c` | `main.rs`, `daemon.rs` | Main entry point, daemon lifecycle |
| `cfs-plug.c` | `fuse/filesystem.rs` | FUSE operations dispatcher |
| `cfs-plug-memdb.c` | `fuse/filesystem.rs` | MemDb integration |
| `cfs-plug-func.c` | `plugins/*.rs` | Virtual file plugins |
| `server.c` | `ipc_service.rs` + pmxcfs-ipc | IPC server |
| `loop.c` | pmxcfs-services | Service management |

## Key Differences from C Implementation

### Command-line Options

Both implementations support the core options with identical behavior:
- `-d` / `--debug` - Turn on debug messages
- `-f` / `--foreground` - Do not daemonize server
- `-l` / `--local` - Force local mode (ignore corosync.conf, force quorum)

The Rust implementation adds these additional options for flexibility and testing:
- `--test-dir <PATH>` - Test directory (sets all paths to subdirectories for isolated testing)
- `--mount <PATH>` - Custom mount point (default: /etc/pve)
- `--db <PATH>` - Custom database path (default: /var/lib/pve-cluster/config.db)
- `--rundir <PATH>` - Custom runtime directory (default: /run/pmxcfs)
- `--cluster-name <NAME>` - Cluster name / CPG group name for Corosync isolation (default: "pmxcfs")

The Rust version is fully backward-compatible with C version command-line usage. The additional options are for advanced use cases (testing, multi-instance deployments) and don't affect standard deployment scenarios.

### Logging

**C Implementation**: Uses libqb's qb_log with traditional syslog format

**Rust Implementation**: Uses tracing + tracing-subscriber with structured output integrated with systemd journald

Log messages may appear in different format, but journald integration provides same searchability as syslog. Log levels work equivalently (debug, info, warn, error).

## Plugin System Details

### Virtual File Plugins

Each plugin provides a read-only (or read-write) virtual file accessible through the FUSE mount:

#### `.version` - Version Information

**Path:** `/etc/pve/.version`
**Format:** `{start_time}:{vmlist_version}:{path_versions...}`
**Purpose:** Allows tools to detect configuration changes
**Implementation:** `plugins/version.rs`

Example output:
Each number is a version counter that increments on changes.

#### `.members` - Cluster Members

**Path:** `/etc/pve/.members`
**Format:** INI-style with member info
**Purpose:** Lists active cluster nodes
**Implementation:** `plugins/members.rs`

Example output:
Format: `{nodeid}\t{name}\t{online}\t{ip}`

#### `.vmlist` - VM/CT Registry

**Path:** `/etc/pve/.vmlist`
**Format:** INI-style with VM info
**Purpose:** Cluster-wide VM/CT registry
**Implementation:** `plugins/vmlist.rs`

Example output:
Format: `{vmid}\t{node}\t{version}`

#### `.rrd` - RRD Metrics Dump

**Path:** `/etc/pve/.rrd`
**Format:** Custom RRD dump format
**Purpose:** Exports all RRD metrics for graph generation
**Implementation:** `plugins/rrd.rs`

Example output:

#### `.clusterlog` - Cluster Log

**Path:** `/etc/pve/.clusterlog`
**Format:** Plain text log entries
**Purpose:** Aggregated cluster-wide log
**Implementation:** `plugins/clusterlog.rs`

Example output:

#### `.debug` - Debug Control

**Path:** `/etc/pve/.debug`
**Format:** Text commands
**Purpose:** Runtime debug level control
**Implementation:** `plugins/debug.rs`

Write "1" to enable debug logging, "0" to disable.

### Plugin Registration

Plugins are registered in `plugins/registry.rs`:

### FUSE Integration

The FUSE filesystem checks plugins before MemDb:

## Crate Structure

The Rust implementation is organized as a workspace with 9 crates:

| Crate | Purpose | Lines | C Equivalent |
|-------|---------|-------|--------------|
| **pmxcfs** | Main daemon binary | ~3500 | pmxcfs.c + plugins |
| **pmxcfs-api-types** | Shared types | ~400 | cfs-utils.h |
| **pmxcfs-config** | Configuration | ~75 | (inline in C) |
| **pmxcfs-memdb** | In-memory database | ~2500 | memdb.c + database.c |
| **pmxcfs-dfsm** | State machine | ~3000 | dfsm.c + dcdb.c |
| **pmxcfs-rrd** | RRD persistence | ~800 | status.c (embedded) |
| **pmxcfs-status** | Status tracking | ~900 | status.c |
| **pmxcfs-ipc** | IPC server | ~2000 | server.c |
| **pmxcfs-services** | Service framework | ~500 | loop.c |

Total: **~14,000 lines** vs C implementation **~15,000 lines**

## Migration Notes

The Rust implementation can coexist with C nodes in the same cluster:
- **Wire protocol**: 100% compatible (DFSM, IPC, RRD)
- **Database format**: SQLite schema identical
- **Corosync integration**: Uses same CPG groups
- **File format**: All config files compatible

## References

### Documentation
- [Implementation Plan](../../pmxcfs-rust-rewrite-plan.rst)
- Individual crate README.md files for detailed docs

### C Implementation
- `src/pmxcfs/` - Original C implementation
