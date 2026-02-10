# pmxcfs-logger

Cluster-wide log management for pmxcfs, fully compatible with the C implementation (logger.c).

## Overview

This crate implements a cluster log system matching Proxmox's C-based logger.c behavior. It provides:

- **Ring Buffer Storage**: Circular buffer for log entries with automatic capacity management
- **FNV-1a Hashing**: Hashing for node and identity-based deduplication
- **Deduplication**: Per-node tracking of latest log entries to avoid duplicates
- **Time-based Sorting**: Chronological ordering of log entries across nodes
- **Multi-node Merging**: Combining logs from multiple cluster nodes
- **JSON Export**: Web UI-compatible JSON output matching C format

## Architecture

### Key Components

1. **LogEntry** (`entry.rs`): Individual log entry with automatic UID generation
2. **RingBuffer** (`ring_buffer.rs`): Circular buffer with capacity management
3. **ClusterLog** (`lib.rs`): Main API with deduplication and merging
4. **Hash Functions** (`hash.rs`): FNV-1a implementation matching C

## C to Rust Mapping

| C Function | Rust Equivalent | Location |
|------------|-----------------|----------|
| `fnv_64a_buf` | `hash::fnv_64a` | hash.rs |
| `clog_pack` | `LogEntry::pack` | entry.rs |
| `clog_copy` | `RingBuffer::add_entry` | ring_buffer.rs |
| `clog_sort` | `RingBuffer::sort` | ring_buffer.rs |
| `clog_dump_json` | `RingBuffer::dump_json` | ring_buffer.rs |
| `clusterlog_insert` | `ClusterLog::insert` | lib.rs |
| `clusterlog_add` | `ClusterLog::add` | lib.rs |
| `clusterlog_merge` | `ClusterLog::merge` | lib.rs |
| `dedup_lookup` | `ClusterLog::dedup_lookup` | lib.rs |

## Key Differences from C

1. **No `node_digest` in DedupEntry**: C stores `node_digest` both as HashMap key and in the struct. Rust only uses it as the key, saving 8 bytes per entry.

2. **Mutex granularity**: C uses a single global mutex. Rust uses separate Arc<Mutex<>> for buffer and dedup table, allowing better concurrency.

3. **Code size**: Rust implementation is ~24% the size of C (740 lines vs 3,000+) while maintaining equivalent functionality.

## Integration

This crate is integrated into `pmxcfs-status` to provide cluster log functionality. The `.clusterlog` FUSE plugin uses this to provide JSON log output compatible with the Proxmox web UI.

## References

### C Implementation
- `src/pmxcfs/logger.c` / `logger.h` - Cluster log implementation

### Related Crates
- **pmxcfs-status**: Integrates ClusterLog for status tracking
- **pmxcfs**: FUSE plugin exposes cluster log via `.clusterlog`
