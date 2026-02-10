# pmxcfs Integration Tests

Comprehensive integration test suite for validating pmxcfs-rs backward compatibility and production readiness.

## Quick Start

```bash
cd src/pmxcfs-rs/integration-tests

# First time - build and run all tests
./test --build

# Subsequent runs - skip build for speed
./test --no-build

# Run specific subsystem
./test rrd

# List available tests
./test --list

# Clean up and start fresh
./test --clean
```

## Test Runner: `./test`

Simple wrapper that handles all complexity:

```bash
./test [SUBSYSTEM] [OPTIONS]
```

### Options

- `--build` - Force rebuild of pmxcfs binary
- `--no-build` - Skip binary rebuild (faster iteration)
- `--cluster` - Run multi-node cluster tests (requires 3-node setup)
- `--mixed` - Run mixed C/Rust cluster tests
- `--clean` - Remove all containers and volumes
- `--list` - List all available test subsystems
- `--help` - Show detailed help

### Examples

```bash
# Run all single-node tests
./test

# Test specific subsystem with rebuild
./test rrd --build

# Quick iteration without rebuild
./test plugins --no-build

# Multi-node cluster tests
./test --cluster

# Clean everything and retry
./test --clean --build
```

## Directory Structure

```
integration-tests/
├── docker/                # Container infrastructure
│   ├── Dockerfile        # Test container image
│   ├── docker-compose.yml           # Main compose file
│   ├── docker-compose.cluster.yml   # Multi-node setup
│   └── lib/              # Support scripts
├── tests/                # Test suites organized by subsystem
│   ├── core/            # Core functionality
│   ├── fuse/            # FUSE operations
│   ├── memdb/           # Database tests
│   ├── ipc/             # IPC/socket tests
│   ├── rrd/             # RRD metrics
│   ├── status/          # Status tracking
│   ├── locks/           # Lock management
│   ├── plugins/         # Plugin system
│   ├── logger/          # Cluster log
│   ├── cluster/         # Multi-node cluster
│   ├── dfsm/            # DFSM synchronization
│   ├── mixed-cluster/   # C/Rust compatibility
│   └── run-c-tests.sh   # Perl compatibility tests
├── results/             # Test results (timestamped logs)
├── test                 # Main test wrapper
├── test-local           # Local testing without containers
└── run-tests.sh         # Core test runner
```

## Test Categories

### Single-Node Tests

Run locally without cluster setup. Compatible with `./test-local`.

| Subsystem | Description |
|-----------|-------------|
| core | Directory structure, version plugin |
| fuse | FUSE filesystem operations |
| memdb | Database access and integrity |
| ipc | Unix socket API compatibility |
| rrd | RRD file creation, schemas, rrdcached integration |
| status | Status tracking, VM registry, operations |
| locks | Lock management and concurrent access |
| plugins | Plugin file access and write operations |
| logger | Single-node cluster log functionality |

### Multi-Node Tests

Require cluster setup with `--cluster` flag.

| Subsystem | Description |
|-----------|-------------|
| cluster | Connectivity, file sync, log sync, binary format |
| dfsm | DFSM state machine, multi-node behavior |
| status | Multi-node status synchronization |
| logger | Multi-node cluster log synchronization |

### Mixed Cluster Tests

Test C and Rust pmxcfs interoperability with `--mixed` flag.

| Test | Description |
|------|-------------|
| 01-node-types.sh | Node type detection (C vs Rust) |
| 02-file-sync.sh | File synchronization between C and Rust nodes |
| 03-quorum.sh | Quorum behavior in heterogeneous cluster |

### Perl Compatibility Tests

Validates backward compatibility with Proxmox VE Perl tools.

**Run with**:
```bash
cd docker && docker compose run --rm c-tests
```

**What's tested**:
- PVE::Cluster module integration
- PVE::IPCC IPC compatibility (Perl -> Rust)
- PVE::Corosync configuration parser
- FUSE filesystem operations from Perl
- VM/CT configuration file handling

## Test Coverage

The test suite validates:

- FUSE filesystem operations (all 12 operations)
- Unix socket API compatibility (libqb wire protocol)
- Database operations (SQLite version 5)
- Plugin system (all 10 plugins: 6 functional + 4 link)
- RRD file creation and metrics
- Status tracking and VM registry
- Lock management and concurrent access
- Cluster log functionality
- Multi-node file synchronization
- DFSM state machine protocol
- Perl API compatibility (drop-in replacement validation)

## Local Testing (No Containers)

Fast iteration during development using `./test-local`:

```bash
# Run all local-compatible tests
./test-local

# Run specific tests
./test-local core/01-test-paths.sh memdb/01-access.sh

# Build first, keep temp directory for debugging
./test-local --build --keep-temp

# Run with debug logging
./test-local --debug
```

**Features**:
- No container overhead
- Uses pmxcfs `--test-dir` flag for isolation
- Fast iteration cycle
- Automatic cleanup (or keep with `--keep-temp`)

**Requirements**:
- pmxcfs binary built (`cargo build --release`)
- FUSE support (fusermount)
- SQLite
- No root required

## Container-Based Testing

Uses Docker/Podman for full isolation and reproducibility.

### Single Container Tests

```bash
cd docker
docker compose run --rm pmxcfs-test
```

Runs all single-node tests in isolated container.

### Perl Compatibility Tests

```bash
cd docker
docker compose run --rm c-tests
```

Validates integration with production Proxmox Perl tools.

### Multi-Node Cluster

```bash
cd docker
docker compose -f docker-compose.cluster.yml up
```

Starts 3-node Rust cluster for multi-node testing.

## Typical Workflows

### Development Iteration

```bash
# Edit code in src/pmxcfs-rs/

# Build and test
cd integration-tests
./test --build

# Quick iteration
# (make changes)
./test --no-build
```

### Working on Specific Feature

```bash
# Focus on RRD subsystem
./test rrd --build

# Iterate quickly
./test rrd --no-build
```

### Before Committing

```bash
# Run full test suite
./test --build

# Check results
cat results/test-results_*.log | tail -20
```

### Troubleshooting

```bash
# Containers stuck or failing mysteriously?
./test --clean

# Then retry
./test --build
```

## Test Results

Results are saved to timestamped log files in `results/`:

```
results/test-results_20251118_091234.log
```

## Environment Variables

- `SKIP_BUILD=true` - Skip cargo build (same as `--no-build`)
- `USE_PODMAN=true` - Force use of podman instead of docker

## Troubleshooting

### "Container already running" or lock errors

```bash
./test --clean
```

### "pmxcfs binary not found"

```bash
./test --build
```

### Tests timing out

Possible causes:
- Container not starting properly
- FUSE mount issues
- Previous containers not cleaned up

Solution:
```bash
./test --clean
./test --build
```

## Known Issues

### Multi-Node Cluster Tests

Multi-node cluster tests require:
- Docker network configuration
- Container-to-container networking
- Corosync CPG multicast support

Current limitations:
- Container IP access from host may not work
- Some tests require being run inside containers
- Mixed cluster tests need architecture refinement

### Test Runner Exit Codes

The test runner properly captures exit codes from test scripts using `set -o pipefail` to ensure pipeline failures are detected correctly.

## Creating New Tests

### Test Template

```bash
#!/bin/bash
# Test: [Test Name]
# [Description]

set -e

echo "Testing [functionality]..."

# Test code here
if [condition]; then
    echo "PASS: [success message]"
else
    echo "ERROR: [failure message]"
    exit 1
fi

echo "PASS: [Test name] completed"
exit 0
```

### Adding Tests

1. Choose appropriate category in `tests/`
2. Follow naming convention: `NN-descriptive-name.sh`
3. Make executable: `chmod +x tests/category/NN-test.sh`
4. Test independently before integrating
5. Update test count in `./test --list` if needed

## Questions?

- **What tests exist?** - `./test --list`
- **How to run them?** - `./test`
- **Specific subsystem?** - `./test <name>` (e.g., `./test rrd`)
- **Tests stuck?** - `./test --clean`
- **Need help?** - `./test --help`
