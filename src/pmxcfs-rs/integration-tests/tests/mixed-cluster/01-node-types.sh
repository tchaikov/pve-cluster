#!/bin/bash
# Test: Mixed Cluster Node Types
# Verify that Rust and C pmxcfs nodes are running correctly

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing mixed cluster node types..."

# Check if we're in multi-node environment
if [ -z "$NODE1_IP" ] || [ -z "$NODE2_IP" ] || [ -z "$NODE3_IP" ]; then
    echo "ERROR: Node IP environment variables not set"
    echo "This test requires multi-node setup with NODE1_IP, NODE2_IP, NODE3_IP"
    exit 1
fi

echo "Mixed cluster environment detected:"
echo "  Node1 (Rust): $NODE1_IP"
echo "  Node2 (Rust): $NODE2_IP"
echo "  Node3 (C):    $NODE3_IP"
echo ""

# Detect container runtime (prefer environment variable for consistency with test runner)
if [ -n "$CONTAINER_CMD" ]; then
    # Use CONTAINER_CMD from environment (set by test runner)
    :
elif command -v podman &> /dev/null; then
    CONTAINER_CMD="podman"
elif command -v docker &> /dev/null; then
    CONTAINER_CMD="docker"
else
    echo "ERROR: No container runtime found (need docker or podman)"
    exit 1
fi

echo "Using container runtime: $CONTAINER_CMD"
echo ""

# Helper function to check pmxcfs binary type on a node
check_node_type() {
    local container_name=$1
    local expected_type=$2
    local node_name=$3

    echo "Checking $node_name ($container_name)..."

    # Check if pmxcfs is running
    if ! $CONTAINER_CMD exec $container_name pgrep pmxcfs > /dev/null 2>&1; then
        echo "  ✗ pmxcfs not running on $node_name"
        return 1
    fi
    echo "  ✓ pmxcfs is running"

    # Get the binary path
    local pmxcfs_pid=$($CONTAINER_CMD exec $container_name pgrep pmxcfs 2>/dev/null | head -1)
    local binary_path=$($CONTAINER_CMD exec $container_name readlink -f /proc/$pmxcfs_pid/exe 2>/dev/null || echo "unknown")

    echo "  Binary: $binary_path"

    # Check if it's the expected type
    if [ "$expected_type" = "rust" ]; then
        if echo "$binary_path" | grep -q "pmxcfs-rs"; then
            echo "  ✓ Running Rust pmxcfs (as expected)"
            return 0
        else
            echo "  ✗ Expected Rust binary but found: $binary_path"
            return 1
        fi
    elif [ "$expected_type" = "c" ]; then
        # C binary would be at /workspace/src/pmxcfs
        if echo "$binary_path" | grep -q "src/pmxcfs" && ! echo "$binary_path" | grep -q "pmxcfs-rs"; then
            echo "  ✓ Running C pmxcfs (as expected)"
            return 0
        else
            echo "  ✗ Expected C binary but found: $binary_path"
            return 1
        fi
    else
        echo "  ✗ Unknown expected type: $expected_type"
        return 1
    fi
}

# Helper function to check FUSE mount on a node
check_fuse_mount() {
    local container_name=$1
    local expected_mount=$2
    local node_name=$3

    echo "Checking FUSE mount on $node_name..."

    # Check if FUSE is mounted
    local mount_output=$($CONTAINER_CMD exec $container_name mount | grep fuse || echo "")

    if [ -z "$mount_output" ]; then
        echo "  ✗ No FUSE mount found on $node_name"
        return 1
    fi

    echo "  ✓ FUSE mounted: $mount_output"

    # Verify the expected mount path exists
    if $CONTAINER_CMD exec $container_name test -d $expected_mount 2>/dev/null; then
        echo "  ✓ Mount path accessible: $expected_mount"
        return 0
    else
        echo "  ✗ Mount path not accessible: $expected_mount"
        return 1
    fi
}

# Test each node
echo "━━━ Node 1 (Rust) ━━━"
check_node_type "pmxcfs-mixed-node1" "rust" "node1" || exit 1
check_fuse_mount "pmxcfs-mixed-node1" "$TEST_MOUNT_PATH" "node1" || exit 1
echo ""

echo "━━━ Node 2 (Rust) ━━━"
check_node_type "pmxcfs-mixed-node2" "rust" "node2" || exit 1
check_fuse_mount "pmxcfs-mixed-node2" "$TEST_MOUNT_PATH" "node2" || exit 1
echo ""

echo "━━━ Node 3 (C) ━━━"
check_node_type "pmxcfs-mixed-node3" "c" "node3" || exit 1
check_fuse_mount "pmxcfs-mixed-node3" "/etc/pve" "node3" || exit 1
echo ""

echo "✓ All nodes running with correct pmxcfs types"
echo "  - Node 1: Rust pmxcfs"
echo "  - Node 2: Rust pmxcfs"
echo "  - Node 3: C pmxcfs"
exit 0
