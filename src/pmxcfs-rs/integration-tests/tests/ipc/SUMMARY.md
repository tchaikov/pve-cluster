# Integration Test Improvements Summary

## What Was Done

Created three new integration tests to verify IPC compatibility fixes using actual libqb clients (Perl `PVE::IPCC` module).

## New Test Files

### 1. `tests/ipc/03-log-cluster-msg.sh`
**Tests:** LOG_CLUSTER_MSG IPC operation (op code 7)

**Verifies the fix for:** C-style null-terminated string handling
- Rust was treating `ident_len`/`tag_len` as string lengths WITHOUT null terminator
- C expects these to INCLUDE the null terminator
- Test sends actual IPC messages and verifies they're logged correctly

**Key test:** Validates that `ident_len = strlen(ident) + 1` (includes null terminator)

### 2. `tests/ipc/04-get-cluster-log.sh`
**Tests:** GET_CLUSTER_LOG IPC operation (op code 8)

**Verifies the fix for:** Missing user filtering parameter
- Rust was ignoring the `user` parameter
- C filters log entries by ident field matching user string
- Test verifies filtering works correctly for different users

**Key tests:**
- GET_CLUSTER_LOG with empty user (returns all entries)
- GET_CLUSTER_LOG with user="alice" (returns only alice's entries)
- GET_CLUSTER_LOG with user="bob" (returns only bob's entries)

### 3. `tests/ipc/05-get-rrd-dump.sh`
**Tests:** GET_RRD_DUMP IPC operation (op code 10)

**Verifies the fixes for:**
- **M2:** Missing NUL terminator (Perl compatibility)
- **M1:** Cache duration mismatch (3 seconds → 2 seconds)

**Key test:** Verifies last byte of RRD dump is NUL (0x00)

## How These Tests Work

### Direct IPC Protocol Testing
Unlike existing tests that verify functionality indirectly through FUSE, these tests:

1. Use actual libqb IPC client (`PVE::IPCC` Perl module)
2. Send real IPC requests with exact C struct wire format
3. Verify responses match C implementation behavior
4. Test edge cases (null terminators, string lengths, filtering)

### Example Test Flow
```perl
# Build LOG_CLUSTER_MSG request with C struct format
my $priority = 6;
my $ident = "testuser";
my $tag = "test";
my $message = "Test message";

# CRITICAL: ident_len INCLUDES null terminator (C behavior)
my $ident_len = length($ident) + 1;
my $tag_len = length($tag) + 1;

# Pack request
my $request = pack("CCC", $priority, $ident_len, $tag_len);
$request .= $ident . "\0";
$request .= $tag . "\0";
$request .= $message . "\0";

# Send via libqb IPC
my $result = PVE::IPCC::ipcc_send_rec($CFS_IPC_LOG_CLUSTER_MSG, $request);
```

## Running the Tests

### Quick Start
```bash
cd src/pmxcfs-rs/integration-tests

# Run all IPC tests
./test ipc

# Run specific test
bash tests/ipc/03-log-cluster-msg.sh
bash tests/ipc/04-get-cluster-log.sh
bash tests/ipc/05-get-rrd-dump.sh
```

### Prerequisites
- pmxcfs binary built and running
- Perl with `PVE::IPCC` module (XS module wrapping libqb)
- `jq` for JSON parsing (optional)

### Expected Behavior
- Tests will **PASS** if fixes are working correctly
- Tests will **SKIP** if Perl/PVE::IPCC not available (graceful degradation)
- Tests will **FAIL** if there's a regression in the fixes

## What Gets Verified

### ✓ LOG_CLUSTER_MSG (03-log-cluster-msg.sh)
- [x] IPC message sending works
- [x] Messages appear in cluster log
- [x] Various string lengths handled correctly
- [x] Null terminator handling matches C (ident_len includes \0)

### ✓ GET_CLUSTER_LOG (04-get-cluster-log.sh)
- [x] GET_CLUSTER_LOG without filter returns all entries
- [x] GET_CLUSTER_LOG with user filter works
- [x] Filtering correctly matches ident field
- [x] max_entries limit is enforced
- [x] max_entries=0 defaults to 50

### ✓ GET_RRD_DUMP (05-get-rrd-dump.sh)
- [x] GET_RRD_DUMP operation works
- [x] NUL terminator is present (M2 fix)
- [x] RRD dump format is correct (key:data\n)
- [x] Caching behavior matches C (2 seconds, M1 fix)
- [x] Perl compatibility maintained (never returns undef)

## Benefits

1. **Regression Prevention:** Tests will catch if fixes are accidentally reverted
2. **Wire Protocol Validation:** Proves Rust is compatible with C libqb clients
3. **Documentation:** Tests serve as executable documentation of IPC protocol
4. **Confidence:** Validates that existing Perl tools work with Rust pmxcfs

## Documentation

- **README.md:** Comprehensive test documentation
- **TESTING-IMPROVEMENTS.md:** Detailed explanation of improvements and methodology

## Next Steps

These tests can be extended to cover:
- Remaining IPC operations (GET_FS_VERSION, GET_CLUSTER_INFO, etc.)
- Stress testing (high-frequency calls, large messages, concurrent clients)
- Mixed cluster testing (C client → Rust server, Rust client → C server)
- Error handling and edge cases

## Files Created

```
tests/ipc/
├── 03-log-cluster-msg.sh       # LOG_CLUSTER_MSG test
├── 04-get-cluster-log.sh       # GET_CLUSTER_LOG test
├── 05-get-rrd-dump.sh          # GET_RRD_DUMP test
├── README.md                   # Test documentation
└── TESTING-IMPROVEMENTS.md     # Detailed improvements doc
```

All tests are executable and follow the same pattern as existing integration tests.
