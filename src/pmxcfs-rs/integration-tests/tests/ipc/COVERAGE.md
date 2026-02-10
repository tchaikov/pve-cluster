# IPC Operation Test Coverage

This document summarizes the test coverage for all pmxcfs IPC operations.

## IPC Operations Summary

| Op Code | Operation Name | Test File | Status |
|---------|---------------|-----------|--------|
| 1 | GET_FS_VERSION | 06-readonly-ops.sh | ✅ Tested |
| 2 | GET_CLUSTER_INFO | 06-readonly-ops.sh | ✅ Tested |
| 3 | GET_GUEST_LIST | 06-readonly-ops.sh | ✅ Tested |
| 4 | SET_STATUS | 07-write-ops.sh | ✅ Tested |
| 5 | GET_STATUS | 06-readonly-ops.sh | ✅ Tested |
| 6 | GET_CONFIG | 06-readonly-ops.sh | ✅ Tested |
| 7 | LOG_CLUSTER_MSG | 03-log-cluster-msg.sh | ✅ Tested |
| 8 | GET_CLUSTER_LOG | 04-get-cluster-log.sh | ✅ Tested |
| 10 | GET_RRD_DUMP | 05-get-rrd-dump.sh | ✅ Tested |
| 11 | GET_GUEST_CONFIG_PROPERTY | 08-guest-config-ops.sh | ✅ Tested |
| 12 | VERIFY_TOKEN | 07-write-ops.sh | ✅ Tested |
| 13 | GET_GUEST_CONFIG_PROPERTIES | 08-guest-config-ops.sh | ✅ Tested |

**Coverage: 12/12 operations (100%)**

## Test Organization

### Infrastructure Tests
- **01-socket-api.sh** - Socket connectivity and basic IPC infrastructure
- **02-flow-control.sh** - Ring buffer flow control and backpressure

### Cluster Log Tests
- **03-log-cluster-msg.sh** - LOG_CLUSTER_MSG with null terminator handling
- **04-get-cluster-log.sh** - GET_CLUSTER_LOG with user filtering

### RRD Tests
- **05-get-rrd-dump.sh** - GET_RRD_DUMP with NUL terminator and caching

### Consolidated Operation Tests
- **06-readonly-ops.sh** - Read-only operations (ops 1, 2, 3, 5, 6)
- **07-write-ops.sh** - Write operations (ops 4, 12)
- **08-guest-config-ops.sh** - Guest config operations (ops 11, 13)

### Common Library
- **test-lib.sh** - Shared functions for all IPC tests

## Test Categories

### Read-Only Operations (6 operations)
Operations that don't modify state and can be called by any user:
- GET_FS_VERSION (op 1)
- GET_CLUSTER_INFO (op 2)
- GET_GUEST_LIST (op 3)
- GET_STATUS (op 5)
- GET_CONFIG (op 6) - with permission checks for priv/ paths
- GET_CLUSTER_LOG (op 8)
- GET_RRD_DUMP (op 10)
- GET_GUEST_CONFIG_PROPERTY (op 11)
- GET_GUEST_CONFIG_PROPERTIES (op 13)

### Write Operations (2 operations)
Operations that modify state and require root permissions (uid=0, gid=0):
- SET_STATUS (op 4)
- LOG_CLUSTER_MSG (op 7)

### Authentication Operations (1 operation)
Operations for token validation:
- VERIFY_TOKEN (op 12)

## Key Test Scenarios

### C Compatibility Fixes Verified
1. **LOG_CLUSTER_MSG null terminator handling**
   - ident_len/tag_len include null terminator (C-style strings)
   - Test: 03-log-cluster-msg.sh

2. **GET_CLUSTER_LOG user filtering**
   - User parameter for filtering by ident field
   - Test: 04-get-cluster-log.sh

3. **GET_RRD_DUMP NUL terminator**
   - Appends \0 at end for Perl compatibility
   - Test: 05-get-rrd-dump.sh

4. **GET_RRD_DUMP cache duration**
   - 2 seconds (not 3) to match C implementation
   - Test: 05-get-rrd-dump.sh

### Error Handling Verified
- EINVAL (22) - Invalid parameters
- ENOENT (2) - Not found
- EPERM (1) - Permission denied
- EIO (5) - I/O error

### Data Format Validation
- JSON response structure
- Field type checking (numbers, booleans, strings, arrays, hashes)
- C-style null-terminated strings
- Binary data packing (little-endian)

### Permission Checks
- Root-only operations (SET_STATUS, LOG_CLUSTER_MSG)
- Private path access (priv/ directory)
- Read-only client restrictions

## Running All Tests

```bash
cd src/pmxcfs-rs/integration-tests
./test ipc
```

This will run all 8 test files in sequence and verify 100% IPC operation coverage.
