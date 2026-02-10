# pmxcfs-api-types

**Shared Types and Error Definitions** for pmxcfs.

This crate provides common types and error definitions used across all pmxcfs crates.

## Overview

The crate contains:
- **Error types**: `PmxcfsError` with errno mapping for FUSE
- **Shared types**: `MemberInfo`, `NodeSyncInfo`, `VmType`, `VmEntry`

## Error Types

### PmxcfsError

Type-safe error enum with automatic errno conversion.

### errno Mapping

Errors automatically convert to POSIX errno values for FUSE.

| Error | errno | Value | Note |
|-------|-------|-------|------|
| `NotFound(_)` | `ENOENT` | 2 | File or directory not found |
| `PermissionDenied` | `EACCES` | 13 | File permission denied |
| `AlreadyExists(_)` | `EEXIST` | 17 | File already exists |
| `NotADirectory(_)` | `ENOTDIR` | 20 | Not a directory |
| `IsADirectory(_)` | `EISDIR` | 21 | Is a directory |
| `DirectoryNotEmpty(_)` | `ENOTEMPTY` | 39 | Directory not empty |
| `InvalidArgument(_)` | `EINVAL` | 22 | Invalid argument |
| `InvalidPath(_)` | `EINVAL` | 22 | Invalid path |
| `FileTooLarge` | `EFBIG` | 27 | File too large |
| `ReadOnlyFilesystem` | `EROFS` | 30 | Read-only filesystem |
| `NoQuorum` | `EACCES` | 13 | No cluster quorum |
| `Lock(_)` | `EAGAIN` | 11 | Lock unavailable, try again |
| `Timeout` | `ETIMEDOUT` | 110 | Operation timed out |
| `Io(e)` | varies | varies | OS error code or `EIO` |
| Others* | `EIO` | 5 | Internal error |

*Others include: `Database`, `Fuse`, `Cluster`, `Corosync`, `Configuration`, `System`, `Ipc`

## Shared Types

### MemberInfo

Cluster member information.

### NodeSyncInfo

DFSM synchronization state.

### VmType

VM/CT type enum (Qemu or Lxc).

### VmEntry

VM/CT entry for vmlist.

## C to Rust Mapping

### Error Handling

**C Version (cfs-utils.h):**
- Return codes: `0` = success, negative = error
- errno-based error reporting
- Manual error checking everywhere

**Rust Version:**
- `Result<T, PmxcfsError>` type

## Known Issues / TODOs

### Missing Features
- None identified

### Compatibility
- **errno values**: Match POSIX standards

## References

### C Implementation
- `src/pmxcfs/cfs-utils.h` - Utility types and error codes

### Related Crates
- **pmxcfs-dfsm**: Uses shared types for cluster sync
- **pmxcfs-memdb**: Uses PmxcfsError for database operations
