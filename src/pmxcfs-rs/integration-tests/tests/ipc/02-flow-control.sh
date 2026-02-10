#!/bin/bash
# Test: IPC Flow Control
# Verify workqueue handles concurrent requests without deadlock

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing IPC flow control mechanism..."

# Verify pmxcfs is running
if ! pgrep -x pmxcfs > /dev/null; then
    echo "ERROR: pmxcfs is not running"
    exit 1
fi
echo "✓ pmxcfs is running"

# Verify IPC socket exists
if ! grep -q "@pve2" /proc/net/unix 2>/dev/null; then
    echo "ERROR: IPC socket not found"
    exit 1
fi
echo "✓ IPC socket exists"

# Test concurrent file operations to potentially fill the workqueue
MOUNT_DIR="$TEST_MOUNT_PATH"
TEST_DIR="$MOUNT_DIR/test-flow-control-$$"

echo "✓ Performing rapid file operations to test workqueue"

# Create test directory
mkdir -p "$TEST_DIR" || {
    echo "ERROR: Failed to create test directory"
    exit 1
}

# Perform 20 rapid file operations concurrently
# The workqueue has capacity 8, so this tests backpressure handling
echo "  Creating 20 test files concurrently..."
for i in {1..20}; do
    echo "test-data-$i" > "$TEST_DIR/file-$i.txt" &
done
wait

# Verify all files were created successfully
FILE_COUNT=$(find "$TEST_DIR" -type f -name "file-*.txt" 2>/dev/null | wc -l)
if [ "$FILE_COUNT" -ne 20 ]; then
    echo "ERROR: Expected 20 files, found $FILE_COUNT"
    echo "  Flow control may have caused failures"
    exit 1
fi
echo "✓ All 20 files created successfully"

# Read back all files rapidly to verify integrity
echo "  Reading 20 test files concurrently..."
for i in {1..20}; do
    cat "$TEST_DIR/file-$i.txt" > /dev/null &
done
wait
echo "✓ All files readable"

# Verify data integrity
echo "  Verifying data integrity..."
CORRUPT_COUNT=0
for i in {1..20}; do
    CONTENT=$(cat "$TEST_DIR/file-$i.txt" 2>/dev/null || echo "ERROR")
    if [ "$CONTENT" != "test-data-$i" ]; then
        CORRUPT_COUNT=$((CORRUPT_COUNT + 1))
        echo "  ERROR: File $i corrupted: expected 'test-data-$i', got '$CONTENT'"
    fi
done

if [ "$CORRUPT_COUNT" -gt 0 ]; then
    echo "ERROR: Found $CORRUPT_COUNT corrupted files"
    exit 1
fi
echo "✓ All files have correct content"

# Cleanup
rm -rf "$TEST_DIR"

echo "✓ Flow control mechanism test completed"
echo "  • Workqueue handled 20 concurrent operations"
echo "  • No deadlock occurred"
echo "  • Data integrity maintained"

exit 0
