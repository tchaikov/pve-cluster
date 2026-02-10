# Logger Integration Tests

Integration tests for cluster log synchronization feature.

## Test Files

### `01-clusterlog-basic.sh`
Single-node cluster log functionality:
- Verifies `.clusterlog` plugin file exists
- Validates JSON format and required fields

## Mixed Cluster Tests

The following tests require a mixed C/Rust cluster and are located in `../mixed-cluster/`:

### `04-c-rust-binary-validation.sh` (NEW)
C/Rust binary format compatibility validation (mixed cluster):
- Verifies buffer size constants match (131KB, not 5MB)
- Validates binary header structure (size, cpos fields)
- Tests entry structure across C and Rust nodes
- Verifies cross-node log synchronization
- Tests string length handling (u8 boundary)

**Catches bugs:**
- Bug #1: Constants mismatch (CLOG_DEFAULT_SIZE, CLOG_MAX_ENTRY_SIZE)
- Bug #2: Binary serialization size (full buffer capacity)
- Bug #3: String length u8 overflow
- Bug #8: Wraparound guards in deserialization

### `05-merge-correctness.sh` (NEW)
Cluster log merge semantics and ordering verification:
- Tests JSON entry ordering (newest-first)
- Verifies keep-first merge semantics (duplicate handling)
- Validates entry count consistency after merge
- Tests deduplication across nodes
- Verifies chronological order preservation
- Tests concurrent entry handling (atomicity)

**Catches bugs:**
- Bug #4: BTreeMap merge (keep-first vs overwrite)
- Bug #5: Merge iteration order
- Bug #6: Merge atomicity (single mutex)
- Bug #7: JSON dump order

### `06-stress-test.sh` (NEW)
Edge cases and robustness under high load:
- Tests long string handling (255-byte boundary)
- Tests high entry volume (buffer wraparound)
- Concurrent high load from multiple nodes
- Entry chain integrity verification
- System recovery after stress
- Memory bounds checking

**Catches bugs:**
- Bug #3: String length u8 overflow under stress
- Bug #8: Wraparound robustness

## Prerequisites

Build the Rust binary:
```bash
cd src/pmxcfs-rs
cargo build --release
```

## Running Tests

### Single Node Test
```bash
cd integration-tests
./test logger
```

### Multi-Node Cluster Test (Rust-only)
```bash
cd integration-tests
./test --cluster
```

### Mixed Cluster Test (C + Rust nodes)
Required for tests 05, 06, 07:
```bash
cd integration-tests
./test --mixed --build
```

## Test Coverage

The new tests (05-07) were designed to prevent regression of bugs fixed in commit e40cbca17:

| Bug | Description | Test Coverage |
|-----|-------------|--------------|
| #1 | Constants mismatch (CLOG_DEFAULT_SIZE 5MB→131KB) | Test 05 |
| #2 | Binary serialization size (return full capacity) | Test 05 |
| #3 | String length u8 overflow (node_len/ident_len/tag_len) | Tests 05, 07 |
| #4 | BTreeMap merge (keep-first vs overwrite) | Test 06 |
| #5 | Merge iteration order (.rev() bug) | Test 06 |
| #6 | Merge atomicity (single mutex) | Test 06 |
| #7 | JSON dump order (data.reverse()) | Test 06 |
| #8 | Wraparound guards (deserialization bounds) | Tests 05, 07 |
| #9 | Duplicate constants | Covered by unit tests |

## External Dependencies

- **Docker/Podman**: Container runtime for multi-node testing
- **Corosync**: Cluster communication (via docker-compose setup)

## References

- Main integration tests: `../../README.md`
- Test runner: `../../test`
