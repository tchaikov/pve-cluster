# IPC Compatibility Testing Improvements

## Summary

Added comprehensive integration tests to verify IPC compatibility fixes between the Rust pmxcfs implementation and C clients using libqb protocol.

## New Test Files

### 1. `tests/ipc/03-log-cluster-msg.sh`
Tests LOG_CLUSTER_MSG IPC operation (op code 7).

**Verifies:**
- Cluster log messages can be sent via IPC
- Messages appear in cluster log with correct fields
- Various string lengths are handled correctly
- **Critical:** Null terminator handling matches C implementation

**Bug caught:** Rust was treating `ident_len`/`tag_len` as string lengths WITHOUT null terminator, but C expects these to INCLUDE the null terminator.

### 2. `tests/ipc/04-get-cluster-log.sh`
Tests GET_CLUSTER_LOG IPC operation (op code 8).

**Verifies:**
- GET_CLUSTER_LOG without user filter returns all entries
- GET_CLUSTER_LOG with user filter correctly filters by ident field
- Multiple user filters work independently (alice, bob, charlie)
- max_entries limit is enforced
- max_entries=0 defaults to 50

**Bug caught:** Rust was ignoring the `user` parameter in GET_CLUSTER_LOG requests. C implementation filters log entries by ident field.

### 3. `tests/ipc/05-get-rrd-dump.sh`
Tests GET_RRD_DUMP IPC operation (op code 10).

**Verifies:**
- GET_RRD_DUMP returns RRD data
- **Critical:** NUL terminator is present at end of data
- RRD dump format is correct (key:data\n)
- Caching behavior matches C (2 seconds, not 3)
- Perl compatibility maintained (never returns undef)

**Bugs caught:**
- M2: Missing NUL terminator (Perl compatibility issue)
- M1: Cache duration mismatch (3 seconds vs 2 seconds)

## Testing Methodology

### Direct IPC Testing
Unlike existing tests that verify functionality indirectly through FUSE operations, these new tests:

1. **Use actual libqb IPC client** (`PVE::IPCC` Perl module)
2. **Send real IPC requests** with exact wire format
3. **Verify responses** match C implementation behavior
4. **Test edge cases** (empty strings, various lengths, null terminators)

### Test Structure
Each test follows this pattern:
```bash
1. Setup: Prepare test data
2. Execute: Send IPC request via PVE::IPCC
3. Verify: Check response format and content
4. Validate: Ensure behavior matches C implementation
```

### Perl IPC Client
Tests use Perl's `PVE::IPCC` module which wraps libqb:
```perl
use PVE::IPCC;

# Build request with exact C struct format
my $request = pack("CCC", $priority, $ident_len, $tag_len);
$request .= $ident . "\0";
$request .= $tag . "\0";
$request .= $message . "\0";

# Send via libqb IPC
my $result = PVE::IPCC::ipcc_send_rec($CFS_IPC_LOG_CLUSTER_MSG, $request);
```

## Compatibility Issues Found and Fixed

### Issue 1: LOG_CLUSTER_MSG Null Terminator Handling
**Location:** `pmxcfs/src/ipc/request.rs:144-209`

**Problem:**
```rust
// WRONG: Treating ident_len as string length
let ident_data = &data[3..3 + ident_len];
let ident = std::ffi::CStr::from_bytes_until_nul(ident_data)?;
```

**C expects:**
```c
// ident_len INCLUDES null terminator
uint8_t ident_len = strlen(ident) + 1;
```

**Fix:**
```rust
// CORRECT: Validate null terminator is at expected position
let ident_data = &data[3..3 + ident_len];
if ident_data[ident_len - 1] != 0 {
    anyhow::bail!("LOG_CLUSTER_MSG: ident not null-terminated");
}
let ident = std::ffi::CStr::from_bytes_with_nul(ident_data)?;
```

**Test:** `03-log-cluster-msg.sh` Test 4

### Issue 2: GET_CLUSTER_LOG Missing User Parameter
**Location:** `pmxcfs/src/ipc/request.rs:212-236`, `pmxcfs/src/ipc/service.rs`, `pmxcfs-status/src/status.rs`

**Problem:**
```rust
// WRONG: Ignoring user parameter
GetClusterLog { max_entries: usize },
```

**C expects:**
```c
// C struct has user string for filtering
struct {
    uint32_t max_entries;
    uint32_t res1, res2, res3;
    char user[];  // Filter by ident field
};
```

**Fix:**
```rust
// CORRECT: Parse and use user parameter
GetClusterLog { max_entries: usize, user: String },

// Parse user string from request
let user = std::ffi::CStr::from_bytes_until_nul(&data[HEADER_SIZE..])?
    .to_str()?
    .to_string();

// Filter log entries by ident field
pub fn get_log_entries_filtered(&self, max: usize, user: &str) -> Vec<ClusterLogEntry> {
    if user.is_empty() {
        return self.get_log_entries(max);
    }
    self.cluster_log
        .get_entries(max * 10)
        .into_iter()
        .filter(|entry| entry.ident == user)
        .take(max)
        .collect()
}
```

**Test:** `04-get-cluster-log.sh` Tests 3-4

### Issue 3: GET_RRD_DUMP Missing NUL Terminator (M2)
**Location:** `pmxcfs-status/src/status.rs:351`

**Problem:**
```rust
// WRONG: No NUL terminator
for entry in rrd.values() {
    result.push_str(&entry.key);
    result.push(':');
    result.push_str(&entry.data);
    result.push('\n');
}
// Missing: result.push('\0');
```

**C expects:**
```c
// C appends NUL byte for Perl compatibility
g_string_append_c(str, 0); // never return undef
```

**Fix:**
```rust
// CORRECT: Append NUL terminator
for entry in rrd.values() {
    result.push_str(&entry.key);
    result.push(':');
    result.push_str(&entry.data);
    result.push('\n');
}
result.push('\0');  // Perl compatibility
```

**Test:** `05-get-rrd-dump.sh` Test 2

### Issue 4: RRD Dump Cache Duration (M1)
**Location:** `pmxcfs-status/src/status.rs:330`

**Problem:**
```rust
// WRONG: 3 seconds
const CACHE_SECONDS: u64 = 3;
```

**C expects:**
```c
// C caches for 2 seconds
if (rrd_dump_buf && (ctime - rrd_dump_last) < 2) {
```

**Fix:**
```rust
// CORRECT: 2 seconds
const CACHE_SECONDS: u64 = 2;
```

**Test:** `05-get-rrd-dump.sh` Test 4

## Test Execution

### Prerequisites
- pmxcfs binary built and running
- Perl with `PVE::IPCC` module installed
- `jq` for JSON parsing (optional)

### Running Tests
```bash
# Run all IPC tests
cd src/pmxcfs-rs/integration-tests
./test ipc

# Run specific test
bash tests/ipc/03-log-cluster-msg.sh
bash tests/ipc/04-get-cluster-log.sh
bash tests/ipc/05-get-rrd-dump.sh

# Run in container
./test --build ipc
```

### Expected Output
```
=========================================
LOG_CLUSTER_MSG IPC Operation Test
=========================================

Test 1: Send cluster log message via IPC
✓ LOG_CLUSTER_MSG IPC call succeeded

Test 2: Verify message appears in cluster log
✓ Test message found in cluster log

Test 3: Test with various string lengths
✓ All string length variations succeeded: 3/3

Test 4: Test null terminator handling
✓ Null terminator handling correct

=========================================
✓ All LOG_CLUSTER_MSG tests passed
=========================================
```

## Benefits

### 1. Direct Protocol Testing
- Tests actual IPC wire protocol, not just high-level functionality
- Catches subtle compatibility issues (null terminators, string lengths)
- Verifies exact C struct layout compatibility

### 2. Regression Prevention
- Tests specifically target fixed bugs
- Will catch if fixes are accidentally reverted
- Documents expected behavior for future developers

### 3. Documentation
- Tests serve as executable documentation
- Show exact wire format for each IPC operation
- Demonstrate C/Rust string handling differences

### 4. Confidence
- Proves Rust implementation is wire-compatible with C clients
- Validates that existing Perl tools will work with Rust pmxcfs
- Enables safe deployment of Rust implementation

## Future Work

### Additional IPC Operations to Test
- GET_FS_VERSION (1)
- GET_CLUSTER_INFO (2)
- GET_GUEST_LIST (3)
- SET_STATUS (4) / GET_STATUS (5)
- GET_CONFIG (6)
- VERIFY_TOKEN (12)
- GET_GUEST_CONFIG_PROPERTY (11)
- GET_GUEST_CONFIG_PROPERTIES (13)

### Stress Testing
- High-frequency IPC calls
- Large message sizes (up to MAX_MSG_SIZE)
- Concurrent IPC clients
- Error injection and recovery

### Mixed Cluster Testing
- C client → Rust server
- Rust client → C server
- Cross-node IPC operations
- Failover scenarios

## Conclusion

These new integration tests provide comprehensive verification of IPC compatibility between Rust and C implementations. They directly test the wire protocol using actual libqb clients, catch subtle compatibility issues, and serve as regression tests for the fixes we've implemented.

The tests are designed to be:
- **Comprehensive:** Cover all fixed bugs and edge cases
- **Maintainable:** Clear structure and documentation
- **Reliable:** Graceful degradation when dependencies missing
- **Informative:** Detailed output explaining what's being tested

This testing infrastructure ensures that the Rust pmxcfs implementation is truly wire-compatible with existing C clients and can be safely deployed as a drop-in replacement.
