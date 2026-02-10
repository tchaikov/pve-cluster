# pmxcfs-rs

## Executive Summary

pmxcfs-rs is a complete rewrite of the Proxmox Cluster File System from C to Rust, achieving full functional parity while maintaining wire-format compatibility with the C implementation. The implementation has passed comprehensive single-node and multi-node integration testing.

**Overall Completion**: All subsystems implemented
- All core subsystems implemented and tested
- Wire protocol compatibility verified
- Comprehensive test coverage (24 integration tests + extensive unit tests)
- Production client compatibility confirmed
- Multi-node cluster functionality validated

---

## Component Status

### Workspace Structure

pmxcfs-rs is organized as a Rust workspace with 9 crates:

| Crate | Purpose |
|-------|---------|
| `pmxcfs` | Main daemon binary |
| `pmxcfs-config` | Configuration management |
| `pmxcfs-api-types` | Shared types and errors |
| `pmxcfs-memdb` | Database with SQLite backend |
| `pmxcfs-dfsm` | Distributed state machine + CPG |
| `pmxcfs-rrd` | RRD file persistence |
| `pmxcfs-status` | Status monitoring + RRD |
| `pmxcfs-ipc` | libqb-compatible IPC server |
| `pmxcfs-services` | Service lifecycle framework |
| `pmxcfs-logger` | Cluster log + ring buffer |

### Compatibility Matrix

| Component | Notes |
|-----------|-------|
| **FUSE Filesystem** | All operations implemented |
| **Database (MemDB)** | SQLite schema compatible |
| **Cluster Communication** | CPG/Quorum via Corosync |
| **DFSM State Machine** | Binary message format compatible |
| **IPC Server** | Wire protocol verified with libqb clients |
| **Plugin System** | All 10 plugins (6 func + 4 link) with write support |
| **RRD Integration** | Format migration implemented |
| **Status Subsystem** | VM list, config tracking, cluster log |

---

## Design Decisions and Notable Differences

### 1. IPC Protocol: Partial libqb Implementation

**Decision**: Implement libqb-compatible wire protocol without using libqb library directly.

**C Implementation**:
- Uses libqb library directly (`libqb0`, `libqb-dev`)
- Full libqb feature set (SHM ring buffers, POSIX message queues, etc.)
- IPC types: `QB_IPC_SOCKET`, `QB_IPC_SHM`, `QB_IPC_POSIX_MQ`

**Rust Implementation**:
- Custom implementation of libqb wire protocol
- Only implements `QB_IPC_SOCKET` type (Unix datagram sockets + shared memory control files)
- Compatible handshake, request/response structures
- Verified with both libqb C clients and production Perl clients (PVE::IPCC)

**Rationale**:
- libqb has no Rust bindings and FFI would be complex
- pmxcfs only uses `QB_IPC_SOCKET` type in production
- Wire protocol compatibility is what matters for clients
- Simpler implementation, easier to maintain

**Compatibility Impact**: **None** - All production clients work identically

**Reference**:
- C: `src/pmxcfs/server.c` (uses libqb API)
- Rust: `src/pmxcfs-rs/pmxcfs-ipc/src/server.rs` (custom implementation)
- Verification: `pmxcfs-ipc/tests/qb_wire_compat.rs` (all tests passing)

---

### 2. Logging System: tracing vs qb_log

**Decision**: Use Rust `tracing` ecosystem instead of libqb's `qb_log`.

**C Implementation**:
- Uses `qb_log` from libqb for all logging
- Log levels: `QB_LOG_EMERG`, `QB_LOG_ALERT`, `QB_LOG_CRIT`, `QB_LOG_ERR`, `QB_LOG_WARNING`, `QB_LOG_NOTICE`, `QB_LOG_INFO`, `QB_LOG_DEBUG`
- Output: syslog + stderr
- Runtime control: Write to `/etc/pve/.debug` file (0 = info, 1 = debug)
- Format: `[domain] LEVEL: message (file.c:line:function)`

**Rust Implementation**:
- Uses `tracing` crate with `tracing-subscriber`
- Log levels: `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE`
- Output: journald (via `tracing-journald`) + stdout
- Runtime control: Same mechanism - `.debug` plugin file (0 = info, 1 = debug)
- Format: `[timestamp] LEVEL module::path: message`

**Key Differences**:

| Aspect | C (qb_log) | Rust (tracing) | Impact |
|--------|-----------|----------------|--------|
| **Log format** | `[domain] INFO: msg (file.c:123)` | `2025-11-14T10:30:45 INFO pmxcfs::module: msg` | Log parsers need update |
| **Severity levels** | 8 levels (syslog standard) | 5 levels (standard Rust) | Mapping works fine |
| **Destination** | syslog | journald (systemd) | Both queryable, journald is modern |
| **Runtime toggle** | `/etc/pve/.debug` | Same | **No change** |
| **CLI flag** | `-d` or `--debug` | Same | **No change** |

**Rationale**:
- `tracing` is the Rust ecosystem standard
- Better async/structured logging support
- No FFI to libqb needed
- Integrates with systemd/journald natively
- Same user-facing behavior (`.debug` file toggle)

**Compatibility Impact**: **Minor** - Log monitoring scripts may need format updates

**Migration**:
```bash
# Old C logs (syslog)
journalctl -u pve-cluster | grep pmxcfs

# New Rust logs (journald, same command works)
journalctl -u pve-cluster | grep pmxcfs
```

**Reference**:
- C: `src/pmxcfs/pmxcfs.c` (qb_log initialization)
- Rust: `src/pmxcfs-rs/pmxcfs/src/main.rs` (tracing-subscriber setup)

---

### 3. OpenVZ Container Support: Intentionally Excluded

**Decision**: No functional support for OpenVZ containers.

**C Implementation**:
- Includes OpenVZ VM type (`VMTYPE_OPENVZ = 2`)
- Detects OpenVZ action scripts (`vps*.mount`, `*.start`, `*.stop`, etc.)
- Sets executable permissions on OpenVZ scripts
- Scans `nodes/*/openvz/` directories for containers
- **All code marked**: `// FIXME: remove openvz stuff for 7.x`

**Rust Implementation**:
- VM types: `VmType::Qemu = 1`, `VmType::Lxc = 3` (no `VMTYPE_OPENVZ = 2`)
- `/openvz` symlink exists (for backward compatibility) but no functional support
- No OpenVZ script detection or VM scanning

**Rationale**:
- OpenVZ deprecated in Proxmox VE 4.0 (2015)
- OpenVZ removed completely in Proxmox VE 7.0 (2021)
- pmxcfs-rs ships with Proxmox VE 9.x (2 major versions after removal)
- Last OpenVZ code change: October 2011 (14 years ago)
- Mandatory LXC migration completed years ago

**Compatibility Impact**: **None** - No PVE 9.x systems have OpenVZ containers

**Reference**:
- C: `src/pmxcfs/status.h:31-32`, `cfs-plug-memdb.c:46-93`, `memdb.c:455-460`
- Rust: `pmxcfs-api-types/src/lib.rs:99-102` (VmType enum)

---

## Testing

pmxcfs-rs has a comprehensive test suite with 100+ tests organized following modern Rust testing best practices.

### Quick Start

```bash
# Run all tests
cargo test --workspace

# Run unit tests only (fast, inline tests)
cargo test --lib

# Run integration tests only
cargo test --test '*'

# Run specific package tests
cargo test -p pmxcfs-memdb
```

### Test Architecture

The test suite is organized into three categories:

1. **Unit Tests** (65+ tests, inline `#[cfg(test)]` modules)
   - Fast (<10ms per test)
   - Use mocks (MockMemDb, MockStatus) for isolation
   - Located next to the code they test
   - Examples: `pmxcfs-memdb/src/database.rs`, `pmxcfs-config/src/lib.rs`

2. **Integration Tests** (35+ tests, `tests/` directories)
   - Test component interactions
   - Use real implementations or TestEnv builder
   - Complete in <1s using condition polling (no sleep)
   - Examples: `pmxcfs-ipc/tests/auth_test.rs`, `pmxcfs-services/tests/service_tests.rs`

3. **Multi-Node Cluster Tests** (24 tests, `integration-tests/`)
   - Full system integration with Corosync
   - Single and multi-node scenarios
   - C/Rust interoperability verification

### Test Utilities

Centralized test helpers in `pmxcfs-test-utils`:

```rust
use pmxcfs_test_utils::{TestEnv, MockMemDb, wait_for_condition};

// Fast unit test with mocks
#[test]
fn test_with_mock() {
    let db = MockMemDb::new();  // 100x faster than real DB
    db.create("/file", libc::S_IFREG, 1000).unwrap();
}

// Integration test with TestEnv builder
#[test]
fn test_integration() {
    let env = TestEnv::new()
        .with_database().unwrap()
        .with_mock_status()
        .build();
}

// Async test with condition polling (no sleep!)
#[tokio::test]
async fn test_async() {
    let ready = wait_for_condition(
        || service.is_ready(),
        Duration::from_secs(5),
        Duration::from_millis(10),
    ).await;
    assert!(ready);
}
```

### Performance

- **Unit tests**: Complete in ~2 seconds (all 65 tests)
- **Integration tests**: Complete in ~5 seconds (condition polling, no arbitrary sleeps)
- **MockMemDb**: 150-500x faster than SQLite-backed tests
- **Parallel execution**: Tests are isolated and run concurrently

### Documentation

- **[TEST_ARCHITECTURE.md](TEST_ARCHITECTURE.md)** - Comprehensive testing guide
- **[MIGRATION_GUIDE.md](MIGRATION_GUIDE.md)** - How to write and migrate tests
- **[TEST_REFACTORING_PROGRESS.md](TEST_REFACTORING_PROGRESS.md)** - Refactoring history

### Multi-Node Integration Tests

Complete integration test suite covering single-node, multi-node cluster, and C/Rust interoperability.

```bash
cd integration-tests
./test --build          # Build and run all tests
./test --no-build       # Quick iteration
./test --list           # Show available tests
```

See [integration-tests/README.md](integration-tests/README.md) for detailed documentation.

---

## Compatibility Summary

### Wire-Compatible
- IPC protocol (verified with libqb clients)
- DFSM message format (binary compatible)
- Database schema (SQLite version 5)
- RRD file formats (all versions)
- FUSE operations (all 12 ops)

### Different but Compatible
- Logging system (tracing vs qb_log) - format differs, functionality same
- IPC implementation (custom vs libqb) - protocol identical, implementation differs
- Event loop (tokio vs qb_loop) - both provide event-driven concurrency

### Intentionally Different
- OpenVZ support (removed, not needed)
- Service priority levels (all run concurrently in Rust)

---

## References

- **C Implementation**: `src/pmxcfs/`
- **Rust Implementation**: `src/pmxcfs-rs/`
  - `pmxcfs` - Main daemon binary
  - `pmxcfs-config` - Configuration management
  - `pmxcfs-api-types` - Shared types and error definitions
  - `pmxcfs-memdb` - In-memory database with SQLite persistence
  - `pmxcfs-dfsm` - Distributed Finite State Machine (CPG integration)
  - `pmxcfs-rrd` - RRD persistence
  - `pmxcfs-status` - Status monitoring and RRD data management
  - `pmxcfs-ipc` - libqb-compatible IPC server
  - `pmxcfs-services` - Service framework for lifecycle management
  - `pmxcfs-logger` - Cluster log with ring buffer and deduplication
- **Testing Guide**: `integration-tests/README.md`
- **Test Runner**: `integration-tests/test` (unified test interface)
