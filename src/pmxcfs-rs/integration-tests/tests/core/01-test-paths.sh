#!/bin/bash
# Test: Test Directory Paths
# Verify pmxcfs uses correct test directory paths in container

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing test directory paths..."

# Test directory paths (configurable via test-config.sh)
TEST_PATHS=(
    "$TEST_DB_PATH"
    "$TEST_MOUNT_PATH"
    "$TEST_RUN_DIR"
    "$TEST_SOCKET_PATH"
)

# Check database exists
if [ ! -f "$TEST_DB_PATH" ]; then
    echo "ERROR: Database not found at $TEST_DB_PATH"
    ls -la "$TEST_DB_DIR/" || echo "Directory doesn't exist"
    exit 1
fi
echo "✓ Database: $TEST_DB_PATH"

# Check database is SQLite
if file "$TEST_DB_PATH" | grep -q "SQLite"; then
    echo "✓ Database is SQLite format"
else
    echo "ERROR: Database is not SQLite format"
    file "$TEST_DB_PATH"
    exit 1
fi

# Check mount directory exists (FUSE mount might not be fully accessible in container)
if mountpoint -q "$TEST_MOUNT_PATH" 2>/dev/null || [ -d "$TEST_MOUNT_PATH" ] 2>/dev/null; then
    echo "✓ Mount dir: $TEST_MOUNT_PATH"
else
    echo "⚠ Warning: FUSE mount at $TEST_MOUNT_PATH not accessible (known container limitation)"
fi

# Check runtime directory
if [ ! -d "$TEST_RUN_DIR" ]; then
    echo "ERROR: Runtime directory not found: $TEST_RUN_DIR"
    exit 1
fi
echo "✓ Runtime dir: $TEST_RUN_DIR"

# Check Unix socket (pmxcfs uses abstract sockets like @pve2)
# Abstract sockets don't appear in the filesystem, check /proc/net/unix instead
if grep -q "$TEST_SOCKET" /proc/net/unix 2>/dev/null; then
    echo "✓ Abstract Unix socket: $TEST_SOCKET"
    # Count how many sockets are bound
    SOCKET_COUNT=$(grep -c "$TEST_SOCKET" /proc/net/unix)
    echo "  Socket entries in /proc/net/unix: $SOCKET_COUNT"
else
    echo "ERROR: Abstract Unix socket $TEST_SOCKET not found"
    echo "Checking /proc/net/unix for pve2-related sockets:"
    grep -i pve /proc/net/unix || echo "  No pve-related sockets found"
    exit 1
fi

# Verify corosync config directory
if [ -d "$TEST_COROSYNC_DIR" ]; then
    echo "✓ Corosync config dir: $TEST_COROSYNC_DIR"
else
    echo "⚠ Warning: $TEST_COROSYNC_DIR not found"
fi

echo "✓ All test directory paths correct"
exit 0
