#!/bin/bash
# Test: File Operations
# Test basic file operations in mounted filesystem

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing file operations..."

MOUNT_PATH="$TEST_MOUNT_PATH"

# Check mount point is accessible
if [ ! -d "$MOUNT_PATH" ]; then
    echo "ERROR: Mount path not accessible: $MOUNT_PATH"
    exit 1
fi
echo "✓ Mount path accessible"

# Check if it's actually a FUSE mount or just a directory
if mount | grep -q "$MOUNT_PATH.*fuse"; then
    echo "✓ Path is FUSE-mounted"
    MOUNT_INFO=$(mount | grep "$MOUNT_PATH")
    echo "  Mount: $MOUNT_INFO"
    IS_FUSE=true
elif [ -d "$MOUNT_PATH" ]; then
    echo "  Path exists as directory (FUSE may not work in container)"
    IS_FUSE=false
else
    echo "ERROR: Mount path not available"
    exit 1
fi

# Test basic directory listing
echo "Testing directory listing..."
if ls -la "$MOUNT_PATH" > /dev/null 2>&1; then
    echo "✓ Directory listing works"
    FILE_COUNT=$(ls -A "$MOUNT_PATH" | wc -l)
    echo "  Files in mount: $FILE_COUNT"
else
    echo "ERROR: Cannot list directory"
    exit 1
fi

# If FUSE is working, test file operations
if [ "$IS_FUSE" = true ]; then
    # Test file creation
    TEST_FILE="$MOUNT_PATH/.container-test-$$"

    echo "Testing file creation..."
    if echo "test data" > "$TEST_FILE" 2>/dev/null; then
        echo "✓ File creation works"

        # Test file read
        echo "Testing file read..."
        CONTENT=$(cat "$TEST_FILE")
        if [ "$CONTENT" = "test data" ]; then
            echo "✓ File read works"
        else
            echo "ERROR: File read returned wrong content"
            exit 1
        fi

        # Test file deletion
        echo "Testing file deletion..."
        rm "$TEST_FILE"
        if [ ! -f "$TEST_FILE" ]; then
            echo "✓ File deletion works"
        else
            echo "ERROR: File deletion failed"
            exit 1
        fi
    else
        echo "  File creation not available (expected in some container configs)"
    fi
else
    echo "  Skipping file operations (FUSE not mounted)"
fi

# Check for plugin files (if any)
PLUGIN_FILES=(.version .members .vmlist .rrd .clusterlog)
FOUND_PLUGINS=0

for plugin in "${PLUGIN_FILES[@]}"; do
    if [ -e "$MOUNT_PATH/$plugin" ]; then
        FOUND_PLUGINS=$((FOUND_PLUGINS + 1))
        echo "  Found plugin: $plugin"
    fi
done

if [ $FOUND_PLUGINS -gt 0 ]; then
    echo "✓ Plugin files accessible ($FOUND_PLUGINS found)"
else
    echo "  No plugin files found (may not be initialized)"
fi

echo "✓ File operations test completed"
exit 0
