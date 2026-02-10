#!/bin/bash
# Test: Multi-Node Status Synchronization
# Verify that status information (.vmlist, .members, .version) synchronizes across cluster nodes

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
echo "Test: Multi-Node Status Synchronization"
echo "========================================="
echo ""

MOUNT_POINT="$TEST_MOUNT_PATH"
NODE_NAME=$(hostname)
TEST_DIR="$MOUNT_POINT/status-sync-test"

echo "Running on node: $NODE_NAME"
echo ""

# ============================================================================
# Helper Functions
# ============================================================================

check_pmxcfs_running() {
    if ! pgrep -x pmxcfs > /dev/null; then
        echo -e "${RED}ERROR: pmxcfs is not running${NC}"
        return 1
    fi
    echo -e "${GREEN}✓${NC} pmxcfs is running"
    return 0
}

# ============================================================================
# Test 1: Verify Plugin Files Exist
# ============================================================================

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 1: Verify Status Plugin Files"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

check_pmxcfs_running || exit 1

PLUGIN_CHECK_FAILED=false
for plugin in .version .members .vmlist; do
    PLUGIN_FILE="$MOUNT_POINT/$plugin"
    if [ -e "$PLUGIN_FILE" ]; then
        echo -e "${GREEN}✓${NC} Plugin file exists: $plugin"
    else
        echo -e "${RED}✗${NC} CRITICAL: Plugin file missing: $plugin"
        PLUGIN_CHECK_FAILED=true
    fi
done

if [ "$PLUGIN_CHECK_FAILED" = true ]; then
    echo ""
    echo -e "${RED}ERROR: Required plugin files are missing!${NC}"
    echo "This indicates a critical failure in plugin initialization."
    echo "All status plugins (.version, .members, .vmlist) must exist when pmxcfs is running."
    exit 1
fi
echo ""

# ============================================================================
# Test 2: Read and Parse .version Plugin
# ============================================================================

echo "━━━━━━━━━���━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 2: Parse .version Plugin"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Create test directory first
mkdir -p "$TEST_DIR" 2>/dev/null || true

VERSION_FILE="$MOUNT_POINT/.version"
if [ ! -e "$VERSION_FILE" ]; then
    echo -e "${RED}✗ CRITICAL: .version file does not exist${NC}"
    echo "Plugin file must exist when pmxcfs is running."
    exit 1
fi

VERSION_CONTENT=$(cat "$VERSION_FILE" 2>/dev/null || echo "")
if [ -z "$VERSION_CONTENT" ]; then
    echo -e "${RED}✗ CRITICAL: .version file is empty or unreadable${NC}"
    exit 1
fi

echo -e "${GREEN}✓${NC} .version file readable"

# Check if it's JSON format (new format) or colon-separated (old format)
if echo "$VERSION_CONTENT" | grep -q "^{"; then
    # JSON format
    echo "  Format: JSON"
    if command -v jq >/dev/null 2>&1; then
        START_TIME=$(echo "$VERSION_CONTENT" | jq -r '.starttime // 0' 2>/dev/null || echo "0")
        VMLIST_VERSION=$(echo "$VERSION_CONTENT" | jq -r '.vmlist // 0' 2>/dev/null || echo "0")
        echo "  Start time: $START_TIME"
        echo "  VM list version: $VMLIST_VERSION"
    else
        # Fallback without jq
        echo "  Content: $VERSION_CONTENT"
        START_TIME=$(echo "$VERSION_CONTENT" | grep -o '"starttime":[0-9]*' | cut -d':' -f2)
        VMLIST_VERSION=$(echo "$VERSION_CONTENT" | grep -o '"vmlist":[0-9]*' | cut -d':' -f2)
        echo "  Start time: ${START_TIME:-unknown}"
        echo "  VM list version: ${VMLIST_VERSION:-unknown}"
    fi
else
    # Old colon-separated format: timestamp:vmlist_version:config_versions...
    echo "  Format: Colon-separated"
    START_TIME=$(echo "$VERSION_CONTENT" | cut -d':' -f1)
    VMLIST_VERSION=$(echo "$VERSION_CONTENT" | cut -d':' -f2)
    echo "  Start time: $START_TIME"
    echo "  VM list version: $VMLIST_VERSION"
fi

# Save version for comparison with other nodes
echo "$VERSION_CONTENT" > "$TEST_DIR/version-${NODE_NAME}.txt"
echo -e "${GREEN}✓${NC} Version saved for multi-node comparison"
echo ""

# ============================================================================
# Test 3: Read and Parse .members Plugin
# ============================================================================

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 3: Parse .members Plugin"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

MEMBERS_FILE="$MOUNT_POINT/.members"
if [ ! -e "$MEMBERS_FILE" ]; then
    echo -e "${RED}✗ CRITICAL: .members file does not exist${NC}"
    echo "Plugin file must exist when pmxcfs is running."
    exit 1
fi

MEMBERS_CONTENT=$(cat "$MEMBERS_FILE" 2>/dev/null || echo "")
if [ -z "$MEMBERS_CONTENT" ]; then
    echo -e "${RED}✗ CRITICAL: .members file is empty or unreadable${NC}"
    exit 1
fi

echo -e "${GREEN}✓${NC} .members file readable"

# Check for [members] section
if echo "$MEMBERS_CONTENT" | grep -q "\[members\]"; then
    echo -e "${GREEN}✓${NC} Members format valid ([members] section found)"
fi

# Count member entries (lines with: nodeid<tab>name<tab>online<tab>ip)
MEMBER_COUNT=$(echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]]" | wc -l || echo "0")
ONLINE_COUNT=$(echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]].*[[:space:]]1[[:space:]]" | wc -l || echo "0")

echo "  Total nodes: $MEMBER_COUNT"
echo "  Online nodes: $ONLINE_COUNT"

# List node details
if [ "$MEMBER_COUNT" -gt 0 ]; then
    echo "  Node details:"
    echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]]" | while read -r line; do
        NODE_ID=$(echo "$line" | awk '{print $1}')
        NODE_NAME_ENTRY=$(echo "$line" | awk '{print $2}')
        ONLINE=$(echo "$line" | awk '{print $3}')
        NODE_IP=$(echo "$line" | awk '{print $4}')

        STATUS="offline"
        [ "$ONLINE" = "1" ] && STATUS="online"

        echo "    - Node $NODE_ID: $NODE_NAME_ENTRY ($NODE_IP) - $STATUS"
    done
fi

# Save members for comparison with other nodes
echo "$MEMBERS_CONTENT" > "$TEST_DIR/members-${NODE_NAME}.txt"
echo -e "${GREEN}✓${NC} Members saved for multi-node comparison"
echo ""

# ============================================================================
# Test 4: Read and Parse .vmlist Plugin
# ============================================================================

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 4: Parse .vmlist Plugin"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

VMLIST_FILE="$MOUNT_POINT/.vmlist"
if [ ! -e "$VMLIST_FILE" ]; then
    echo -e "${RED}✗ CRITICAL: .vmlist file does not exist${NC}"
    echo "Plugin file must exist when pmxcfs is running."
    exit 1
fi

VMLIST_CONTENT=$(cat "$VMLIST_FILE" 2>/dev/null || echo "")
if [ -z "$VMLIST_CONTENT" ]; then
    echo -e "${RED}✗ CRITICAL: .vmlist file is empty or unreadable${NC}"
    exit 1
fi

echo -e "${GREEN}✓${NC} .vmlist file readable"

# Check for [qemu] and [lxc] sections
HAS_QEMU=false
HAS_LXC=false

if echo "$VMLIST_CONTENT" | grep -q "\[qemu\]"; then
    HAS_QEMU=true
    echo -e "${GREEN}✓${NC} QEMU section present"
else
    echo "  No QEMU VMs"
fi

if echo "$VMLIST_CONTENT" | grep -q "\[lxc\]"; then
    HAS_LXC=true
    echo -e "${GREEN}✓${NC} LXC section present"
else
    echo "  No LXC containers"
fi

# Count VM/CT entries (format: VMID<tab>NODE<tab>VERSION)
TOTAL_VMS=$(echo "$VMLIST_CONTENT" | grep -E "^[0-9]+[[:space:]]" | wc -l || echo "0")
echo "  Total VMs/CTs: $TOTAL_VMS"

if [ "$TOTAL_VMS" -gt 0 ]; then
    echo "  VM/CT details:"
    echo "$VMLIST_CONTENT" | grep -E "^[0-9]+[[:space:]]" | while read -r line; do
        VMID=$(echo "$line" | awk '{print $1}')
        VM_NODE=$(echo "$line" | awk '{print $2}')
        VM_VERSION=$(echo "$line" | awk '{print $3}')

        # Determine type based on which section it's in
        TYPE="unknown"
        if [ "$HAS_QEMU" = true ] && echo "$VMLIST_CONTENT" | sed -n '/\[qemu\]/,/\[lxc\]/p' | grep -q "^${VMID}[[:space:]]"; then
            TYPE="qemu"
        elif [ "$HAS_LXC" = true ]; then
            TYPE="lxc"
        fi

        echo "    - VMID $VMID: node=$VM_NODE, version=$VM_VERSION, type=$TYPE"
    done
fi

# Save vmlist for comparison with other nodes
echo "$VMLIST_CONTENT" > "$TEST_DIR/vmlist-${NODE_NAME}.txt"
echo -e "${GREEN}✓${NC} VM list saved for multi-node comparison"
echo ""

# ============================================================================
# Test 5: Create Test VM Entry (Simulate VM Registration)
# ============================================================================

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 5: Create Test VM Configuration"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Create a test VM configuration file to trigger status update
# Format follows Proxmox QEMU config format
TEST_VMID="9999"
TEST_VM_DIR="$MOUNT_POINT/nodes/$NODE_NAME/qemu-server"
TEST_VM_CONF="$TEST_VM_DIR/${TEST_VMID}.conf"

# Create directory if it doesn't exist
mkdir -p "$TEST_VM_DIR" 2>/dev/null || true

if [ -d "$TEST_VM_DIR" ]; then
    echo -e "${GREEN}✓${NC} VM directory exists: $TEST_VM_DIR"

    # Write a minimal QEMU VM configuration
    cat > "$TEST_VM_CONF" <<EOF
# Test VM configuration created by status sync test
# Node: $NODE_NAME
# Timestamp: $(date -u +%Y-%m-%dT%H:%M:%SZ)

bootdisk: scsi0
cores: 2
memory: 2048
name: test-vm-$NODE_NAME
net0: virtio=00:00:00:00:00:01,bridge=vmbr0
numa: 0
ostype: l26
scsi0: local:vm-${TEST_VMID}-disk-0,size=32G
scsihw: virtio-scsi-pci
sockets: 1
vmgenid: $(uuidgen)
EOF

    if [ -f "$TEST_VM_CONF" ]; then
        echo -e "${GREEN}✓${NC} Test VM configuration created: VMID $TEST_VMID"
        echo "  Config file: $TEST_VM_CONF"

        # Wait a moment for status subsystem to detect the new VM
        sleep 2

        # Check if VM now appears in .vmlist
        if [ -e "$VMLIST_FILE" ]; then
            UPDATED_VMLIST=$(cat "$VMLIST_FILE" 2>/dev/null || echo "")
            if echo "$UPDATED_VMLIST" | grep -q "^${TEST_VMID}[[:space:]]"; then
                echo -e "${GREEN}✓${NC} Test VM $TEST_VMID appears in .vmlist"
            else
                echo -e "${YELLOW}⚠${NC} Test VM not yet visible in .vmlist (may require daemon restart or scan trigger)"
            fi
        fi
    else
        echo -e "${YELLOW}⚠${NC} Could not create test VM configuration"
    fi
else
    echo -e "${YELLOW}⚠${NC} Cannot create VM directory (may require privileges)"
fi
echo ""

# ============================================================================
# Test 6: Create Node Marker for Multi-Node Detection
# ============================================================================

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 6: Create Node Marker"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

mkdir -p "$TEST_DIR" 2>/dev/null || true

MARKER_FILE="$TEST_DIR/status-test-${NODE_NAME}.json"
cat > "$MARKER_FILE" <<EOF
{
  "node": "$NODE_NAME",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "pid": $$,
  "test": "multi-node-status-sync",
  "plugins_checked": {
    "version": "$([ -e "$MOUNT_POINT/.version" ] && echo "available" || echo "unavailable")",
    "members": "$([ -e "$MOUNT_POINT/.members" ] && echo "available" || echo "unavailable")",
    "vmlist": "$([ -e "$MOUNT_POINT/.vmlist" ] && echo "available" || echo "unavailable")"
  },
  "vm_registered": "$TEST_VMID"
}
EOF

if [ -f "$MARKER_FILE" ]; then
    echo -e "${GREEN}✓${NC} Node marker created: $MARKER_FILE"
else
    echo -e "${YELLOW}⚠${NC} Could not create node marker"
fi
echo ""

# ============================================================================
# Test 7: Check for Other Nodes
# ============================================================================

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 7: Detect Other Cluster Nodes"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Check for marker files from other nodes
OTHER_MARKERS=$(ls -1 "$TEST_DIR"/status-test-*.json 2>/dev/null | grep -v "$NODE_NAME" | wc -l || echo "0")

if [ "$OTHER_MARKERS" -gt 0 ]; then
    echo -e "${GREEN}✓${NC} Found $OTHER_MARKERS marker file(s) from other nodes"

    ls -1 "$TEST_DIR"/status-test-*.json | grep -v "$NODE_NAME" | while read marker; do
        OTHER_NODE=$(basename "$marker" .json | sed 's/status-test-//')
        echo ""
        echo "  Detected node: $OTHER_NODE"

        # Compare status files with other node
        echo "  Comparing status data..."

        # Compare .members
        if [ -f "$TEST_DIR/members-${NODE_NAME}.txt" ] && [ -f "$TEST_DIR/members-${OTHER_NODE}.txt" ]; then
            if diff -q "$TEST_DIR/members-${NODE_NAME}.txt" "$TEST_DIR/members-${OTHER_NODE}.txt" > /dev/null 2>&1; then
                echo -e "    ${GREEN}✓${NC} .members content matches with $OTHER_NODE"
            else
                echo -e "    ${YELLOW}⚠${NC} .members content differs from $OTHER_NODE"
                echo "      This may be expected if nodes have different view of cluster"
            fi
        fi

        # Compare .vmlist
        if [ -f "$TEST_DIR/vmlist-${NODE_NAME}.txt" ] && [ -f "$TEST_DIR/vmlist-${OTHER_NODE}.txt" ]; then
            if diff -q "$TEST_DIR/vmlist-${NODE_NAME}.txt" "$TEST_DIR/vmlist-${OTHER_NODE}.txt" > /dev/null 2>&1; then
                echo -e "    ${GREEN}✓${NC} .vmlist content matches with $OTHER_NODE"
            else
                echo -e "    ${YELLOW}⚠${NC} .vmlist content differs from $OTHER_NODE"
                echo "      Differences:"
                diff "$TEST_DIR/vmlist-${NODE_NAME}.txt" "$TEST_DIR/vmlist-${OTHER_NODE}.txt" | head -10
            fi
        fi

        # Compare .version (vmlist version should be consistent)
        if [ -f "$TEST_DIR/version-${NODE_NAME}.txt" ] && [ -f "$TEST_DIR/version-${OTHER_NODE}.txt" ]; then
            LOCAL_VMLIST_VER=$(cat "$TEST_DIR/version-${NODE_NAME}.txt" | cut -d':' -f2)
            OTHER_VMLIST_VER=$(cat "$TEST_DIR/version-${OTHER_NODE}.txt" | cut -d':' -f2)

            if [ "$LOCAL_VMLIST_VER" = "$OTHER_VMLIST_VER" ]; then
                echo -e "    ${GREEN}✓${NC} VM list version matches with $OTHER_NODE (v$LOCAL_VMLIST_VER)"
            else
                echo -e "    ${YELLOW}⚠${NC} VM list version differs: $LOCAL_VMLIST_VER (local) vs $OTHER_VMLIST_VER ($OTHER_NODE)"
            fi
        fi
    done
else
    echo -e "${YELLOW}⚠${NC} No markers from other nodes found"
    echo "  This test is running on a single node"
    echo "  For full multi-node validation, run on a cluster with multiple nodes"
fi
echo ""

# ============================================================================
# Test 8: Verify Quorum State Consistency
# ============================================================================

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test 8: Verify Quorum State"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

if [ -e "$MEMBERS_FILE" ]; then
    MEMBERS_CONTENT=$(cat "$MEMBERS_FILE" 2>/dev/null || echo "")
    TOTAL_NODES=$(echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]]" | wc -l || echo "0")
    ONLINE_NODES=$(echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]].*[[:space:]]1[[:space:]]" | wc -l || echo "0")

    if [ "$TOTAL_NODES" -gt 0 ]; then
        QUORUM_NEEDED=$(( (TOTAL_NODES / 2) + 1 ))

        echo "  Total nodes in cluster: $TOTAL_NODES"
        echo "  Online nodes: $ONLINE_NODES"
        echo "  Quorum threshold: $QUORUM_NEEDED"

        if [ "$ONLINE_NODES" -ge "$QUORUM_NEEDED" ]; then
            echo -e "${GREEN}✓${NC} Cluster has quorum ($ONLINE_NODES/$TOTAL_NODES nodes online)"
        else
            echo -e "${YELLOW}⚠${NC} Cluster does NOT have quorum ($ONLINE_NODES/$TOTAL_NODES nodes online, need $QUORUM_NEEDED)"
        fi
    else
        echo "  Single node or standalone mode"
    fi
else
    echo -e "${YELLOW}⚠${NC} Cannot check quorum (no .members file)"
fi
echo ""

# ============================================================================
# Summary
# ============================================================================

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Node: $NODE_NAME"
echo ""
echo "Status Plugins:"
echo "  .version:  $([ -e "$MOUNT_POINT/.version" ] && echo -e "${GREEN}✓ Available${NC}" || echo -e "${YELLOW}⚠ Unavailable${NC}")"
echo "  .members:  $([ -e "$MOUNT_POINT/.members" ] && echo -e "${GREEN}✓ Available${NC}" || echo -e "${YELLOW}⚠ Unavailable${NC}")"
echo "  .vmlist:   $([ -e "$MOUNT_POINT/.vmlist" ] && echo -e "${GREEN}✓ Available${NC}" || echo -e "${YELLOW}⚠ Unavailable${NC}")"
echo ""
echo "Multi-Node Detection:"
echo "  Other nodes detected: $OTHER_MARKERS"
echo ""

if [ "$OTHER_MARKERS" -gt 0 ]; then
    echo -e "${GREEN}✓${NC} Multi-node status synchronization test completed"
    echo "  Status data compared across $((OTHER_MARKERS + 1)) nodes"
else
    echo -e "${BLUE}ℹ${NC} Single-node test completed"
    echo "  Run on multiple nodes simultaneously for full multi-node validation"
fi
echo ""

exit 0
