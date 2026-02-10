# RRD Integration Tests

This directory contains integration tests for the pmxcfs-rrd component, verifying RRD (Round-Robin Database) functionality.

## Test Overview

### 01-rrd-basic.sh
**Purpose**: Verify basic RRD functionality
**Coverage**:
- RRD directory existence
- rrdtool availability check
- Basic RRD file creation
- RRD update operations
- RRD info queries
- pmxcfs RRD file pattern detection

**Dependencies**: rrdtool (optional - test degrades gracefully if not available)

---

### 02-schema-validation.sh
**Purpose**: Validate RRD schemas match pmxcfs-rrd specifications
**Coverage**:
- Node schema (pve2 format - 12 data sources)
- Node schema (pve9.0 format - 19 data sources)
- VM schema (pve2 format - 10 data sources)
- VM schema (pve9.0 format - 17 data sources)
- Storage schema (2 data sources)
- Data source types (GAUGE vs DERIVE)
- RRA (Round-Robin Archive) definitions
- Heartbeat values (120 seconds)
- Backward compatibility (pve9.0 includes pve2)

**Test Method**:
- Creates RRD files using rrdtool with exact schemas from `pmxcfs-rrd/src/schema.rs`
- Validates using `rrdtool info` to verify data sources and RRAs
- Compares against C implementation specifications

**Dependencies**: rrdtool (required - test skips if not available)

**Reference**: See `src/pmxcfs-rs/pmxcfs-rrd/src/schema.rs` for schema definitions

---

### 03-rrdcached-integration.sh (NEW)
**Purpose**: Verify pmxcfs integration with rrdcached daemon
**Coverage**:
- **Test 1**: rrdcached daemon startup and socket creation
- **Test 2**: RRD file creation through rrdcached
- **Test 3**: Cached updates (5 updates buffered in memory)
- **Test 4**: Cache flush to disk (FLUSH command)
- **Test 5**: Daemon stop/restart recovery
- **Test 6**: Data persistence across daemon restart
- **Test 7**: Journal file creation and recovery
- **Test 8**: Schema access through rrdcached

**Test Method**:
- Starts standalone rrdcached instance with Unix socket
- Creates RRD files using `rrdtool --daemon` option
- Performs updates through socket (cached mode)
- Tests FLUSH command to force disk writes
- Stops and restarts daemon to verify persistence
- Validates journal files for crash recovery
- Queries schema through daemon socket

**Dependencies**:
- rrdcached (required - test skips if not available)
- rrdtool (required - test skips if not available)
- socat (required for STATS/FLUSH commands)
- bc (required for floating-point math)

**Socket Protocol**:
- Uses Unix domain socket for communication
- Commands: STATS, FLUSH <filename>
- Response format: "0 Success" or error code

**rrdcached Options Used**:
- `-g`: Run in foreground (for testing)
- `-l unix:<path>`: Listen on Unix socket
- `-b <dir>`: Base directory for RRD files
- `-B`: Restrict access to base directory
- `-m 660`: Socket permissions
- `-p <file>`: PID file location
- `-j <dir>`: Journal directory for crash recovery
- `-F`: Flush all updates on shutdown
- `-w 5`: Write timeout (5 seconds)
- `-f 10`: Flush dead data interval (10 seconds)

**Why This Test Matters**:
- rrdcached provides write caching and batching for RRD updates
- Reduces disk I/O for high-frequency metric updates
- Provides crash recovery through journal files
- Used by pmxcfs in production for performance
- Validates that created RRD files work with caching daemon

---

## Running Tests

### Run all RRD tests:
```bash
cd src/pmxcfs-rs/integration-tests
./run-tests.sh --subsystem rrd
```

### Run specific test:
```bash
cd src/pmxcfs-rs/integration-tests
bash tests/rrd/01-rrd-basic.sh
bash tests/rrd/02-schema-validation.sh
bash tests/rrd/03-rrdcached-integration.sh
```

### Run in Docker container:
```bash
cd src/pmxcfs-rs/integration-tests
docker-compose run --rm test-node bash -c "bash /workspace/src/pmxcfs-rs/integration-tests/tests/rrd/03-rrdcached-integration.sh"
```

## Test Results

All tests are designed to:
- ✅ Pass when dependencies are available
- ⚠️ Skip gracefully when optional dependencies are missing
- ❌ Fail only on actual functional errors

## Dependencies Installation

For Debian/Ubuntu:
```bash
apt-get install rrdtool rrdcached socat bc
```

For testing container (already included in Dockerfile):
- rrdtool: v1.7.2+ (RRD command-line tool)
- rrdcached: v1.7.2+ (RRD caching daemon)
- librrd8t64: RRD library
- socat: Socket communication tool
- bc: Arbitrary precision calculator

## Implementation Notes

### Schema Validation
The schemas tested here **must match** the definitions in:
- `src/pmxcfs-rs/pmxcfs-rrd/src/schema.rs`
- C implementation in `src/pmxcfs/status.c`

Any changes to RRD schemas should update both:
1. The schema definition code
2. These validation tests

### rrdcached Integration
The daemon test validates the **client-side** behavior. The pmxcfs-rrd crate provides:
- `src/daemon.rs`: rrdcached client implementation
- `src/writer.rs`: RRD file creation and updates

This test ensures the protocol works end-to-end, even though it doesn't directly test the Rust client (that's covered by unit tests).

## Related Documentation

- pmxcfs-rrd README: `src/pmxcfs-rs/pmxcfs-rrd/README.md`
- Schema definitions: `src/pmxcfs-rs/pmxcfs-rrd/src/schema.rs`
- Test coverage evaluation: `src/pmxcfs-rs/integration-tests/TEST_COVERAGE_EVALUATION.md`
- RRDtool documentation: https://oss.oetiker.ch/rrdtool/doc/index.en.html
