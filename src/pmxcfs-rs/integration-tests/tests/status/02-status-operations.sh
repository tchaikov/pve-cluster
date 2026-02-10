#!/bin/bash
# Test: Status Operations (VM Registration, Cluster Membership)
# Comprehensive testing of status tracking operations

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing status operations..."

MOUNT_PATH="$TEST_MOUNT_PATH"

# Check if mount path is accessible
if [ ! -d "$MOUNT_PATH" ]; then
    echo "ERROR: Mount path not accessible: $MOUNT_PATH"
    exit 1
fi
echo "✓ Mount path accessible"

# Test .vmlist plugin - VM/CT registry operations
echo ""
echo "Testing VM/CT registry operations..."

VMLIST_FILE="$MOUNT_PATH/.vmlist"
if [ -e "$VMLIST_FILE" ]; then
    VMLIST_CONTENT=$(cat "$VMLIST_FILE" 2>/dev/null || echo "")

    # Check for both QEMU and LXC sections
    if echo "$VMLIST_CONTENT" | grep -q "\[qemu\]"; then
        echo "✓ QEMU section present in .vmlist"

        # Count QEMU VMs (lines with tab-separated values after [qemu])
        QEMU_COUNT=$(echo "$VMLIST_CONTENT" | sed -n '/\[qemu\]/,/\[lxc\]/p' | grep -E "^[0-9]+[[:space:]]" | wc -l || echo "0")
        echo "  QEMU VMs: $QEMU_COUNT"
    else
        echo "  No QEMU VMs registered"
    fi

    if echo "$VMLIST_CONTENT" | grep -q "\[lxc\]"; then
        echo "✓ LXC section present in .vmlist"

        # Count LXC containers
        LXC_COUNT=$(echo "$VMLIST_CONTENT" | sed -n '/\[lxc\]/,$p' | grep -E "^[0-9]+[[:space:]]" | wc -l || echo "0")
        echo "  LXC containers: $LXC_COUNT"
    else
        echo "  No LXC containers registered"
    fi

    # Verify format: each entry should be "VMID<tab>NODE<tab>VERSION"
    TOTAL_VMS=$(echo "$VMLIST_CONTENT" | grep -E "^[0-9]+[[:space:]]" | wc -l || echo "0")
    if [ "$TOTAL_VMS" -gt 0 ]; then
        echo "✓ Total VMs/CTs: $TOTAL_VMS"

        # Check format of first entry
        FIRST_ENTRY=$(echo "$VMLIST_CONTENT" | grep -E "^[0-9]+[[:space:]]" | head -1)
        FIELD_COUNT=$(echo "$FIRST_ENTRY" | awk '{print NF}')

        if [ "$FIELD_COUNT" -ge 2 ]; then
            echo "✓ VM list entry format valid (VMID + node + version)"
        else
            echo "⚠ Warning: Unexpected VM list entry format"
        fi
    fi
else
    echo "  .vmlist plugin not yet available"
fi

# Test cluster membership (.members plugin)
echo ""
echo "Testing cluster membership..."

MEMBERS_FILE="$MOUNT_PATH/.members"
if [ -e "$MEMBERS_FILE" ]; then
    MEMBERS_CONTENT=$(cat "$MEMBERS_FILE" 2>/dev/null || echo "")

    if echo "$MEMBERS_CONTENT" | grep -q "\[members\]"; then
        echo "✓ .members file has correct format"

        # Extract member information
        # Format: nodeid<tab>name<tab>online<tab>ip
        MEMBER_COUNT=$(echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]]" | wc -l || echo "0")
        echo "  Total nodes: $MEMBER_COUNT"

        if [ "$MEMBER_COUNT" -gt 0 ]; then
            # Check online nodes
            ONLINE_COUNT=$(echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]].*[[:space:]]1[[:space:]]" | wc -l || echo "0")
            echo "  Online nodes: $ONLINE_COUNT"

            # List node names
            echo "  Nodes:"
            echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]]" | while read -r line; do
                NODE_ID=$(echo "$line" | awk '{print $1}')
                NODE_NAME=$(echo "$line" | awk '{print $2}')
                ONLINE=$(echo "$line" | awk '{print $3}')
                NODE_IP=$(echo "$line" | awk '{print $4}')

                STATUS="offline"
                if [ "$ONLINE" = "1" ]; then
                    STATUS="online"
                fi

                echo "    - Node $NODE_ID: $NODE_NAME ($NODE_IP) - $STATUS"
            done
        fi
    fi
else
    echo "  .members plugin not yet available"
fi

# Test version tracking (.version plugin)
echo ""
echo "Testing version tracking..."

VERSION_FILE="$MOUNT_PATH/.version"
if [ -e "$VERSION_FILE" ]; then
    VERSION_CONTENT=$(cat "$VERSION_FILE" 2>/dev/null || echo "")

    # Version format: timestamp:vmlist_version:config_versions...
    if echo "$VERSION_CONTENT" | grep -qE '^[0-9]+:[0-9]+:[0-9]+'; then
        echo "✓ Version file format valid"

        # Extract components
        TIMESTAMP=$(echo "$VERSION_CONTENT" | cut -d':' -f1)
        VMLIST_VER=$(echo "$VERSION_CONTENT" | cut -d':' -f2)

        echo "  Start timestamp: $TIMESTAMP"
        echo "  VM list version: $VMLIST_VER"

        # Count total version fields
        VERSION_FIELDS=$(echo "$VERSION_CONTENT" | tr ':' '\n' | wc -l)
        echo "  Tracked config files: $((VERSION_FIELDS - 2))"
    else
        echo "⚠ Warning: Version format unexpected"
    fi
else
    echo "  .version plugin not yet available"
fi

# Test quorum state (if available in .members)
echo ""
echo "Testing quorum state..."

if [ -e "$MEMBERS_FILE" ]; then
    # Check if cluster has quorum (simple heuristic: more than half online)
    TOTAL_NODES=$(echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]]" | wc -l || echo "0")
    ONLINE_NODES=$(echo "$MEMBERS_CONTENT" | grep -E "^[0-9]+[[:space:]].*[[:space:]]1[[:space:]]" | wc -l || echo "0")

    if [ "$TOTAL_NODES" -gt 0 ]; then
        QUORUM_NEEDED=$(( (TOTAL_NODES / 2) + 1 ))

        if [ "$ONLINE_NODES" -ge "$QUORUM_NEEDED" ]; then
            echo "✓ Cluster has quorum ($ONLINE_NODES/$TOTAL_NODES nodes online)"
        else
            echo "⚠ Cluster does NOT have quorum ($ONLINE_NODES/$TOTAL_NODES nodes online, need $QUORUM_NEEDED)"
        fi
    fi
fi

# Test node-specific directories
echo ""
echo "Testing node-specific structures..."

NODES_DIR="$MOUNT_PATH/nodes"
if [ -d "$NODES_DIR" ]; then
    NODE_COUNT=$(ls -1 "$NODES_DIR" 2>/dev/null | wc -l)
    echo "✓ Nodes directory exists with $NODE_COUNT nodes"

    # Check each node's subdirectories
    for node_dir in "$NODES_DIR"/*; do
        if [ -d "$node_dir" ]; then
            NODE_NAME=$(basename "$node_dir")
            echo "  Node: $NODE_NAME"

            # Check for expected subdirectories
            for subdir in qemu-server lxc openvz priv; do
                if [ -d "$node_dir/$subdir" ]; then
                    COUNT=$(ls -1 "$node_dir/$subdir" 2>/dev/null | wc -l)
                    if [ "$COUNT" -gt 0 ]; then
                        echo "    - $subdir/: $COUNT files"
                    fi
                fi
            done
        fi
    done
else
    echo "  Nodes directory not yet created"
fi

echo ""
echo "✓ Status operations test completed"
exit 0
