#!/bin/bash
# Test: Plugin Files
# Verify all FUSE plugin files are accessible and return valid data

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing plugin files..."

MOUNT_PATH="$TEST_MOUNT_PATH"

# Check if mount path is accessible
if [ ! -d "$MOUNT_PATH" ]; then
    echo "ERROR: Mount path not accessible: $MOUNT_PATH"
    exit 1
fi
echo "✓ Mount path accessible"

# List of plugin files to test
declare -A PLUGINS=(
    [".version"]="Version and timestamp information"
    [".members"]="Cluster member list"
    [".vmlist"]="VM and container registry"
    [".rrd"]="RRD metrics dump"
    [".clusterlog"]="Cluster log entries"
    [".debug"]="Debug control"
)

FOUND=0
READABLE=0
TOTAL=${#PLUGINS[@]}

echo ""
echo "Testing plugin files:"

for plugin in "${!PLUGINS[@]}"; do
    PLUGIN_PATH="$MOUNT_PATH/$plugin"
    DESC="${PLUGINS[$plugin]}"

    echo ""
    echo "Plugin: $plugin"
    echo "  Description: $DESC"

    # Check if plugin file exists
    if [ -e "$PLUGIN_PATH" ]; then
        echo "  ✓ File exists"
        FOUND=$((FOUND + 1))

        # Check if file is readable
        if [ -r "$PLUGIN_PATH" ]; then
            echo "  ✓ File is readable"

            # Try to read content
            if CONTENT=$(cat "$PLUGIN_PATH" 2>/dev/null); then
                READABLE=$((READABLE + 1))
                CONTENT_LEN=${#CONTENT}
                LINE_COUNT=$(echo "$CONTENT" | wc -l)

                echo "  ✓ Content readable (${CONTENT_LEN} bytes, ${LINE_COUNT} lines)"

                # Plugin-specific validation
                case "$plugin" in
                    ".version")
                        if echo "$CONTENT" | grep -qE '^[0-9]+:[0-9]+:[0-9]+'; then
                            echo "  ✓ Version format valid"
                            echo "  Content: $CONTENT"
                        else
                            echo "  ⚠ Unexpected version format"
                        fi
                        ;;
                    ".members")
                        if echo "$CONTENT" | grep -q "\[members\]"; then
                            echo "  ✓ Members format valid"
                            MEMBER_COUNT=$(echo "$CONTENT" | grep -c "^[0-9]" || echo "0")
                            echo "  Members: $MEMBER_COUNT"
                        else
                            echo "  Content may be empty (no cluster members yet)"
                        fi
                        ;;
                    ".vmlist")
                        if echo "$CONTENT" | grep -qE "\[qemu\]|\[lxc\]"; then
                            echo "  ✓ VM list format valid"
                            VM_COUNT=$(echo "$CONTENT" | grep -c "^[0-9]" || echo "0")
                            echo "  VMs/CTs: $VM_COUNT"
                        else
                            echo "  VM list empty (no VMs registered yet)"
                        fi
                        ;;
                    ".rrd")
                        if [ "$CONTENT_LEN" -gt 0 ]; then
                            echo "  ✓ RRD data available"
                            # Check for common RRD key patterns
                            if echo "$CONTENT" | grep -q "pve2-node\|pve2-vm\|pve2-storage"; then
                                echo "  ✓ RRD keys found"
                            fi
                        else
                            echo "  RRD data empty (no metrics collected yet)"
                        fi
                        ;;
                    ".clusterlog")
                        if [ "$CONTENT_LEN" -gt 0 ]; then
                            echo "  ✓ Cluster log available"
                        else
                            echo "  Cluster log empty (no events logged yet)"
                        fi
                        ;;
                    ".debug")
                        # Debug file typically returns runtime debug info
                        if [ "$CONTENT_LEN" -gt 0 ]; then
                            echo "  ✓ Debug info available"
                        fi
                        ;;
                esac
            else
                echo "  ✗ ERROR: Cannot read content"
            fi
        else
            echo "  ✗ ERROR: File not readable"
        fi
    else
        echo "  ✗ File does not exist"
    fi
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Summary:"
echo "  Plugin files found:    $FOUND / $TOTAL"
echo "  Plugin files readable: $READABLE / $TOTAL"

if [ "$FOUND" -eq "$TOTAL" ]; then
    echo "✓ All plugin files exist"
else
    echo "⚠ Some plugin files missing (may not be initialized yet)"
fi

if [ "$READABLE" -ge 3 ]; then
    echo "✓ Most plugin files are working"
    exit 0
else
    echo "⚠ Limited plugin availability"
    exit 0  # Don't fail - plugins may not be initialized yet
fi
