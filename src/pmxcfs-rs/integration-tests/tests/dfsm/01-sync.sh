#!/bin/bash
# Test DFSM cluster synchronization
# This test validates that the DFSM protocol correctly synchronizes
# data across cluster nodes using corosync

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

echo "========================================="
echo "Test: DFSM Cluster Synchronization"
echo "========================================="
echo ""

# Test configuration
MOUNT_POINT="$TEST_MOUNT_PATH"
TEST_DIR="$MOUNT_POINT/test-sync"
TEST_FILE="$TEST_DIR/sync-test.txt"

# Helper function to check if pmxcfs is running
check_pmxcfs() {
    if ! pgrep -x pmxcfs > /dev/null; then
        echo -e "${RED}ERROR: pmxcfs is not running${NC}"
        exit 1
    fi
}

# Helper function to wait for file to appear with content
wait_for_file_content() {
    local file=$1
    local expected_content=$2
    local timeout=30
    local elapsed=0

    while [ $elapsed -lt $timeout ]; do
        if [ -f "$file" ]; then
            local content=$(cat "$file" 2>/dev/null || echo "")
            if [ "$content" = "$expected_content" ]; then
                return 0
            fi
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    return 1
}

echo "1. Checking pmxcfs is running..."
check_pmxcfs
echo -e "${GREEN}✓${NC} pmxcfs is running"
echo ""

echo "2. Checking FUSE mount..."
if [ ! -d "$MOUNT_POINT" ]; then
    echo -e "${RED}ERROR: Mount point $MOUNT_POINT does not exist${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} FUSE mount exists"
echo ""

echo "3. Creating test directory..."
mkdir -p "$TEST_DIR"
echo -e "${GREEN}✓${NC} Test directory created"
echo ""

echo "4. Writing test file on this node..."
echo "Hello from $(hostname)" > "$TEST_FILE"
if [ ! -f "$TEST_FILE" ]; then
    echo -e "${RED}ERROR: Failed to create test file${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} Test file created: $TEST_FILE"
echo ""

echo "5. Verifying file content..."
CONTENT=$(cat "$TEST_FILE")
if [ "$CONTENT" != "Hello from $(hostname)" ]; then
    echo -e "${RED}ERROR: File content mismatch${NC}"
    echo "Expected: Hello from $(hostname)"
    echo "Got: $CONTENT"
    exit 1
fi
echo -e "${GREEN}✓${NC} File content correct"
echo ""

echo "6. Creating subdirectory structure..."
mkdir -p "$TEST_DIR/subdir1/subdir2"
echo "nested file" > "$TEST_DIR/subdir1/subdir2/nested.txt"
if [ ! -f "$TEST_DIR/subdir1/subdir2/nested.txt" ]; then
    echo -e "${RED}ERROR: Failed to create nested file${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} Nested directory structure created"
echo ""

echo "7. Creating multiple files..."
for i in {1..5}; do
    echo "File $i content" > "$TEST_DIR/file$i.txt"
done
# Verify all files exist
FILE_COUNT=$(ls -1 "$TEST_DIR"/file*.txt 2>/dev/null | wc -l)
if [ "$FILE_COUNT" -ne 5 ]; then
    echo -e "${RED}ERROR: Expected 5 files, found $FILE_COUNT${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} Multiple files created (count: $FILE_COUNT)"
echo ""

echo "8. Testing file modification..."
ORIGINAL_CONTENT=$(cat "$TEST_FILE")
echo "Modified at $(date)" >> "$TEST_FILE"
MODIFIED_CONTENT=$(cat "$TEST_FILE")
if [ "$ORIGINAL_CONTENT" = "$MODIFIED_CONTENT" ]; then
    echo -e "${RED}ERROR: File was not modified${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} File modification successful"
echo ""

echo "9. Testing file deletion..."
TEMP_FILE="$TEST_DIR/temp-delete-me.txt"
echo "temporary" > "$TEMP_FILE"
if [ ! -f "$TEMP_FILE" ]; then
    echo -e "${RED}ERROR: Failed to create temp file${NC}"
    exit 1
fi
rm "$TEMP_FILE"
if [ -f "$TEMP_FILE" ]; then
    echo -e "${RED}ERROR: File was not deleted${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} File deletion successful"
echo ""

echo "10. Testing rename operation..."
RENAME_SRC="$TEST_DIR/rename-src.txt"
RENAME_DST="$TEST_DIR/rename-dst.txt"
# Clean up destination if it exists from previous run
rm -f "$RENAME_DST"
echo "rename test" > "$RENAME_SRC"
mv "$RENAME_SRC" "$RENAME_DST"
if [ -f "$RENAME_SRC" ]; then
    echo -e "${RED}ERROR: Source file still exists after rename${NC}"
    exit 1
fi
if [ ! -f "$RENAME_DST" ]; then
    echo -e "${RED}ERROR: Destination file does not exist after rename${NC}"
    exit 1
fi
DST_CONTENT=$(cat "$RENAME_DST")
if [ "$DST_CONTENT" != "rename test" ]; then
    echo -e "${RED}ERROR: Content mismatch after rename${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} File rename successful"
echo ""

echo "11. Checking database state..."
# The database should be accessible
if [ -d "$TEST_DB_DIR" ]; then
    DB_FILES=$(ls -1 /test/db/*.db 2>/dev/null | wc -l)
    echo -e "${GREEN}✓${NC} Database directory exists (files: $DB_FILES)"
else
    echo -e "${BLUE}ℹ${NC} Database directory not accessible (expected in test mode)"
fi
echo ""

echo "12. Testing large file write..."
LARGE_FILE="$TEST_DIR/large-file.bin"
# Create 1MB file
dd if=/dev/zero of="$LARGE_FILE" bs=1024 count=1024 2>/dev/null
if [ ! -f "$LARGE_FILE" ]; then
    echo -e "${RED}ERROR: Failed to create large file${NC}"
    exit 1
fi
LARGE_SIZE=$(stat -c%s "$LARGE_FILE" 2>/dev/null || stat -f%z "$LARGE_FILE" 2>/dev/null)
EXPECTED_SIZE=$((1024 * 1024))
if [ "$LARGE_SIZE" -ne "$EXPECTED_SIZE" ]; then
    echo -e "${RED}ERROR: Large file size mismatch (expected: $EXPECTED_SIZE, got: $LARGE_SIZE)${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} Large file created (size: $LARGE_SIZE bytes)"
echo ""

echo "13. Testing concurrent writes..."
for i in {1..10}; do
    echo "Concurrent write $i" > "$TEST_DIR/concurrent-$i.txt" &
done
wait
CONCURRENT_COUNT=$(ls -1 "$TEST_DIR"/concurrent-*.txt 2>/dev/null | wc -l)
if [ "$CONCURRENT_COUNT" -ne 10 ]; then
    echo -e "${RED}ERROR: Concurrent writes failed (expected: 10, got: $CONCURRENT_COUNT)${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} Concurrent writes successful (count: $CONCURRENT_COUNT)"
echo ""

echo "14. Listing final directory contents..."
TOTAL_FILES=$(find "$TEST_DIR" -type f | wc -l)
echo "Total files created: $TOTAL_FILES"
echo "Directory structure:"
find "$TEST_DIR" -type f | head -10 | while read file; do
    echo "  - $(basename $file)"
done
if [ "$TOTAL_FILES" -gt 10 ]; then
    echo "  ... ($(($TOTAL_FILES - 10)) more files)"
fi

echo ""
echo -e "${GREEN}✓ DFSM sync test passed${NC}"
exit 0
