# Quick Start: Running IPC Integration Tests

## TL;DR

```bash
cd src/pmxcfs-rs/integration-tests
./test ipc
```

## What These Tests Do

The new IPC tests verify that our compatibility fixes work correctly by:
1. Using actual libqb IPC clients (Perl `PVE::IPCC`)
2. Sending real IPC requests with exact C struct wire format
3. Verifying responses match C implementation behavior

## Test Files

- `03-log-cluster-msg.sh` - Tests LOG_CLUSTER_MSG null terminator handling
- `04-get-cluster-log.sh` - Tests GET_CLUSTER_LOG user filtering
- `05-get-rrd-dump.sh` - Tests GET_RRD_DUMP NUL terminator and caching

## Running Tests

### Option 1: Run all IPC tests
```bash
cd src/pmxcfs-rs/integration-tests
./test ipc
```

### Option 2: Run specific test
```bash
cd src/pmxcfs-rs/integration-tests
bash tests/ipc/03-log-cluster-msg.sh
bash tests/ipc/04-get-cluster-log.sh
bash tests/ipc/05-get-rrd-dump.sh
```

### Option 3: Run in container (full isolation)
```bash
cd src/pmxcfs-rs/integration-tests
./test --build ipc
```

## Expected Output

### Success
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

### Graceful Skip (if Perl not available)
```
⚠ Warning: PVE::IPCC module not available, skipping test
  This test requires the Perl IPC client to be installed
```

## Prerequisites

### Required
- pmxcfs binary built (`cargo build --release`)
- pmxcfs daemon running
- Perl with `PVE::IPCC` module

### Optional
- `jq` for JSON parsing (tests degrade gracefully without it)

## What Gets Tested

### ✓ LOG_CLUSTER_MSG (op 7)
- Null terminator handling (ident_len/tag_len include \0)
- Various string lengths
- Message appears in cluster log

### ✓ GET_CLUSTER_LOG (op 8)
- User filtering parameter
- Empty user (returns all entries)
- Specific user (filters by ident field)
- max_entries limit

### ✓ GET_RRD_DUMP (op 10)
- NUL terminator at end (M2 fix)
- Cache duration (2 seconds, M1 fix)
- Perl compatibility (never returns undef)

## Troubleshooting

### "PVE::IPCC module not available"
Tests will skip gracefully. To run tests, you need:
- Perl development environment
- libqb-dev installed
- PVE::IPCC module built

### "Cannot connect to pmxcfs"
Make sure pmxcfs is running:
```bash
pgrep pmxcfs
# or
ps aux | grep pmxcfs
```

### "jq: command not found"
Tests will still run but with limited JSON validation. Install jq:
```bash
apt-get install jq  # Debian/Ubuntu
```

## Documentation

- `README.md` - Comprehensive test documentation
- `TESTING-IMPROVEMENTS.md` - Detailed explanation of improvements
- `SUMMARY.md` - Quick summary of what was done

## Next Steps

After running these tests successfully, you can:
1. Verify all workspace tests still pass: `cargo test --workspace`
2. Run full integration test suite: `./test --build`
3. Test in mixed C/Rust cluster: `./test --mixed --build`

## Questions?

- What do these tests verify? → See `SUMMARY.md`
- How do they work? → See `TESTING-IMPROVEMENTS.md`
- What's the test architecture? → See `README.md`
