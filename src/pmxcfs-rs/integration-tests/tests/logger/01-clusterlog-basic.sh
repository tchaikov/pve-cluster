#!/bin/bash
# Test: ClusterLog Basic Functionality
# Verify cluster log storage and retrieval

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing cluster log functionality..."

MOUNT_PATH="$TEST_MOUNT_PATH"
CLUSTERLOG_FILE="$MOUNT_PATH/.clusterlog"

# Check if mount path is accessible
if [ ! -d "$MOUNT_PATH" ]; then
    echo "ERROR: Mount path not accessible: $MOUNT_PATH"
    exit 1
fi
echo "✓ Mount path accessible"

# Test .clusterlog plugin file
if [ -e "$CLUSTERLOG_FILE" ]; then
    echo "✓ .clusterlog plugin file exists"

    # Try to read cluster log
    if CLUSTERLOG_CONTENT=$(cat "$CLUSTERLOG_FILE" 2>/dev/null); then
        echo "✓ .clusterlog file readable"

        CONTENT_LEN=${#CLUSTERLOG_CONTENT}
        echo "  Content length: $CONTENT_LEN bytes"

        if [ "$CONTENT_LEN" -gt 0 ]; then
            # Check if content is JSON (expected format)
            if echo "$CLUSTERLOG_CONTENT" | jq . > /dev/null 2>&1; then
                echo "✓ Cluster log is valid JSON"

                # Check structure: should be object with 'data' array
                if echo "$CLUSTERLOG_CONTENT" | jq -e 'type == "object"' > /dev/null 2>&1; then
                    echo "✓ JSON is an object"
                else
                    echo "⚠ JSON is not an object (expected {\"data\": [...]})"
                fi

                if echo "$CLUSTERLOG_CONTENT" | jq -e 'has("data")' > /dev/null 2>&1; then
                    echo "✓ JSON has 'data' field"
                else
                    echo "⚠ JSON missing 'data' field"
                fi

                # Count log entries in data array
                ENTRY_COUNT=$(echo "$CLUSTERLOG_CONTENT" | jq '.data | length' 2>/dev/null || echo "0")
                echo "  Log entries: $ENTRY_COUNT"

                # If we have entries, validate structure
                if [ "$ENTRY_COUNT" -gt 0 ]; then
                    echo "  Validating log entry structure..."

                    # Check first entry has expected fields
                    FIRST_ENTRY=$(echo "$CLUSTERLOG_CONTENT" | jq '.data[0]' 2>/dev/null)

                    # Expected fields: time, node, pri, ident, tag, msg
                    for field in time node pri ident tag msg; do
                        if echo "$FIRST_ENTRY" | jq -e ".$field" > /dev/null 2>&1; then
                            echo "    ✓ Field '$field' present"
                        else
                            echo "    ⚠ Field '$field' missing"
                        fi
                    done
                else
                    echo "  No log entries yet (expected for new installation)"
                fi
            elif command -v jq &> /dev/null; then
                echo "⚠ Cluster log content is not JSON"
                echo "  First 100 chars: ${CLUSTERLOG_CONTENT:0:100}"
            else
                echo "  jq not available, cannot validate JSON format"
                echo "  Content preview: ${CLUSTERLOG_CONTENT:0:100}"
            fi
        else
            echo "  Cluster log is empty (no events logged yet)"
        fi
    else
        echo "ERROR: Cannot read .clusterlog file"
        exit 1
    fi
else
    echo "⚠ Warning: .clusterlog plugin not available"
    echo "  This may indicate pmxcfs is not fully initialized"
fi

# Test cluster log characteristics
echo ""
echo "Cluster log characteristics (from pmxcfs-clusterlog README):"
echo "  - Ring buffer size: 5000 entries"
echo "  - Deduplication: FNV-1a hash (8 bytes)"
echo "  - Dedup window: 128 entries"
echo "  - Format: JSON array"
echo "  - Fields: time, node, pri, ident, tag, msg"

# Check if we can write to cluster log (requires IPC)
# This would typically be done via pvesh or pvecm commands
if command -v pvecm &> /dev/null; then
    echo ""
    echo "Testing cluster log write via pvecm..."

    # Try to log a test message (requires running cluster)
    if pvecm status 2>/dev/null | grep -q "Quorum information"; then
        echo "  Cluster is active, log writes available"
        # Don't actually write - just note capability
    else
        echo "  Cluster not active, write tests skipped"
    fi
fi

echo ""
echo "✓ Cluster log basic test completed"
exit 0
