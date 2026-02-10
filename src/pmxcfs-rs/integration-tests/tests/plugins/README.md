# Plugin Tests

Integration tests for plugin files exposed via FUSE.

## Overview

Plugins are virtual files that appear in the FUSE-mounted filesystem and provide dynamic content. These tests verify plugin files work correctly when accessed through the filesystem.

## Test Files

### `01-plugin-files.sh`
Basic plugin file functionality:
- Verifies plugin files exist in FUSE mount
- Tests file readability
- Validates basic file operations

### `02-clusterlog-plugin.sh`
ClusterLog plugin comprehensive test:
- Validates JSON format and structure
- Checks required fields and types
- Verifies read consistency and concurrent access

### `03-plugin-write.sh`
Plugin write operations:
- Tests write to `.debug` plugin (debug level toggle)
- Verifies write permissions
- Validates read-only plugin enforcement

## Prerequisites

Build the Rust binary:
```bash
cd src/pmxcfs-rs
cargo build --release
```

## Running Tests

```bash
cd integration-tests
./test plugins
```

## External Dependencies

- **FUSE**: Filesystem in userspace (for mounting /etc/pve)
- **jq**: JSON processor (for validating plugin output)

## References

- Main integration tests: `../../README.md`
- Test runner: `../../test`
