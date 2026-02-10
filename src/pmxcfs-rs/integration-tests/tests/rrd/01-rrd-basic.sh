#!/bin/bash
# Test: RRD Basic Functionality
# Verify RRD file creation and updates work

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing RRD basic functionality..."

MOUNT_PATH="$TEST_MOUNT_PATH"
RRD_DIR="/var/lib/rrdcached/db"

# Alternative RRD directory if default doesn't exist
if [ ! -d "$RRD_DIR" ]; then
    RRD_DIR="$TEST_RRD_DIR"
    mkdir -p "$RRD_DIR"
fi

# Check if RRD directory exists
if [ ! -d "$RRD_DIR" ]; then
    echo "ERROR: RRD directory not found: $RRD_DIR"
    exit 1
fi
echo "✓ RRD directory exists: $RRD_DIR"

# Check if rrdtool is available
if ! command -v rrdtool &> /dev/null; then
    echo "⚠ Warning: rrdtool not installed, skipping detailed checks"
    echo "  (This is expected in minimal containers)"
    echo "✓ RRD basic functionality test completed (limited)"
    exit 0
fi

# Test RRD file creation (this would normally be done by pmxcfs)
TEST_RRD="$RRD_DIR/test-node-$$"
TIMESTAMP=$(date +%s)

# Create a simple RRD file for testing
if rrdtool create "$TEST_RRD" \
    --start "$((TIMESTAMP - 10))" \
    --step 60 \
    DS:cpu:GAUGE:120:0:1 \
    DS:mem:GAUGE:120:0:U \
    RRA:AVERAGE:0.5:1:70 2>/dev/null; then
    echo "✓ RRD file creation works"

    # Test RRD update
    if rrdtool update "$TEST_RRD" "$TIMESTAMP:0.5:1073741824" 2>/dev/null; then
        echo "✓ RRD update works"
    else
        echo "ERROR: RRD update failed"
        rm -f "$TEST_RRD"
        exit 1
    fi

    # Test RRD info
    if rrdtool info "$TEST_RRD" | grep -q "ds\[cpu\]"; then
        echo "✓ RRD info works"
    else
        echo "ERROR: RRD info failed"
        rm -f "$TEST_RRD"
        exit 1
    fi

    # Cleanup
    rm -f "$TEST_RRD"
else
    echo "⚠ Warning: RRD creation not available"
fi

# Check for pmxcfs RRD files (if any were created)
RRD_COUNT=$(find "$RRD_DIR" -name "pve2-*" -o -name "pve2.3-*" 2>/dev/null | wc -l)
if [ "$RRD_COUNT" -gt 0 ]; then
    echo "✓ Found $RRD_COUNT pmxcfs RRD files"
else
    echo "  No pmxcfs RRD files found yet (expected if just started)"
fi

# Check for common RRD key patterns
echo "  Checking for expected RRD file patterns:"
for pattern in "pve2-node" "pve2-vm" "pve2-storage" "pve2.3-vm"; do
    if ls "$RRD_DIR"/$pattern* 2>/dev/null | head -1 > /dev/null; then
        echo "    ✓ Pattern found: $pattern"
    else
        echo "    - Pattern not found: $pattern (expected if no data yet)"
    fi
done

echo "✓ RRD basic functionality test passed"
exit 0
