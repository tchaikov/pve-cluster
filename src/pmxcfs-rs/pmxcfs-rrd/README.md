# pmxcfs-rrd

RRD (Round-Robin Database) persistence for pmxcfs performance metrics.

## Overview

This crate provides RRD file management for storing time-series performance data from Proxmox nodes and VMs. It handles file creation, updates, and integration with rrdcached daemon for efficient writes.

### Key Features

- RRD file creation with schema-based initialization
- RRD updates (write metrics to disk)
- rrdcached integration for batched writes
- Support for both legacy and current schema versions (v1/v2/v3)
- Type-safe key parsing and validation
- Compatible with existing C-created RRD files

## Usage Flow

The typical data flow through this crate:

1. **Metrics Collection**: pmxcfs-status collects performance metrics (CPU, memory, network, etc.)
2. **Key Generation**: Metrics are organized by key type (node, VM, storage)
3. **Schema Selection**: Appropriate RRD schema is selected based on key type and version
4. **Data Transformation**: Legacy data (v1/v2) is transformed to current format (v3) if needed
5. **Backend Selection**:
   - **Daemon backend**: Preferred for performance, batches writes via rrdcached
   - **Direct backend**: Fallback using librrd directly when daemon unavailable
   - **Fallback backend**: Tries daemon first, falls back to direct on failure
6. **File Operations**: Create RRD files if needed, update with new data points

### Data Transformation

The crate handles migration between schema versions:
- **v1 → v2**: Adds additional data sources for extended metrics
- **v2 → v3**: Consolidates and optimizes data sources
- **Transform logic**: `schema.rs:transform_data()` handles conversion, skipping incompatible entries

### Backend Differences

- **Daemon Backend** (`backend_daemon.rs`):
  - Uses vendored rrdcached client for async communication
  - Batches multiple updates for efficiency
  - Requires rrdcached daemon running
  - Best for high-frequency updates

- **Direct Backend** (`backend_direct.rs`):
  - Uses rrd crate (librrd FFI bindings) directly
  - Synchronous file operations
  - No external daemon required
  - Reliable fallback option

- **Fallback Backend** (`backend_fallback.rs`):
  - Composite pattern: tries daemon, falls back to direct
  - Matches C implementation behavior
  - Provides best of both worlds

## Module Structure

| Module | Purpose |
|--------|---------|
| `writer.rs` | Main RrdWriter API - high-level interface for RRD operations |
| `schema.rs` | RRD schema definitions (DS, RRA) and data transformation logic |
| `key_type.rs` | RRD key parsing, validation, and path sanitization |
| `daemon.rs` | rrdcached daemon client wrapper |
| `backend.rs` | Backend trait and implementations (daemon/direct/fallback) |
| `rrdcached/` | Vendored rrdcached client implementation (adapted from rrdcached-client v0.1.5) |

## Usage Example

```rust
use pmxcfs_rrd::{RrdWriter, RrdFallbackBackend};

// Create writer with fallback backend
let backend = RrdFallbackBackend::new("/var/run/rrdcached.sock").await?;
let writer = RrdWriter::new(backend);

// Update node CPU metrics
writer.update(
    "pve/nodes/node1/cpu",
    &[0.45, 0.52, 0.38, 0.61], // CPU usage values
    None, // Use current timestamp
).await?;

// Create new RRD file for VM
writer.create(
    "pve/qemu/100/cpu",
    1704067200, // Start timestamp
).await?;
```

## External Dependencies

- **rrd crate**: Provides Rust bindings to librrd (RRDtool C library)
- **rrdcached client**: Vendored and adapted from rrdcached-client v0.1.5 (Apache-2.0 license)
  - Original source: https://github.com/SINTEF/rrdcached-client
  - Vendored to gain full control and adapt to our specific needs
  - Can be disabled via the `rrdcached` feature flag

## Testing

Unit tests verify:
- Schema generation and validation
- Key parsing for different RRD types (node, VM, storage)
- RRD file creation and update operations
- rrdcached client connection and fallback behavior

Run tests with:
```bash
cargo test -p pmxcfs-rrd
```

## References

- **C Implementation**: `src/pmxcfs/status.c` (RRD code embedded)
- **Related Crates**:
  - `pmxcfs-status` - Uses RrdWriter for metrics persistence
  - `pmxcfs` - FUSE `.rrd` plugin reads RRD files
- **RRDtool Documentation**: https://oss.oetiker.ch/rrdtool/
