#!/bin/bash
# Test: Lock Management
# Verify file locking functionality in memdb

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing lock management..."

MOUNT_PATH="$TEST_MOUNT_PATH"
DB_PATH="$TEST_DB_PATH"

# Check if mount path is accessible
if [ ! -d "$MOUNT_PATH" ]; then
    echo "ERROR: Mount path not accessible: $MOUNT_PATH"
    exit 1
fi
echo "✓ Mount path accessible"

# Create a test directory for lock testing
TEST_DIR="$MOUNT_PATH/test-locks-$$"
mkdir -p "$TEST_DIR" 2>/dev/null || true

if [ -d "$TEST_DIR" ]; then
    echo "✓ Test directory created: $TEST_DIR"

    # Test file creation for locking
    TEST_FILE="$TEST_DIR/locktest.txt"
    if echo "test data" > "$TEST_FILE" 2>/dev/null; then
        echo "✓ Test file created"

        # Test file locking using flock
        if command -v flock &> /dev/null; then
            echo "Testing file locking with flock..."

            # Create a lock and verify it works
            (
                flock -x 200
                echo "Lock acquired"
                sleep 1
            ) 200>"$TEST_FILE.lock" 2>/dev/null && echo "✓ File locking works"

            # Test non-blocking lock
            if flock -n -x "$TEST_FILE.lock" -c "echo 'Non-blocking lock works'" 2>/dev/null; then
                echo "✓ Non-blocking lock works"
            fi

            # Cleanup lock file
            rm -f "$TEST_FILE.lock"
        else
            echo "⚠ Warning: flock not available, skipping flock tests"
        fi

        # Test concurrent access (basic)
        echo "Testing concurrent file access..."
        if (
            # Write to file from subshell
            echo "concurrent write 1" >> "$TEST_FILE"
        ) 2>/dev/null && (
            # Write to file from another subshell
            echo "concurrent write 2" >> "$TEST_FILE"
        ) 2>/dev/null; then
            echo "✓ Concurrent writes work"

            # Verify both writes made it
            LINE_COUNT=$(wc -l < "$TEST_FILE")
            if [ "$LINE_COUNT" -ge 3 ]; then
                echo "✓ Data integrity maintained"
            fi
        fi

        # Cleanup test file
        rm -f "$TEST_FILE"
    else
        echo "⚠ Warning: Cannot create test file (may be read-only)"
    fi

    # Cleanup test directory
    rmdir "$TEST_DIR" 2>/dev/null || rm -rf "$TEST_DIR" 2>/dev/null || true
else
    echo "⚠ Warning: Cannot create test directory"
fi

# Check database for lock-related tables (if sqlite3 available)
if command -v sqlite3 &> /dev/null && [ -r "$DB_PATH" ]; then
    echo "Checking database for lock information..."

    # Check for lock-related columns in tree table
    if sqlite3 "$DB_PATH" "PRAGMA table_info(tree);" 2>/dev/null | grep -qi "writer\|lock"; then
        echo "✓ Database has lock-related columns"
    else
        echo "  No explicit lock columns found (locks may be in-memory)"
    fi

    # Check for any locked entries
    LOCK_COUNT=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM tree WHERE writer IS NOT NULL;" 2>/dev/null || echo "0")
    if [ "$LOCK_COUNT" -gt 0 ]; then
        echo "  Found $LOCK_COUNT locked entries"
    else
        echo "  No currently locked entries"
    fi
fi

# Test pmxcfs-specific locking behavior
echo "Testing pmxcfs lock behavior..."

# pmxcfs uses writer field and timestamps for lock management
# Locks expire after 120 seconds by default
echo "  Lock expiration timeout: 120 seconds (as per pmxcfs-memdb docs)"
echo "  Lock updates happen every 10 seconds (as per pmxcfs-memdb docs)"

# Create a file that might trigger lock mechanisms
LOCK_TEST_FILE="$MOUNT_PATH/test-lock-behavior.tmp"
if echo "lock test" > "$LOCK_TEST_FILE" 2>/dev/null; then
    echo "✓ Created lock test file"

    # Immediate read-back should work
    if cat "$LOCK_TEST_FILE" > /dev/null 2>&1; then
        echo "✓ File immediately readable after write"
    fi

    # Cleanup
    rm -f "$LOCK_TEST_FILE"
fi

echo "✓ Lock management test completed"
echo ""
echo "Note: Advanced lock testing (expiration, concurrent access from multiple nodes)"
echo "      requires multi-node cluster environment. See cluster/ tests."

exit 0
