#!/bin/bash
# Test: Plugin Write Operations
# Verify that the .debug plugin can be written to through FUSE

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing plugin write operations..."

MOUNT_PATH="$TEST_MOUNT_PATH"

# Check if mount path is accessible
if [ ! -d "$MOUNT_PATH" ]; then
    echo "ERROR: Mount path not accessible: $MOUNT_PATH"
    exit 1
fi
echo "✓ Mount path accessible"

PASSED=0
FAILED=0

# Test 1: Verify .debug plugin exists and is writable
echo ""
echo "Test 1: Verify .debug plugin exists and is writable"
if [ ! -f "$MOUNT_PATH/.debug" ]; then
    echo "  ✗ .debug plugin file does not exist"
    FAILED=$((FAILED + 1))
else
    echo "  ✓ .debug plugin file exists"
    PASSED=$((PASSED + 1))
fi

# Check permissions (should be 0o640 = rw-r-----)
PERMS=$(stat -c "%a" "$MOUNT_PATH/.debug" 2>/dev/null || echo "000")
if [ "$PERMS" != "640" ]; then
    echo "  ⚠ .debug has unexpected permissions: $PERMS (expected 640)"
else
    echo "  ✓ .debug has correct permissions: 640"
    PASSED=$((PASSED + 1))
fi

# Test 2: Read initial debug level
echo ""
echo "Test 2: Read initial debug level"
INITIAL_LEVEL=$(cat "$MOUNT_PATH/.debug" 2>/dev/null)
if [ -z "$INITIAL_LEVEL" ]; then
    echo "  ✗ Could not read .debug file"
    FAILED=$((FAILED + 1))
else
    echo "  ✓ Initial debug level: $INITIAL_LEVEL"
    PASSED=$((PASSED + 1))
fi

# Test 3: Write new debug level
echo ""
echo "Test 3: Write new debug level"
echo "1" > "$MOUNT_PATH/.debug" 2>/dev/null
if [ $? -ne 0 ]; then
    echo "  ✗ Failed to write to .debug plugin"
    FAILED=$((FAILED + 1))
else
    echo "  ✓ Successfully wrote to .debug plugin"
    PASSED=$((PASSED + 1))
fi

# Test 4: Verify the write took effect
echo ""
echo "Test 4: Verify the write took effect"
NEW_LEVEL=$(cat "$MOUNT_PATH/.debug" 2>/dev/null)
if [ "$NEW_LEVEL" != "1" ]; then
    echo "  ✗ Debug level did not change (got: $NEW_LEVEL, expected: 1)"
    FAILED=$((FAILED + 1))
else
    echo "  ✓ Debug level changed to: $NEW_LEVEL"
    PASSED=$((PASSED + 1))
fi

# Test 5: Test writing different values
echo ""
echo "Test 5: Test writing different values"
ALL_OK=1
for level in 0 2 3 1; do
    echo "$level" > "$MOUNT_PATH/.debug" 2>/dev/null
    CURRENT=$(cat "$MOUNT_PATH/.debug" 2>/dev/null)
    if [ "$CURRENT" != "$level" ]; then
        echo "  ✗ Failed to set debug level to $level (got: $CURRENT)"
        ALL_OK=0
    fi
done
if [ $ALL_OK -eq 1 ]; then
    echo "  ✓ Successfully set multiple debug levels (0, 2, 3, 1)"
    PASSED=$((PASSED + 1))
else
    FAILED=$((FAILED + 1))
fi

# Test 6: Verify read-only plugins cannot be written
echo ""
echo "Test 6: Verify read-only plugins reject writes"
# Temporarily disable exit-on-error for write tests that are expected to fail
set +e
echo "test" > "$MOUNT_PATH/.version" 2>/dev/null
if [ $? -eq 0 ]; then
    echo "  ✗ .version plugin incorrectly allowed write"
    FAILED=$((FAILED + 1))
else
    echo "  ✓ Read-only .version plugin correctly rejected write"
    PASSED=$((PASSED + 1))
fi

echo "test" > "$MOUNT_PATH/.members" 2>/dev/null
if [ $? -eq 0 ]; then
    echo "  ✗ .members plugin incorrectly allowed write"
    FAILED=$((FAILED + 1))
else
    echo "  ✓ Read-only .members plugin correctly rejected write"
    PASSED=$((PASSED + 1))
fi
set -e

# Test 7: Verify plugin write persists across reads
echo ""
echo "Test 7: Verify plugin write persists across reads"
echo "2" > "$MOUNT_PATH/.debug" 2>/dev/null
PERSIST_OK=1
for i in {1..5}; do
    LEVEL=$(cat "$MOUNT_PATH/.debug" 2>/dev/null)
    if [ "$LEVEL" != "2" ]; then
        echo "  ✗ Debug level not persistent (iteration $i: got $LEVEL, expected 2)"
        PERSIST_OK=0
        break
    fi
done
if [ $PERSIST_OK -eq 1 ]; then
    echo "  ✓ Plugin write persists across multiple reads"
    PASSED=$((PASSED + 1))
else
    FAILED=$((FAILED + 1))
fi

# Test 8: Test write with newline handling
echo ""
echo "Test 8: Test write with newline handling"
echo -n "3" > "$MOUNT_PATH/.debug" 2>/dev/null  # No newline
LEVEL=$(cat "$MOUNT_PATH/.debug" 2>/dev/null)
if [ "$LEVEL" != "3" ]; then
    echo "  ✗ Failed to write without newline (got: $LEVEL, expected: 3)"
    FAILED=$((FAILED + 1))
else
    echo "  ✓ Write without newline works correctly"
    PASSED=$((PASSED + 1))
fi

echo "4" > "$MOUNT_PATH/.debug" 2>/dev/null  # With newline
LEVEL=$(cat "$MOUNT_PATH/.debug" 2>/dev/null)
if [ "$LEVEL" != "4" ]; then
    echo "  ✗ Failed to write with newline (got: $LEVEL, expected: 4)"
    FAILED=$((FAILED + 1))
else
    echo "  ✓ Write with newline works correctly"
    PASSED=$((PASSED + 1))
fi

# Test 9: Restore initial debug level
echo ""
echo "Test 9: Restore initial debug level"
echo "$INITIAL_LEVEL" > "$MOUNT_PATH/.debug" 2>/dev/null
FINAL_LEVEL=$(cat "$MOUNT_PATH/.debug" 2>/dev/null)
if [ "$FINAL_LEVEL" != "$INITIAL_LEVEL" ]; then
    echo "  ⚠ Could not restore initial debug level (got: $FINAL_LEVEL, expected: $INITIAL_LEVEL)"
else
    echo "  ✓ Restored initial debug level: $INITIAL_LEVEL"
    PASSED=$((PASSED + 1))
fi

# Summary
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Total tests: $((PASSED + FAILED))"
echo "Passed: $PASSED"
echo "Failed: $FAILED"

if [ $FAILED -gt 0 ]; then
    echo ""
    echo "[✗] Some tests FAILED"
    exit 1
else
    echo ""
    echo "[✓] ✓ All tests PASSED"
    exit 0
fi

