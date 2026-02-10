#!/bin/bash
# Test: Mixed Cluster File Synchronization
# Test file sync between Rust and C pmxcfs nodes

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing file synchronization in mixed cluster..."

# Check if we're in multi-node environment
if [ -z "$NODE1_IP" ] || [ -z "$NODE2_IP" ] || [ -z "$NODE3_IP" ]; then
    echo "ERROR: Node IP environment variables not set"
    echo "This test requires multi-node setup with NODE1_IP, NODE2_IP, NODE3_IP"
    exit 1
fi

echo "Mixed cluster environment:"
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

# Helper function to create file on a node
create_file_on_node() {
    local container_name=$1
    local file_path=$2
    local content=$3
    local node_name=$4

    echo "Creating file on $node_name ($container_name)..."
    echo "  Path: $file_path"

    if $CONTAINER_CMD exec $container_name bash -c "echo '$content' > $file_path" 2>/dev/null; then
        echo "  ✓ File created"
        return 0
    else
        echo "  ✗ Failed to create file"
        return 1
    fi
}

# Helper function to check file on a node
check_file_on_node() {
    local container_name=$1
    local file_path=$2
    local expected_content=$3
    local node_name=$4

    echo "Checking file on $node_name ($container_name)..."

    if ! $CONTAINER_CMD exec $container_name test -f $file_path 2>/dev/null; then
        echo "  ✗ File not found: $file_path"
        return 1
    fi

    local content=$($CONTAINER_CMD exec $container_name cat $file_path 2>/dev/null || echo "")

    if [ "$content" = "$expected_content" ]; then
        echo "  ✓ File found with correct content"
        return 0
    else
        echo "  ⚠ File found but content differs"
        echo "    Expected: '$expected_content'"
        echo "    Got:      '$content'"
        return 1
    fi
}

# Helper function to remove file on a node
remove_file_on_node() {
    local container_name=$1
    local file_path=$2
    local node_name=$3

    $CONTAINER_CMD exec $container_name rm -f $file_path 2>/dev/null || true
}

# Test 1: Rust → Rust sync
echo "━━━ Test 1: File sync from Rust (node1) to Rust (node2) ━━━"
TEST_FILE_1="/test/pve/mixed-sync-rust-to-rust-$(date +%s).txt"
TEST_CONTENT_1="Rust to Rust sync test"

create_file_on_node "pmxcfs-mixed-node1" "$TEST_FILE_1" "$TEST_CONTENT_1" "node1" || exit 1

echo "Waiting for cluster sync (5s)..."
sleep 5

if check_file_on_node "pmxcfs-mixed-node2" "$TEST_FILE_1" "$TEST_CONTENT_1" "node2"; then
    echo "✓ Rust → Rust sync works"
else
    echo "✗ Rust → Rust sync failed"
    exit 1
fi

# Cleanup
remove_file_on_node "pmxcfs-mixed-node1" "$TEST_FILE_1" "node1"
echo ""

# Test 2: Rust → C sync
echo "━━━ Test 2: File sync from Rust (node1) to C (node3) ━━━"
TEST_FILE_2="/test/pve/mixed-sync-rust-to-c-$(date +%s).txt"
TEST_CONTENT_2="Rust to C sync test"
# C pmxcfs uses /etc/pve as mount point
C_TEST_FILE_2="/etc/pve/mixed-sync-rust-to-c-$(date +%s).txt"

# Use the same relative path but different mount points
RELATIVE_PATH="mixed-sync-rust-to-c-$(date +%s).txt"
create_file_on_node "pmxcfs-mixed-node1" "/test/pve/$RELATIVE_PATH" "$TEST_CONTENT_2" "node1" || exit 1

echo "Waiting for cluster sync (5s)..."
sleep 5

if check_file_on_node "pmxcfs-mixed-node3" "/etc/pve/$RELATIVE_PATH" "$TEST_CONTENT_2" "node3"; then
    echo "✓ Rust → C sync works"
else
    echo "✗ Rust → C sync failed"
    exit 1
fi

# Cleanup
remove_file_on_node "pmxcfs-mixed-node1" "/test/pve/$RELATIVE_PATH" "node1"
remove_file_on_node "pmxcfs-mixed-node3" "/etc/pve/$RELATIVE_PATH" "node3"
echo ""

# Test 3: C → Rust sync
echo "━━━ Test 3: File sync from C (node3) to Rust (node1) ━━━"
RELATIVE_PATH_3="mixed-sync-c-to-rust-$(date +%s).txt"
TEST_CONTENT_3="C to Rust sync test"

create_file_on_node "pmxcfs-mixed-node3" "/etc/pve/$RELATIVE_PATH_3" "$TEST_CONTENT_3" "node3" || exit 1

echo "Waiting for cluster sync (5s)..."
sleep 5

if check_file_on_node "pmxcfs-mixed-node1" "/test/pve/$RELATIVE_PATH_3" "$TEST_CONTENT_3" "node1"; then
    echo "✓ C → Rust sync works"
else
    echo "✗ C → Rust sync failed"
    exit 1
fi

# Also verify it reached node2
if check_file_on_node "pmxcfs-mixed-node2" "/test/pve/$RELATIVE_PATH_3" "$TEST_CONTENT_3" "node2"; then
    echo "✓ C → Rust sync propagated to all Rust nodes"
else
    echo "⚠ C → Rust sync didn't reach node2"
fi

# Cleanup
remove_file_on_node "pmxcfs-mixed-node3" "/etc/pve/$RELATIVE_PATH_3" "node3"
remove_file_on_node "pmxcfs-mixed-node1" "/test/pve/$RELATIVE_PATH_3" "node1"
remove_file_on_node "pmxcfs-mixed-node2" "/test/pve/$RELATIVE_PATH_3" "node2"
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✓ All mixed cluster file sync tests PASSED"
echo ""
echo "Summary:"
echo "  ✓ Rust → Rust synchronization works"
echo "  ✓ Rust → C synchronization works"
echo "  ✓ C → Rust synchronization works"
echo ""
echo "Mixed cluster file synchronization is functioning correctly!"
exit 0
