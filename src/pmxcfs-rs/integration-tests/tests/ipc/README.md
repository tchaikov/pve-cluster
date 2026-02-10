# IPC Integration Tests

Integration tests for pmxcfs IPC (Inter-Process Communication) operations using libqb protocol.

## Overview

These tests verify the IPC compatibility between the Rust pmxcfs implementation and C clients using the libqb IPC library. They use the Perl `PVE::IPCC` module (which wraps libqb) to send actual IPC requests and verify responses.

## Test Structure

### Perl Scripts (`perl/` directory)
Standalone Perl scripts for each IPC operation:
- **IPCTestLib.pm** - Common library with reusable functions
- **12 operation scripts** - One per IPC operation (get-fs-version.pl, get-cluster-info.pl, etc.)

Each Perl script:
- Can be run independently
- Takes command-line arguments
- Prints SUCCESS/FAILED
- Returns appropriate exit code

See [perl/README.md](perl/README.md) for details.

### Bash Test Drivers
- **09-all-ipc-ops.sh** - Single driver that runs all Perl scripts
- **test-lib.sh** - Common bash functions for test output formatting

### Legacy Tests (being phased out)
- 01-socket-api.sh, 02-flow-control.sh - Infrastructure tests
- 03-log-cluster-msg.sh, 04-get-cluster-log.sh, 05-get-rrd-dump.sh - Specific operation tests
- 06-readonly-ops.sh, 07-write-ops.sh, 08-guest-config-ops.sh - Consolidated tests

## Test Files

### `09-all-ipc-ops.sh` (RECOMMENDED)
**Comprehensive IPC operations test using Perl scripts**

Single bash driver that runs all 12 IPC operation tests using standalone Perl scripts from the `perl/` directory.

**What it tests:**
- All 12 IPC operations (100% coverage)
- Read-only operations (GET_FS_VERSION, GET_CLUSTER_INFO, GET_GUEST_LIST, GET_CONFIG, GET_STATUS, GET_RRD_DUMP)
- Write operations (SET_STATUS, LOG_CLUSTER_MSG)
- Authentication (VERIFY_TOKEN)
- Guest config operations (GET_GUEST_CONFIG_PROPERTY, GET_GUEST_CONFIG_PROPERTIES)
- Cluster log operations (GET_CLUSTER_LOG)

**Advantages:**
- Perl scripts can be tested independently
- Better code organization and maintainability
- Easier to debug individual operations
- Reusable Perl library (IPCTestLib.pm)

**Usage:**
```bash
# Run all tests
./09-all-ipc-ops.sh

# Run individual Perl script
./perl/get-fs-version.pl
./perl/get-cluster-log.pl 50 alice
```

### Common Library: `test-lib.sh`
Shared functions for IPC tests:
- `check_perl_requirements()` - Check Perl and PVE::IPCC availability
- `call_ipc()` - Call IPC operation with string data
- `call_ipc_binary()` - Call IPC operation with binary data
- `test_ipc_perl()` - Run Perl-based IPC test
- `validate_json_fields()` - Validate JSON response structure
- `print_section()` / `print_subsection()` - Test output formatting

### `01-socket-api.sh`
Basic socket connectivity test:
- Verifies abstract Unix socket exists (`@pve2`)
- Checks pmxcfs process is running
- Tests socket connectivity
- Validates FUSE operations (indirect IPC test)

### `02-flow-control.sh`
Flow control mechanism test:
- Tests ring buffer flow control
- Verifies backpressure handling

### `03-log-cluster-msg.sh` (NEW)
**LOG_CLUSTER_MSG IPC operation test**

Tests the fix for C-style null-terminated string handling in LOG_CLUSTER_MSG parsing.

**What it tests:**
- Sending cluster log messages via IPC
- Verifying messages appear in cluster log
- Various string lengths (minimal, normal, long)
- **Critical:** Null terminator handling (ident_len/tag_len include null terminator)

**Bug fixed:**
- **Issue:** Rust was treating `ident_len`/`tag_len` as string lengths WITHOUT null terminator
- **C behavior:** These lengths INCLUDE the null terminator
- **Fix location:** `pmxcfs/src/ipc/request.rs:144-209`

**Test method:**
```perl
# C struct format:
#   uint8_t priority;
#   uint8_t ident_len;  // Length INCLUDING null terminator
#   uint8_t tag_len;    // Length INCLUDING null terminator
#   char data[];        // ident\0 + tag\0 + message\0

my $ident_len = length($ident) + 1;  # +1 for null terminator
my $tag_len = length($tag) + 1;      # +1 for null terminator
```

### `04-get-cluster-log.sh` (NEW)
**GET_CLUSTER_LOG IPC operation test**

Tests the fix for missing user parameter in GET_CLUSTER_LOG request parsing.

**What it tests:**
- GET_CLUSTER_LOG without user filter (empty string)
- GET_CLUSTER_LOG with user filter (filters by ident field)
- Multiple user filters (alice, bob, charlie)
- max_entries limit enforcement
- max_entries=0 defaults to 50

**Bug fixed:**
- **Issue:** Rust was ignoring the `user` parameter in GET_CLUSTER_LOG requests
- **C behavior:** Filters log entries by ident field matching user string
- **Fix location:** `pmxcfs/src/ipc/request.rs:212-236`, `pmxcfs/src/ipc/service.rs`, `pmxcfs-status/src/status.rs`

**Test method:**
```perl
# C struct format:
#   uint32_t max_entries;
#   uint32_t res1, res2, res3;  // reserved
#   char user[];  // null-terminated user string for filtering

my $request = pack("LLLL", $max_entries, 0, 0, 0);
$request .= $user . "\0";
```

### `05-get-rrd-dump.sh` (NEW)
**GET_RRD_DUMP IPC operation test**

Tests the M2 fix for missing NUL terminator in RRD dump output.

**What it tests:**
- GET_RRD_DUMP basic operation
- **Critical:** NUL terminator presence (last byte = 0)
- RRD dump format (key:data\n)
- Caching behavior (2 seconds, M1 fix)
- Perl compatibility (never returns undef)

**Bugs fixed:**
- **M2 (Medium):** Missing NUL terminator in RRD dump
  - **Issue:** Rust wasn't appending `\0` at end of RRD dump
  - **C behavior:** Appends NUL byte "never return undef" (Perl compatibility)
  - **Fix location:** `pmxcfs-status/src/status.rs:351`
- **M1 (Medium):** RRD dump cache duration mismatch
  - **Issue:** Rust cached for 3 seconds, C caches for 2 seconds
  - **Fix location:** `pmxcfs-status/src/status.rs:330`

**Test method:**
```perl
my $result = PVE::IPCC::ipcc_send_rec($CFS_IPC_GET_RRD_DUMP);

# Verify last byte is NUL
my $last_byte = substr($result, -1, 1);
my $last_byte_ord = ord($last_byte);
# Should be 0
```

### `06-readonly-ops.sh` (NEW)
**Read-only IPC operations test**

Consolidated test for multiple read-only IPC operations:
- **GET_FS_VERSION (op 1):** Returns filesystem version info (version=1, protocol=1, cluster status)
- **GET_CLUSTER_INFO (op 2):** Returns cluster member list with nodelist and quorate status
- **GET_GUEST_LIST (op 3):** Returns VM/CT list with version and ids
- **GET_CONFIG (op 6):** Reads configuration files from memdb, respects priv/ permissions
- **GET_STATUS (op 5):** Gets node status, validates empty name returns ENOENT

**What it tests:**
- JSON response format validation
- Required field presence
- Type checking (numbers, booleans, strings, arrays, hashes)
- Error handling (ENOENT, EPERM)
- Permission checks for private paths

### `07-write-ops.sh` (NEW)
**Write IPC operations test**

Tests write operations that require root permissions:
- **SET_STATUS (op 4):** Updates node status, tests roundtrip with GET_STATUS
- **VERIFY_TOKEN (op 12):** Validates authentication tokens
  - Empty token → EINVAL
  - Token with newline → EINVAL
  - Nonexistent token → ENOENT
  - Exact line matching (no trimming)

**What it tests:**
- Write permission enforcement (EPERM for non-root)
- Data persistence (SET_STATUS → GET_STATUS roundtrip)
- Input validation (empty strings, newlines)
- Token file parsing (priv/token.cfg)

### `08-guest-config-ops.sh` (NEW)
**Guest config IPC operations test**

Tests VM/CT configuration property retrieval:
- **GET_GUEST_CONFIG_PROPERTY (op 11):** Gets single property from VM config
- **GET_GUEST_CONFIG_PROPERTIES (op 13):** Gets multiple properties from VM config

**What it tests:**
- vmid validation (must be 0 or >= 100)
- Property name validation (must start with [a-z])
- vmid=0 behavior (returns properties from all VMs)
- Multiple property requests
- Error handling for nonexistent VMs (ENOENT)
- Error handling for invalid inputs (EINVAL)

## Prerequisites

### Required
- pmxcfs binary built and running
- Perl with `PVE::IPCC` module (XS module wrapping libqb)
- libqb IPC library

### Optional
- `jq` for JSON parsing (used in log verification)
- `socat` for socket testing

## Running Tests

### Run all IPC tests
```bash
cd src/pmxcfs-rs/integration-tests
./test ipc
```

### Run specific test
```bash
cd src/pmxcfs-rs/integration-tests
bash tests/ipc/03-log-cluster-msg.sh
bash tests/ipc/04-get-cluster-log.sh
bash tests/ipc/05-get-rrd-dump.sh
```

### Run in Docker container
```bash
cd src/pmxcfs-rs/integration-tests
./test --build ipc
```

## Test Coverage

These tests verify the following IPC operations:

| Operation | Op Code | Test File | What's Tested |
|-----------|---------|-----------|---------------|
| LOG_CLUSTER_MSG | 7 | 03-log-cluster-msg.sh | Null-terminated string parsing |
| GET_CLUSTER_LOG | 8 | 04-get-cluster-log.sh | User filtering parameter |
| GET_RRD_DUMP | 10 | 05-get-rrd-dump.sh | NUL terminator, caching |

### Other IPC operations (not yet tested)
- GET_FS_VERSION (1)
- GET_CLUSTER_INFO (2)
- GET_GUEST_LIST (3)
- SET_STATUS (4)
- GET_STATUS (5)
- GET_CONFIG (6)
- VERIFY_TOKEN (12)
- GET_GUEST_CONFIG_PROPERTY (11)
- GET_GUEST_CONFIG_PROPERTIES (13)

## Compatibility Fixes Verified

### 1. LOG_CLUSTER_MSG Null Terminator Handling
**Problem:** C expects `ident_len` and `tag_len` to INCLUDE the null terminator, but Rust was treating them as string lengths WITHOUT the null terminator.

**Fix:** Updated parsing to use `CStr::from_bytes_with_nul()` and validate null terminator position.

**Test:** `03-log-cluster-msg.sh` Test 4

### 2. GET_CLUSTER_LOG User Filtering
**Problem:** C implementation filters log entries by `ident` field using the `user` parameter, but Rust was ignoring this parameter.

**Fix:** Added `user` field to `IpcRequest::GetClusterLog`, updated parsing to read user string, added `get_log_entries_filtered()` method.

**Test:** `04-get-cluster-log.sh` Tests 3-4

### 3. GET_RRD_DUMP NUL Terminator (M2)
**Problem:** C appends NUL byte to RRD dump for Perl compatibility ("never return undef"), but Rust wasn't doing this.

**Fix:** Added `result.push('\0')` before returning RRD dump.

**Test:** `05-get-rrd-dump.sh` Test 2

### 4. RRD Dump Cache Duration (M1)
**Problem:** Rust cached RRD dumps for 3 seconds, C caches for 2 seconds.

**Fix:** Changed `CACHE_SECONDS` from 3 to 2.

**Test:** `05-get-rrd-dump.sh` Test 4

## Implementation Details

### IPC Protocol
The tests use the libqb IPC protocol:
- **Transport:** Abstract Unix socket (`@pve2`)
- **Wire format:** Request/response headers + data
- **Request header:** `struct qb_ipc_request_header { id, size }`
- **Response header:** `struct qb_ipc_response_header { id, size, error }`

### Perl IPC Client
The `PVE::IPCC` module provides:
```perl
my $result = PVE::IPCC::ipcc_send_rec($msgid, $data);
```

This wraps libqb's `qb_ipcc_sendv_recv()` function.

### C String Handling
Critical difference between C and Rust:
- **C:** String lengths often INCLUDE the null terminator
- **Rust:** String lengths are the actual string length WITHOUT null terminator

Example:
```c
// C code
char ident[] = "test";
uint8_t ident_len = 5;  // strlen("test") + 1 for '\0'
```

```rust
// Rust code (WRONG)
let ident = "test";
let ident_len = ident.len();  // 4 - WRONG!

// Rust code (CORRECT)
let ident = "test";
let ident_len = ident.len() + 1;  // 5 - includes null terminator
```

## Known Issues

### Test Dependencies
- Tests require `PVE::IPCC` Perl module to be installed
- Tests gracefully skip if dependencies are missing
- Some tests require `jq` for JSON parsing

### Test Timing
- RRD dump caching test may be timing-sensitive
- Cluster log tests may see messages from other sources

## Future Improvements

1. **Add tests for remaining IPC operations:**
   - GET_FS_VERSION
   - GET_CLUSTER_INFO
   - GET_GUEST_LIST
   - SET_STATUS / GET_STATUS
   - GET_CONFIG
   - VERIFY_TOKEN
   - GET_GUEST_CONFIG_PROPERTY/PROPERTIES

2. **Add stress tests:**
   - High-frequency IPC calls
   - Large message sizes
   - Concurrent IPC clients

3. **Add error handling tests:**
   - Invalid message formats
   - Malformed requests
   - Buffer overflow attempts

4. **Add mixed cluster tests:**
   - C client → Rust server
   - Rust client → C server
   - Cross-node IPC operations

## References

- **libqb documentation:** https://github.com/ClusterLabs/libqb
- **C implementation:** `src/pmxcfs/server.c`
- **Rust implementation:** `src/pmxcfs-rs/pmxcfs/src/ipc/`
- **IPC request parsing:** `src/pmxcfs-rs/pmxcfs/src/ipc/request.rs`
- **IPC service handlers:** `src/pmxcfs-rs/pmxcfs/src/ipc/service.rs`
- **Perl IPC client:** `src/PVE/IPCC.xs`
