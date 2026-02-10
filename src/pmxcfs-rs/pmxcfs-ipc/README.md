# pmxcfs-ipc: libqb-Compatible IPC Server

**Rust implementation of libqb IPC server for pmxcfs using shared memory ring buffers**

This crate provides a wire-compatible IPC server that works with libqb clients (C `qb_ipcc_*` API) without depending on the libqb C library.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Protocol Implementation](#protocol-implementation)
- [Usage](#usage)
- [Testing](#testing)
- [References](#references)

---

## Overview

pmxcfs uses libqb for IPC communication between the daemon and client tools (`pvecm`, `pvenode`, etc.). This crate implements a server using QB_IPC_SHM (shared memory ring buffers) that is wire-compatible with libqb clients, enabling the Rust pmxcfs implementation to communicate with existing C-based tools.

**Key Features**:
- Wire-compatible with libqb clients
- QB_IPC_SHM transport (shared memory ring buffers)
- Async I/O via tokio
- Lock-free SPSC ring buffers
- Supports authentication via uid/gid
- Per-connection context (uid, gid, pid, read-only flag)
- Connection statistics tracking
- Abstract Unix sockets for setup handshake (Linux-specific)

---

## Architecture

### Transport: QB_IPC_SHM (Shared Memory Ring Buffers)

**Rust pmxcfs uses**: `QB_IPC_SHM` (shared memory ring buffers)

We implemented shared memory transport using lock-free SPSC (single-producer single-consumer) ring buffers. This provides:

- **Wire compatibility**: Same handshake protocol as libqb
- **Async I/O**: Integration with tokio ecosystem

**Ring Buffer Design**:
- Each connection has 3 ring buffers:
  1. **Request ring**: Client writes, server reads
  2. **Response ring**: Server writes, client reads
  3. **Event ring**: Server writes, client reads (for async notifications)
- Ring buffers stored in `/dev/shm` (Linux shared memory)
- Chunk-based protocol matching libqb

### Server Structure

### Connection Statistics

Tracks statistics for C compatibility (matching `qb_ipcs_stats`).

---

## Protocol Implementation

### Connection Handshake

Server creates an abstract Unix socket `@pve2` (@ prefix indicates abstract namespace) for initial connection setup.

### Request/Response Communication

After handshake, communication happens via shared memory ring buffers using libqb-compatible chunk format.

### Wire Format Structures

All structures use `#[repr(C, align(8))]` to match C's alignment requirements.

Error codes must be negative errno values (e.g., `-EPERM`, `-EINVAL`) to match libqb convention.

---

## Testing

Requires Corosync running for integration tests. See `tests/` directory for C client FFI compatibility tests.

## Implementation Status

### Implemented

- Connection handshake (SOCK_STREAM setup socket)
- Authentication via SO_PASSCRED (uid/gid/pid)
- QB_IPC_SHM transport (shared memory ring buffers)
- Lock-free SPSC ring buffers
- Async I/O via tokio
- Abstract Unix sockets for setup handshake
- Message header parsing (request/response)
- Error code propagation (negative errno)
- Ring buffer file management (creation/cleanup)
- Event channel ring buffers (created, not actively used)
- Connection statistics tracking
- Disconnect detection
- Read-only flag based on gid

### Not Implemented

- Event channel message sending (pmxcfs doesn't use events yet)

## Application-Level IPC Operations

### Operation Summary

The following IPC operations are supported (defined in pmxcfs):

| Operation | Request Data | Response Data | Description |
|-----------|-------------|---------------|-------------|
| GET_FS_VERSION | Empty | uint32_t version | Get filesystem version number |
| GET_CLUSTER_INFO | Empty | JSON string | Get cluster information |
| GET_GUEST_LIST | Empty | JSON array | Get list of all VMs/containers |
| SET_STATUS | name + data | Empty | Set status key-value pair |
| GET_STATUS | name | Binary data | Get status value by name |
| GET_CONFIG | name | File contents | Read configuration file |
| LOG_CLUSTER_MSG | priority + msg | Empty | Add cluster log entry |
| GET_CLUSTER_LOG | max_entries | JSON array | Get cluster log entries |
| GET_RRD_DUMP | Empty | RRD dump text | Get all RRD data |
| GET_GUEST_CONFIG_PROPERTY | vmid + key | String value | Get single VM config property |
| GET_GUEST_CONFIG_PROPERTIES | vmid | JSON object | Get all VM config properties |
| VERIFY_TOKEN | userid + token | Boolean | Verify API token validity |

### Common Clients

The following Proxmox components use the IPC interface:

- **pvestatd**: Updates node/VM/storage metrics (SET_STATUS, GET_STATUS)
- **pve-ha-crm**: HA cluster resource manager (GET_CLUSTER_INFO, GET_GUEST_LIST)
- **pve-ha-lrm**: HA local resource manager (GET_CONFIG, LOG_CLUSTER_MSG)
- **pvecm**: Cluster management CLI (GET_CLUSTER_INFO, GET_CLUSTER_LOG)
- **pvedaemon**: PVE API daemon (All query operations)

### Permission Model

**Write Operations** (require root):
- SET_STATUS
- LOG_CLUSTER_MSG

**Read Operations** (any authenticated user):
- All GET_* operations
- VERIFY_TOKEN

---

## References

### libqb Source

Reference implementation of QB IPC protocol (available at https://github.com/ClusterLabs/libqb):

- `libqb/lib/ringbuffer.c` - Ring buffer implementation
- `libqb/lib/ipc_shm.c` - Shared memory transport
- `libqb/lib/ipc_setup.c` - Connection setup/handshake
- `libqb/include/qb/qbipc_common.h` - Wire protocol structures

### C pmxcfs (pve-cluster)

- `src/pmxcfs/server.c` - C IPC server using libqb
- `src/pmxcfs/cfs-ipc-ops.h` - pmxcfs IPC operation codes

### Related Documentation

- `../C_COMPATIBILITY.md` - General C compatibility notes (if exists)

---

## Notes

### Ring Buffer Naming Convention

Ring buffer files are created in `/dev/shm` with names based on connection descriptor and ring type (request/response/event).

### Error Handling

Always use **negative errno values** for errors to maintain compatibility with libqb clients.

### Alignment and Padding

All wire format structures must use `#[repr(C, align(8))]` to ensure 8-byte alignment matching C's requirements.
