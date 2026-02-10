#!/bin/bash
# Test: Status Tracking
# Verify status tracking and VM registry functionality

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing status tracking..."

MOUNT_PATH="$TEST_MOUNT_PATH"

# Check if mount path is accessible
if [ ! -d "$MOUNT_PATH" ]; then
    echo "ERROR: Mount path not accessible: $MOUNT_PATH"
    exit 1
fi
echo "✓ Mount path accessible"

# Test .version plugin (status version tracking)
VERSION_FILE="$MOUNT_PATH/.version"
if [ -f "$VERSION_FILE" ] || [ -e "$VERSION_FILE" ]; then
    echo "✓ .version plugin file exists"

    # Try to read version info
    if VERSION_CONTENT=$(cat "$VERSION_FILE" 2>/dev/null); then
        echo "✓ .version file readable"
        echo "  Version content: $VERSION_CONTENT"

        # Validate version format (should be colon-separated values)
        if echo "$VERSION_CONTENT" | grep -qE '^[0-9]+:[0-9]+:[0-9]+'; then
            echo "✓ Version format valid"
        else
            echo "⚠ Warning: Version format unexpected"
        fi
    else
        echo "⚠ Warning: Cannot read .version file"
    fi
else
    echo "⚠ Warning: .version plugin not available"
fi

# Test .members plugin (cluster membership tracking)
MEMBERS_FILE="$MOUNT_PATH/.members"
if [ -f "$MEMBERS_FILE" ] || [ -e "$MEMBERS_FILE" ]; then
    echo "✓ .members plugin file exists"

    # Try to read members info
    if MEMBERS_CONTENT=$(cat "$MEMBERS_FILE" 2>/dev/null); then
        echo "✓ .members file readable"

        # Count member entries
        MEMBER_COUNT=$(echo "$MEMBERS_CONTENT" | grep -c "^\[members\]\|^[0-9]" || echo "0")
        echo "  Member entries: $MEMBER_COUNT"

        if echo "$MEMBERS_CONTENT" | grep -q "\[members\]"; then
            echo "✓ Members format valid"
        fi
    else
        echo "⚠ Warning: Cannot read .members file"
    fi
else
    echo "⚠ Warning: .members plugin not available"
fi

# Test .vmlist plugin (VM/CT registry)
VMLIST_FILE="$MOUNT_PATH/.vmlist"
if [ -f "$VMLIST_FILE" ] || [ -e "$VMLIST_FILE" ]; then
    echo "✓ .vmlist plugin file exists"

    # Try to read VM list
    if VMLIST_CONTENT=$(cat "$VMLIST_FILE" 2>/dev/null); then
        echo "✓ .vmlist file readable"

        # Check for QEMU and LXC sections
        if echo "$VMLIST_CONTENT" | grep -q "\[qemu\]"; then
            echo "  Found [qemu] section"
        fi
        if echo "$VMLIST_CONTENT" | grep -q "\[lxc\]"; then
            echo "  Found [lxc] section"
        fi

        # Count VM entries (lines with tab-separated values)
        VM_COUNT=$(echo "$VMLIST_CONTENT" | grep -E "^[0-9]+\t" | wc -l)
        echo "  VM/CT entries: $VM_COUNT"
    else
        echo "⚠ Warning: Cannot read .vmlist file"
    fi
else
    echo "⚠ Warning: .vmlist plugin not available"
fi

# Check for node-specific status files in /test/pve/nodes/
NODES_DIR="$MOUNT_PATH/nodes"
if [ -d "$NODES_DIR" ]; then
    echo "✓ Nodes directory exists"
    NODE_COUNT=$(ls -1 "$NODES_DIR" 2>/dev/null | wc -l)
    echo "  Node count: $NODE_COUNT"
else
    echo "  Nodes directory not yet created"
fi

# Test quorum status (if available via .members or dedicated file)
if [ -f "$MEMBERS_FILE" ]; then
    if cat "$MEMBERS_FILE" 2>/dev/null | grep -q "online.*1"; then
        echo "✓ At least one node appears online"
    fi
fi

echo "✓ Status tracking test completed"
exit 0
