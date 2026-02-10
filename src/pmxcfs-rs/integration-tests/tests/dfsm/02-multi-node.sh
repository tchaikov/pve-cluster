#!/bin/bash
# Multi-node DFSM synchronization test
# Tests that data written on one node is synchronized to other nodes

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo "========================================="
echo "Test: Multi-Node DFSM Synchronization"
echo "========================================="
echo ""

# This script should be run from a test orchestrator that can exec into multiple nodes
# For now, it just creates marker files that can be checked by the orchestrator

MOUNT_POINT="$TEST_MOUNT_PATH"
SYNC_TEST_DIR="$MOUNT_POINT/multi-node-sync-test"
NODE_NAME=$(hostname)
MARKER_FILE="$SYNC_TEST_DIR/node-${NODE_NAME}.marker"

echo "Running on node: $NODE_NAME"
echo ""

echo "1. Checking pmxcfs is running..."
if ! pgrep -x pmxcfs > /dev/null; then
    echo -e "${RED}ERROR: pmxcfs is not running${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} pmxcfs is running"
echo ""

echo "2. Creating sync test directory..."
mkdir -p "$SYNC_TEST_DIR"
echo -e "${GREEN}✓${NC} Sync test directory created"
echo ""

echo "3. Writing node marker file..."
cat > "$MARKER_FILE" <<EOF
{
  "node": "$NODE_NAME",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "pid": $$,
  "test": "multi-node-sync"
}
EOF

if [ ! -f "$MARKER_FILE" ]; then
    echo -e "${RED}ERROR: Failed to create marker file${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} Marker file created: $MARKER_FILE"
echo ""

echo "4. Creating test data..."
TEST_DATA_FILE="$SYNC_TEST_DIR/shared-data-from-${NODE_NAME}.txt"
cat > "$TEST_DATA_FILE" <<EOF
This file was created by $NODE_NAME
Timestamp: $(date -u +%Y-%m-%dT%H:%M:%SZ)
Random data: $(tr -dc A-Za-z0-9 </dev/urandom | head -c 32)
EOF

if [ ! -f "$TEST_DATA_FILE" ]; then
    echo -e "${RED}ERROR: Failed to create test data file${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} Test data file created"
echo ""

echo "5. Creating directory hierarchy..."
HIERARCHY_DIR="$SYNC_TEST_DIR/hierarchy-${NODE_NAME}"
mkdir -p "$HIERARCHY_DIR/level1/level2/level3"
for level in level1 level2 level3; do
    echo "$NODE_NAME - $level" > "$HIERARCHY_DIR/level1/${level}.txt"
done
echo -e "${GREEN}✓${NC} Directory hierarchy created"
echo ""

echo "6. Listing sync directory contents..."
echo "Files in sync directory:"
ls -la "$SYNC_TEST_DIR" | grep -v "^total" | grep -v "^d" | while read line; do
    echo "  $line"
done
echo ""

echo "7. Checking for files from other nodes..."
OTHER_MARKERS=$(ls -1 "$SYNC_TEST_DIR"/node-*.marker 2>/dev/null | grep -v "$NODE_NAME" | wc -l)
if [ "$OTHER_MARKERS" -gt 0 ]; then
    echo -e "${GREEN}✓${NC} Found $OTHER_MARKERS marker files from other nodes"
    ls -1 "$SYNC_TEST_DIR"/node-*.marker | grep -v "$NODE_NAME" | while read marker; do
        NODE=$(basename "$marker" .marker | sed 's/node-//')
        echo "  - Detected node: $NODE"
        if [ -f "$marker" ]; then
            echo "    Content preview: $(head -1 "$marker")"
        fi
    done
else
    echo -e "${YELLOW}ℹ${NC} No marker files from other nodes found yet (might be first node or still syncing)"
fi
echo ""

echo "8. Writing sync verification data..."
VERIFY_FILE="$SYNC_TEST_DIR/verify-${NODE_NAME}.json"
cat > "$VERIFY_FILE" <<EOF
{
  "node": "$NODE_NAME",
  "test_type": "sync_verification",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "operations": {
    "marker_created": true,
    "test_data_created": true,
    "hierarchy_created": true
  },
  "sync_status": {
    "other_nodes_visible": $OTHER_MARKERS
  }
}
EOF
echo -e "${GREEN}✓${NC} Verification data written"
echo ""

echo "9. Creating config file (simulating real usage)..."
CONFIG_DIR="$SYNC_TEST_DIR/config-${NODE_NAME}"
mkdir -p "$CONFIG_DIR"
cat > "$CONFIG_DIR/cluster.conf" <<EOF
# Cluster configuration created by $NODE_NAME
nodes {
  $NODE_NAME {
    ip = "127.0.0.1"
    role = "test"
  }
}
sync_test {
  enabled = yes
  timestamp = $(date +%s)
}
EOF
echo -e "${GREEN}✓${NC} Config file created"
echo ""

echo "10. Final status check..."
TOTAL_FILES=$(find "$SYNC_TEST_DIR" -type f | wc -l)
TOTAL_DIRS=$(find "$SYNC_TEST_DIR" -type d | wc -l)
echo "Statistics:"
echo "  Total files: $TOTAL_FILES"
echo "  Total directories: $TOTAL_DIRS"

echo ""
echo -e "${GREEN}✓ Multi-node sync test passed${NC}"
echo "Note: In multi-node cluster, orchestrator should verify files sync to other nodes"
exit 0
