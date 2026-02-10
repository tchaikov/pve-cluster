# pmxcfs-memdb

**In-Memory Database** with SQLite persistence for pmxcfs cluster filesystem.

This crate provides a thread-safe, cluster-synchronized in-memory database that serves as the backend storage for the Proxmox cluster filesystem. All filesystem operations (read, write, create, delete) are performed on in-memory structures with SQLite providing durable persistence.

## Overview

The MemDb is the core data structure that stores all cluster configuration files in memory for fast access while maintaining durability through SQLite. Changes are synchronized across the cluster using the DFSM protocol.

### Key Features

- **In-memory tree structure**: All filesystem entries cached in memory
- **SQLite persistence**: Durable storage with ACID guarantees
- **Cluster synchronization**: State replication via DFSM (pmxcfs-dfsm crate)
- **Version tracking**: Monotonically increasing version numbers for conflict detection
- **Resource locking**: File-level locks with timeout-based expiration
- **Thread-safe**: All operations protected by mutex
- **Size limits**: Enforces max file size (1 MiB) and total filesystem size (128 MiB)

## Architecture

### Module Structure

| Module | Purpose | C Equivalent |
|--------|---------|--------------|
| `database.rs` | Core MemDb struct and CRUD operations | `memdb.c` (main functions) |
| `types.rs` | TreeEntry, LockInfo, constants | `memdb.h:38-51, 71-74` |
| `locks.rs` | Resource locking functionality | `memdb.c:memdb_lock_*` |
| `sync.rs` | State serialization for cluster sync | `memdb.c:memdb_encode_index` |
| `index.rs` | Index comparison for DFSM updates | `memdb.c:memdb_index_*` |

## C to Rust Mapping

### Data Structures

| C Type | Rust Type | Notes |
|--------|-----------|-------|
| `memdb_t` | `MemDb` | Main database handle (Clone-able via Arc) |
| `memdb_tree_entry_t` | `TreeEntry` | File/directory entry |
| `memdb_index_t` | `MemDbIndex` | Serialized state for sync |
| `memdb_index_extry_t` | `IndexEntry` | Single index entry |
| `memdb_lock_info_t` | `LockInfo` | Lock metadata |
| `db_backend_t` | `Connection` | SQLite backend (rusqlite) |
| `GHashTable *index` | `HashMap<u64, TreeEntry>` | Inode index |
| `GHashTable *locks` | `HashMap<String, LockInfo>` | Lock table |
| `GMutex mutex` | `Mutex` | Thread synchronization |

### Core Functions

#### Database Lifecycle

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `memdb_open()` | `MemDb::open()` | database.rs |
| `memdb_close()` | (Drop trait) | Automatic |
| `memdb_checkpoint()` | (implicit in writes) | Auto-commit |

#### File Operations

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `memdb_read()` | `MemDb::read()` | database.rs |
| `memdb_write()` | `MemDb::write()` | database.rs |
| `memdb_create()` | `MemDb::create()` | database.rs |
| `memdb_delete()` | `MemDb::delete()` | database.rs |
| `memdb_mkdir()` | `MemDb::create()` (with DT_DIR) | database.rs |
| `memdb_rename()` | `MemDb::rename()` | database.rs |
| `memdb_mtime()` | (included in write) | database.rs |

#### Directory Operations

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `memdb_readdir()` | `MemDb::readdir()` | database.rs |
| `memdb_dirlist_free()` | (automatic) | Rust's Vec drops automatically |

#### Metadata Operations

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `memdb_getattr()` | `MemDb::lookup_path()` | database.rs |
| `memdb_statfs()` | `MemDb::statfs()` | database.rs |

#### Tree Entry Functions

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `memdb_tree_entry_new()` | `TreeEntry { ... }` | Struct initialization |
| `memdb_tree_entry_copy()` | `.clone()` | Automatic (derive Clone) |
| `memdb_tree_entry_free()` | (Drop trait) | Automatic |
| `tree_entry_debug()` | `{:?}` format | Automatic (derive Debug) |
| `memdb_tree_entry_csum()` | `TreeEntry::compute_checksum()` | types.rs |

#### Lock Operations

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `memdb_lock_expired()` | `MemDb::is_lock_expired()` | locks.rs |
| `memdb_update_locks()` | `MemDb::update_locks()` | locks.rs |

#### Index/Sync Operations

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `memdb_encode_index()` | `MemDb::get_index()` | sync.rs |
| `memdb_index_copy()` | `.clone()` | Automatic (derive Clone) |
| `memdb_compute_checksum()` | `MemDb::compute_checksum()` | sync.rs |
| `bdb_backend_commit_update()` | `MemDb::apply_tree_entry()` | database.rs |

#### State Synchronization

| C Function | Rust Equivalent | Location |
|-----------|-----------------|----------|
| `memdb_recreate_vmlist()` | (handled by status crate) | External |
| (implicit) | `MemDb::replace_all_entries()` | database.rs |

### SQLite Backend

**C Version (database.c):**
- Direct SQLite3 C API
- Manual statement preparation
- Explicit transaction management
- Manual memory management

**Rust Version (database.rs):**
- `rusqlite` crate for type-safe SQLite access

## Database Schema

The SQLite schema stores all filesystem entries with metadata:
- `inode = 1` is always the root directory
- `parent = 0` for root, otherwise parent directory's inode
- `version` increments on each modification (monotonic)
- `writer` is the node ID that made the change
- `mtime` is seconds since UNIX epoch
- `data` is NULL for directories, BLOB for files

## TreeEntry Wire Format

For cluster synchronization (DFSM Update messages), TreeEntry uses C-compatible serialization that is byte-compatible with C's implementation.

## Key Differences from C Implementation

### Thread Safety

**C Version:**
- Single `GMutex` protects entire memdb_t
- Callback-based access from qb_loop (single-threaded)

**Rust Version:**
- Mutex for each data structure (index, tree, locks, conn)
- More granular locking
- Can be shared across tokio tasks

### Data Structures

**C Version:**
- `GHashTable` (GLib) for index and tree
- Recursive tree structure with pointers

**Rust Version:**
- `HashMap` from std
- Flat structure: `HashMap<u64, HashMap<String, u64>>` for tree
- Separate `HashMap<u64, TreeEntry>` for index
- No recursive pointers (eliminates cycles)

### SQLite Integration

**C Version (database.c):**
- Direct SQLite3 C API

**Rust Version (database.rs):**
- `rusqlite` crate for type-safe SQLite access

## Constants

| Constant | Value | Purpose |
|----------|-------|---------|
| `MEMDB_MAX_FILE_SIZE` | 1 MiB | Maximum file size (matches C) |
| `MEMDB_MAX_FSSIZE` | 128 MiB | Maximum total filesystem size |
| `MEMDB_MAX_INODES` | 256k | Maximum number of files/dirs |
| `MEMDB_BLOCKSIZE` | 4096 | Block size for statfs |
| `LOCK_TIMEOUT` | 120 sec | Lock expiration timeout |
| `DT_DIR` | 4 | Directory type (matches POSIX) |
| `DT_REG` | 8 | Regular file type (matches POSIX) |

## Known Issues / TODOs

### Missing Features

- [ ] **vmlist regeneration**: `memdb_recreate_vmlist()` not implemented (handled by status crate's `scan_vmlist()`)
- [ ] **C integration tests**: No tests with real C-generated databases or Update messages
- [ ] **Concurrent access tests**: No multi-threaded stress tests for lock contention

### Behavioral Differences (Benign)

- **Lock storage**: C reads from filesystem at startup, Rust does the same but implementation differs
- **Index encoding**: Rust uses `Vec<IndexEntry>` instead of flexible array member
- **Checksum algorithm**: Same (SHA-256) but implementation differs (ring vs OpenSSL)

### Error Handling & Recovery

**Error Flag Behavior:**

When a database operation fails (e.g., SQLite error, transaction failure), the `errors` flag is set to `true` (matching C behavior in `memdb->errors`). Once set:
- **All subsequent operations will fail** with "Database has errors, refusing operation"
- **No automatic recovery mechanism** is provided
- **Manual intervention required:** Restart the pmxcfs daemon to clear the error state

This is a **fail-safe design** to prevent data corruption. If the database enters an inconsistent state due to an error, the system refuses all further operations rather than risk corrupting the cluster state.

**Production Impact:**
- A single database error will make the node unable to process further memdb operations
- The node must be restarted to recover
- This matches C implementation behavior

**Future Improvements:**
- [ ] Add error context to help diagnose which operation caused the error
- [ ] Consider adding a recovery mechanism (e.g., re-open database, validate consistency)
- [ ] Add monitoring/alerting for error flag state

### Path Normalization Strategy

**Internal Path Format:**

All paths are internally stored and processed as **absolute paths** with:
- Leading `/` (e.g., "/nodes/node1/qemu-server/100.conf")
- No trailing `/` except for root ("/")
- No `..` or `.` components

**C Compatibility:**

The C implementation sometimes sends paths without leading `/` (see `find_plug` in pmxcfs.c). The Rust implementation automatically normalizes these to absolute paths using `normalize_path()`.

**Security:**

Path traversal is prevented by:
1. Normalization removes leading/trailing slashes
2. Lock paths explicitly reject `..` components
3. All lookups go through `lookup_path()` which only follows valid tree structure

### Compatibility

- **Database format**: 100% compatible with C version (same SQLite schema)
- **Wire format**: TreeEntry serialization matches C byte-for-byte
- **Constants**: All limits match C version exactly

## References

### C Implementation
- `src/pmxcfs/memdb.c` / `memdb.h` - In-memory database
- `src/pmxcfs/database.c` - SQLite backend

### Related Crates
- **pmxcfs-dfsm**: Uses MemDb for cluster synchronization
- **pmxcfs-api-types**: Message types for FUSE operations
- **pmxcfs**: Main daemon and FUSE integration

### External Dependencies
- **rusqlite**: SQLite bindings
- **parking_lot**: Fast mutex implementation
- **sha2**: SHA-256 checksums
