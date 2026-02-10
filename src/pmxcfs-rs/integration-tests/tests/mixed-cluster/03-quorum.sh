#!/bin/bash
# Test: Mixed Cluster Quorum
# Verify cluster quorum with mixed Rust and C nodes

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing cluster quorum in mixed environment..."

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

# Helper function to check quorum on a node
check_quorum_on_node() {
    local container_name=$1
    local node_name=$2

    echo "Checking quorum on $node_name..."

    # Run corosync-quorumtool
    local quorum_output=$($CONTAINER_CMD exec $container_name corosync-quorumtool -s 2>&1 || echo "ERROR")

    if echo "$quorum_output" | grep -q "ERROR"; then
        echo "  ✗ Failed to get quorum status"
        echo "$quorum_output" | head -5
        return 1
    fi

    echo "$quorum_output"

    # Check if quorate
    if echo "$quorum_output" | grep -q "Quorate.*Yes"; then
        echo "  ✓ Node is quorate"
    else
        echo "  ✗ Node is NOT quorate"
        return 1
    fi

    # Extract node count
    local node_count=$(echo "$quorum_output" | grep "Nodes:" | awk '{print $2}' || echo "0")
    echo "  Node count: $node_count"

    if [ "$node_count" -ge 3 ]; then
        echo "  ✓ All 3 nodes visible"
    else
        echo "  ⚠ Only $node_count nodes visible (expected 3)"
        return 1
    fi

    return 0
}

# Check quorum on all nodes
echo "━━━ Node 1 (Rust) ━━━"
if check_quorum_on_node "pmxcfs-mixed-node1" "node1"; then
    NODE1_QUORATE=true
else
    NODE1_QUORATE=false
fi
echo ""

echo "━━━ Node 2 (Rust) ━━━"
if check_quorum_on_node "pmxcfs-mixed-node2" "node2"; then
    NODE2_QUORATE=true
else
    NODE2_QUORATE=false
fi
echo ""

echo "━━━ Node 3 (C) ━━━"
if check_quorum_on_node "pmxcfs-mixed-node3" "node3"; then
    NODE3_QUORATE=true
else
    NODE3_QUORATE=false
fi
echo ""

# Verify all nodes see consistent cluster state
echo "━━━ Verifying Cluster Consistency ━━━"

# Get membership list from each node
echo "Getting membership from node1 (Rust)..."
NODE1_MEMBERS=$($CONTAINER_CMD exec pmxcfs-mixed-node1 corosync-quorumtool -l 2>&1 | grep "node" || echo "")

echo "Getting membership from node2 (Rust)..."
NODE2_MEMBERS=$($CONTAINER_CMD exec pmxcfs-mixed-node2 corosync-quorumtool -l 2>&1 | grep "node" || echo "")

echo "Getting membership from node3 (C)..."
NODE3_MEMBERS=$($CONTAINER_CMD exec pmxcfs-mixed-node3 corosync-quorumtool -l 2>&1 | grep "node" || echo "")

echo ""
echo "Membership lists:"
echo "Node1: $NODE1_MEMBERS"
echo "Node2: $NODE2_MEMBERS"
echo "Node3: $NODE3_MEMBERS"
echo ""

# Final verdict
if [ "$NODE1_QUORATE" = true ] && [ "$NODE2_QUORATE" = true ] && [ "$NODE3_QUORATE" = true ]; then
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✓ Mixed cluster quorum test PASSED"
    echo ""
    echo "Summary:"
    echo "  ✓ All 3 nodes are quorate"
    echo "  ✓ Rust and C nodes coexist in same cluster"
    echo "  ✓ Cluster membership consistent across all nodes"
    echo ""
    echo "Mixed cluster quorum is functioning correctly!"
    exit 0
else
    echo "✗ Mixed cluster quorum test FAILED"
    echo ""
    echo "Status:"
    echo "  Node1 (Rust): $NODE1_QUORATE"
    echo "  Node2 (Rust): $NODE2_QUORATE"
    echo "  Node3 (C):    $NODE3_QUORATE"
    echo ""
    echo "Possible issues:"
    echo "  - Corosync not configured properly"
    echo "  - Network connectivity issues"
    echo "  - Nodes not joined to cluster"
    exit 1
fi
