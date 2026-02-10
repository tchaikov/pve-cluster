# pmxcfs-dfsm

**Distributed Finite State Machine** for cluster-wide state synchronization in pmxcfs.

This crate implements the DFSM protocol used to replicate configuration changes and status updates across all nodes in a Proxmox cluster via Corosync CPG (Closed Process Group).

## Overview

The DFSM is the core mechanism for maintaining consistency across cluster nodes. It ensures that:

- All nodes see filesystem operations (writes, creates, deletes) in the same order
- Database state remains synchronized even after network partitions
- Status information (VM states, RRD data) is broadcast to all nodes
- State verification catches inconsistencies

## Architecture

### Key Components

### Module Structure

| Module | Purpose | C Equivalent |
|--------|---------|--------------|
| `state_machine.rs` | Core DFSM logic, state transitions | `dfsm.c` |
| `cluster_database_service.rs` | MemDb sync service | `dcdb.c`, `loop.c:service_dcdb` |
| `status_sync_service.rs` | Status/kvstore sync service | `loop.c:service_status` |
| `cpg_service.rs` | Corosync CPG integration | `dfsm.c:cpg_callbacks` |
| `dfsm_message.rs` | Protocol message types | `dfsm.c:dfsm_message_*_header_t` |
| `message.rs` | Message trait and serialization | (inline in C) |
| `wire_format.rs` | C-compatible wire format | `dcdb.c:c_fuse_message_header_t` |
| `broadcast.rs` | Cluster-wide message broadcast | `dcdb.c:dcdb_send_fuse_message` |
| `types.rs` | Type definitions (modes, epochs) | `dfsm.c:dfsm_mode_t` |

## C to Rust Mapping

### Data Structures

| C Type | Rust Type | Notes |
|--------|-----------|-------|
| `dfsm_t` | `Dfsm` | Main state machine |
| `dfsm_mode_t` | `DfsmMode` | Enum with type safety |
| `dfsm_node_info_t` | (internal) | Node state tracking |
| `dfsm_sync_info_t` | (internal) | Sync session info |
| `dfsm_callbacks_t` | Trait-based callbacks | Type-safe callbacks via traits |
| `dfsm_message_*_header_t` | `DfsmMessage` | Type-safe enum variants |

### Functions

#### Core DFSM Operations

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `dfsm_new()` | `Dfsm::new()` | state_machine.rs |
| `dfsm_initialize()` | `Dfsm::init_cpg()` | state_machine.rs |
| `dfsm_join()` | (part of init_cpg) | state_machine.rs |
| `dfsm_dispatch()` | `Dfsm::dispatch_events()` | state_machine.rs |
| `dfsm_send_message()` | `Dfsm::send_message()` | state_machine.rs |
| `dfsm_send_update()` | `Dfsm::send_update()` | state_machine.rs |
| `dfsm_verify_request()` | `Dfsm::verify_request()` | state_machine.rs |
| `dfsm_finalize()` | `Dfsm::stop_services()` | state_machine.rs |

#### DCDB (Cluster Database) Operations

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `dcdb_new()` | `ClusterDatabaseService::new()` | cluster_database_service.rs |
| `dcdb_send_fuse_message()` | `broadcast()` | broadcast.rs |
| `dcdb_send_unlock()` | `FuseMessage::Unlock` + broadcast | broadcast.rs |
| `service_dcdb()` | `ClusterDatabaseService` | cluster_database_service.rs |

#### Status Sync Operations

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `service_status()` | `StatusSyncService` | status_sync_service.rs |
| (kvstore CPG group) | `StatusSyncService` | Uses separate CPG group |

### Callback System

**C Implementation:**

**Rust Implementation:**
- Uses trait-based callbacks instead of function pointers
- Callbacks are implemented by `MemDbCallbacks` (memdb integration)
- Defined in external crates (pmxcfs-memdb)

## Synchronization Protocol

The DFSM ensures all nodes maintain consistent database state through a multi-phase synchronization protocol:

### Protocol Phases

#### Phase 1: Membership Change

When nodes join or leave the cluster:

1. **Corosync CPG** delivers membership change notification
2. **DFSM invalidates** cached checksums
3. **Message queues** are cleared
4. **Epoch counter** is incremented

**CPG Leader** (lowest node ID):
- Initiates sync by sending `SyncStart` message
- Sends its own `State` (CPG doesn't loop back messages)

**All Followers**:
- Respond to `SyncStart` by sending their `State`
- Wait for other nodes' states

#### Phase 2: State Exchange

Each node collects `State` messages containing serialized **MemDbIndex** (compact state summary using C-compatible wire format).

State digests are computed using SHA-256 hashing to detect differences between nodes.

#### Phase 3: Leader Election

When all states are collected, `process_state_update()` is called:

1. **Parse indices** from all node states
2. **Elect data leader** (may differ from CPG leader):
   - Highest `version` wins
   - If tied, highest `mtime` wins
3. **Identify synced nodes**: Nodes whose index matches leader exactly
4. **Determine own status**:
   - If we're the data leader → send updates to followers
   - If we're synced with leader → mark as Synced
   - Otherwise → enter Update mode and wait

**Leader Election Algorithm**:

#### Phase 4: Incremental Updates

**Data Leader** (node with highest version):

1. **Compare indices** using `find_differences()` for each follower
2. **Serialize differing entries** to C-compatible TreeEntry format
3. **Send Update messages** via CPG
4. **Send UpdateComplete** when all updates sent

**Followers** (out-of-sync nodes):

1. **Receive Update messages**
2. **Deserialize TreeEntry** via `TreeEntry::deserialize_from_update()`
3. **Apply to database** via `MemDb::apply_tree_entry()`:
   - INSERT OR REPLACE in SQLite
   - Update in-memory structures
   - Handle entry moves (parent/name changes)
4. **On UpdateComplete**: Transition to Synced mode

#### Phase 5: Normal Operations

When in **Synced** mode:

- FUSE operations are broadcast via `send_fuse_message()`
- Messages are delivered immediately via `deliver_message()`
- Leader periodically sends `VerifyRequest` for checksum comparison
- Nodes respond with `Verify` containing SHA-256 of entire database
- Mismatches trigger cluster resync

---

## Protocol Details

### State Machine Transitions

Based on analysis of C implementation (`dfsm.c` lines 795-1209):

#### Critical Protocol Rules

1. **Epoch Management**:
   - Each node creates local epoch during confchg: `(counter++, time, own_nodeid, own_pid)`
   - **Leader sends SYNC_START with its epoch**
   - **Followers MUST adopt leader's epoch from SYNC_START** (`dfsm->sync_epoch = header->epoch`)
   - All STATE messages in sync round use adopted epoch
   - Epoch mismatch → message discarded (may lead to LEAVE)

2. **Member List Validation**:
   - Built from `member_list` in confchg callback
   - Stored in `dfsm->sync_info->nodes[]`
   - STATE sender MUST be in this list
   - Non-member STATE → immediate LEAVE

3. **Duplicate Detection**:
   - Each node sends STATE exactly once per sync round
   - Tracked via `ni->state` pointer (NULL = not received, non-NULL = received)
   - Duplicate STATE from same nodeid/pid → immediate LEAVE
   - ✅ **FIXED**: Rust implementation now matches C (see commit c321869cc)

4. **Message Ordering** (one sync round):
   
5. **Leader Selection**:
   - Determined by `lowest_nodeid` from member list
   - Set in confchg callback before any messages sent
   - Used to validate SYNC_START sender (logged but not enforced)
   - Re-elected during state processing based on DB versions

### DFSM States (DfsmMode)

| State | Value | Description | C Equivalent |
|-------|-------|-------------|--------------|
| `Start` | 0 | Initial connection | `DFSM_MODE_START` |
| `StartSync` | 1 | Beginning sync | `DFSM_MODE_START_SYNC` |
| `Synced` | 2 | Fully synchronized | `DFSM_MODE_SYNCED` |
| `Update` | 3 | Receiving updates | `DFSM_MODE_UPDATE` |
| `Leave` | 253 | Leaving group | `DFSM_MODE_LEAVE` |
| `VersionError` | 254 | Protocol mismatch | `DFSM_MODE_VERSION_ERROR` |
| `Error` | 255 | Error state | `DFSM_MODE_ERROR` |

### Message Types (DfsmMessageType)

| Type | Value | Purpose |
|------|-------|---------|
| `Normal` | 0 | Application messages (with header + payload) |
| `SyncStart` | 1 | Start sync (from leader) |
| `State` | 2 | Full state data |
| `Update` | 3 | Incremental update |
| `UpdateComplete` | 4 | End of updates |
| `VerifyRequest` | 5 | Request state verification |
| `Verify` | 6 | State checksum response |

All messages use C-compatible wire format with headers and payloads.

### Application Message Types

The DFSM can carry two types of application messages:

1. **Fuse Messages** (Filesystem operations)
   - CPG Group: `pmxcfs_v1` (DCDB)
   - Message types: `Write`, `Create`, `Delete`, `Mkdir`, `Rename`, `SetMtime`, `Unlock`
   - Defined in: `pmxcfs-api-types::FuseMessage`

2. **KvStore Messages** (Status/RRD sync)
   - CPG Group: `pve_kvstore_v1`
   - Message types: `Data` (key-value pairs for status sync)
   - Defined in: `pmxcfs-api-types::KvStoreMessage`

### Wire Format Compatibility

All wire formats are **byte-compatible** with the C implementation. Messages include appropriate headers and payloads as defined in the C protocol.

## Synchronization Flow

### 1. Node Join

### 2. Normal Operation

### 3. State Verification (Periodic)

## Key Differences from C Implementation

### Event Loop Architecture

**C Version:**
- Uses libqb's `qb_loop` for event loop
- CPG fd registered with `qb_loop_poll_add()`
- Dispatch called from qb_loop when fd is readable

**Rust Version:**
- Uses tokio async runtime
- Service trait provides `dispatch()` method
- ServiceManager polls fd using tokio's async I/O
- No qb_loop dependency

### CPG Instance Management

**C Version:**
- Single DFSM struct with callbacks
- Two different CPG groups created separately

**Rust Version:**
- Each CPG group gets its own `Dfsm` instance
- `ClusterDatabaseService` - manages `pmxcfs_v1` CPG group (MemDb)
- `StatusSyncService` - manages `pve_kvstore_v1` CPG group (Status/RRD)
- Both use same DFSM protocol but different callbacks

## Error Handling

### Split-Brain Prevention

- Checksum verification detects divergence
- Automatic resync on mismatch
- Version monotonicity ensures forward progress

### Network Partition Recovery

- Membership changes trigger sync
- Highest version always wins
- Stale data is safely replaced

### Consistency Guarantees

- SQLite transactions ensure atomic updates
- In-memory structures updated atomically
- Version increments are monotonic
- All nodes converge to same state

## Compatibility Matrix

| Feature | C Version | Rust Version | Compatible |
|---------|-----------|--------------|------------|
| Wire format | `dfsm_message_*_header_t` | `DfsmMessage::serialize()` | Yes |
| CPG protocol | libcorosync | rust-corosync | Yes |
| Message types | 0-6 | `DfsmMessageType` | Yes |
| State machine | `dfsm_mode_t` | `DfsmMode` | Yes |
| Protocol version | 1 | 1 | Yes |
| Group names | `pmxcfs_v1`, `pve_kvstore_v1` | Same | Yes |

## Known Issues / TODOs

### Missing Features
- [ ] **Sync message batching**: C version can batch updates, Rust sends individually
- [ ] **Message queue limits**: C has MAX_QUEUE_LEN, Rust unbounded (potential memory issue)
- [ ] **Detailed error codes**: C returns specific CS_ERR_* codes, Rust uses anyhow errors

### Behavioral Differences (Benign)
- **Logging**: Rust uses `tracing` instead of `qb_log` (compatible with journald)
- **Threading**: Rust uses tokio tasks, C uses qb_loop single-threaded model
- **Timers**: Rust uses tokio timers, C uses qb_loop timers (same timeout values)

### Incompatibilities (None Known)
No incompatibilities have been identified. The Rust implementation is fully wire-compatible and can operate in a mixed C/Rust cluster.

## References

### C Implementation
- `src/pmxcfs/dfsm.c` / `dfsm.h` - Core DFSM implementation
- `src/pmxcfs/dcdb.c` / `dcdb.h` - Distributed database coordination
- `src/pmxcfs/loop.c` / `loop.h` - Service loop and management

### Related Crates
- **pmxcfs-memdb**: Database callbacks for DFSM
- **pmxcfs-status**: Status tracking and kvstore
- **pmxcfs-api-types**: Message type definitions
- **pmxcfs-services**: Service framework for lifecycle management
- **rust-corosync**: CPG bindings (external dependency)

### Corosync Documentation
- CPG (Closed Process Group) API: https://github.com/corosync/corosync
- Group communication semantics: Total order, virtual synchrony
