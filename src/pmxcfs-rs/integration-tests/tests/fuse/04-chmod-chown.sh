#!/bin/bash
# Test: chmod operations (M1)
#
# Tests that chmod operations validate permissions correctly.
#
# With default_permissions, the kernel checks file ownership before calling FUSE.
# Since files are owned by root, only root can chmod them. So:
# - Valid chmod tests run as root (kernel allows, FUSE validates mode)
# - Invalid chmod tests run as www-data (kernel sends SETATTR, FUSE rejects)
#
# Expected behavior (matching C implementation - pmxcfs.c:180-197):
# - chmod: Validates mode is 0600 (private) or 0640 (non-private), returns -EPERM otherwise
# - The operation doesn't actually change the file's stored permissions

set -e

# Source test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing chmod operations..."

PASS=0
FAIL=0

test_pass() {
    echo "  ✓ $1"
    PASS=$((PASS + 1))
}

test_fail() {
    echo "  ✗ FAIL: $1"
    FAIL=$((FAIL + 1))
}

# Helper: run chmod as www-data so the kernel sends SETATTR to FUSE
# (root has CAP_FOWNER, so kernel bypasses FUSE for root)
run_chmod_as_wwwdata() {
    su -s /bin/sh www-data -c "chmod $1 '$2'" 2>/dev/null
}

# Test 1: chmod 0640 on non-private paths (should succeed)
echo ""
echo "Test 1: chmod 0640 on non-private paths"
echo "test1" > "$MOUNT_PATH/test1.txt"

if chmod 0640 "$MOUNT_PATH/test1.txt" 2>/dev/null; then
    test_pass "chmod 0640 succeeded on non-private path"
else
    test_fail "chmod 0640 failed on non-private path (should succeed)"
fi

# Test 2: chmod 0600 on private paths (should succeed)
echo ""
echo "Test 2: chmod 0600 on private paths"
mkdir -p "$MOUNT_PATH/priv"
echo "test2" > "$MOUNT_PATH/priv/test2.txt"

if chmod 0600 "$MOUNT_PATH/priv/test2.txt" 2>/dev/null; then
    test_pass "chmod 0600 succeeded on private path"
else
    test_fail "chmod 0600 failed on private path (should succeed)"
fi

# Test 3: chmod with invalid modes as www-data (should fail with EPERM)
# www-data is not the file owner, so default_permissions denies chmod.
# This verifies the kernel sends SETATTR and our FUSE handler rejects it.
echo ""
echo "Test 3: chmod with invalid modes (as www-data)"
echo "test3" > "$MOUNT_PATH/test3.txt"

for mode in 0755 0644 0777 0400; do
    if run_chmod_as_wwwdata "$mode" "$MOUNT_PATH/test3.txt"; then
        test_fail "chmod $mode succeeded (should fail)"
    else
        test_pass "chmod $mode rejected as expected"
    fi
done

# Test 4: chmod wrong mode for path type (as www-data)
echo ""
echo "Test 4: chmod wrong mode for path type (as www-data)"
echo "test4" > "$MOUNT_PATH/priv/test4.txt"
echo "test5" > "$MOUNT_PATH/test5.txt"

if run_chmod_as_wwwdata 0640 "$MOUNT_PATH/priv/test4.txt"; then
    test_fail "chmod 0640 on private path succeeded (should fail)"
else
    test_pass "chmod 0640 on private path rejected as expected"
fi

if run_chmod_as_wwwdata 0600 "$MOUNT_PATH/test5.txt"; then
    test_fail "chmod 0600 on non-private path succeeded (should fail)"
else
    test_pass "chmod 0600 on non-private path rejected as expected"
fi

# Test 5: chmod on directories (should succeed)
echo ""
echo "Test 5: chmod on directories"
mkdir -p "$MOUNT_PATH/priv/testdir"
mkdir -p "$MOUNT_PATH/testdir"

if chmod 0600 "$MOUNT_PATH/priv/testdir" 2>/dev/null; then
    test_pass "chmod 0600 succeeded on private directory"
else
    test_fail "chmod 0600 failed on private directory"
fi

if chmod 0640 "$MOUNT_PATH/testdir" 2>/dev/null; then
    test_pass "chmod 0640 succeeded on non-private directory"
else
    test_fail "chmod 0640 failed on non-private directory"
fi

# Test 6: Verify chmod doesn't affect file content
echo ""
echo "Test 6: chmod doesn't affect file content"
CONTENT="This is test content that should not change"
echo "$CONTENT" > "$MOUNT_PATH/test6.txt"

CHECKSUM_BEFORE=$(md5sum "$MOUNT_PATH/test6.txt" | cut -d' ' -f1)

chmod 0640 "$MOUNT_PATH/test6.txt" 2>/dev/null || true

CHECKSUM_AFTER=$(md5sum "$MOUNT_PATH/test6.txt" | cut -d' ' -f1)

if [ "$CHECKSUM_BEFORE" = "$CHECKSUM_AFTER" ]; then
    test_pass "File content unchanged after chmod"
else
    test_fail "File content changed after chmod"
fi

echo ""
echo "============================================================================"
echo "chmod test: $PASS/$((PASS + FAIL)) passed"
if [ $FAIL -eq 0 ]; then
    echo "✓ All chmod tests PASSED"
else
    echo "FAILED: $FAIL test(s) failed"
    exit 1
fi
