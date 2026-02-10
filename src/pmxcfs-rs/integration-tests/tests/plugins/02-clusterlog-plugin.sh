#!/bin/bash
# Test: ClusterLog Plugin FUSE File
# Comprehensive test for .clusterlog plugin file functionality

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "========================================="
echo "ClusterLog Plugin FUSE File Test"
echo "========================================="
echo ""

# Configuration
MOUNT_PATH="$TEST_MOUNT_PATH"
CLUSTERLOG_FILE="$MOUNT_PATH/.clusterlog"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test counters
TESTS_PASSED=0
TESTS_FAILED=0
TOTAL_TESTS=0

# Helper functions
log_info() {
    echo "[INFO] $1"
}

log_error() {
    echo -e "${RED}[ERROR] $1${NC}" >&2
}

log_success() {
    echo -e "${GREEN}[✓] $1${NC}"
}

log_warning() {
    echo -e "${YELLOW}[⚠] $1${NC}"
}

test_start() {
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    echo ""
    echo "Test $TOTAL_TESTS: $1"
    echo "----------------------------------------"
}

test_pass() {
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_success "$1"
}

test_fail() {
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_error "$1"
}

# Test 1: Plugin file exists
test_start "Verify .clusterlog plugin file exists"

if [ -e "$CLUSTERLOG_FILE" ]; then
    test_pass ".clusterlog file exists at $CLUSTERLOG_FILE"
else
    test_fail ".clusterlog file does not exist at $CLUSTERLOG_FILE"
    log_info "Directory contents:"
    ls -la "$MOUNT_PATH" || true
    exit 1
fi

# Test 2: Plugin file is readable
test_start "Verify .clusterlog plugin file is readable"

if [ -r "$CLUSTERLOG_FILE" ]; then
    test_pass ".clusterlog file is readable"

    # Try to read it
    CONTENT=$(cat "$CLUSTERLOG_FILE" 2>/dev/null || echo "")
    if [ -n "$CONTENT" ]; then
        CONTENT_LEN=${#CONTENT}
        test_pass ".clusterlog file has content ($CONTENT_LEN bytes)"
    else
        test_fail ".clusterlog file is empty or unreadable"
    fi
else
    test_fail ".clusterlog file is not readable"
    exit 1
fi

# Test 3: Content is valid JSON
test_start "Verify .clusterlog content is valid JSON"

CONTENT=$(cat "$CLUSTERLOG_FILE")
if echo "$CONTENT" | jq . >/dev/null 2>&1; then
    test_pass "Content is valid JSON"
else
    test_fail "Content is not valid JSON"
    log_info "Content preview:"
    echo "$CONTENT" | head -10
    exit 1
fi

# Test 4: JSON has correct structure
test_start "Verify JSON has correct structure (object with 'data' array)"

if echo "$CONTENT" | jq -e 'type == "object"' >/dev/null 2>&1; then
    test_pass "JSON is an object"
else
    test_fail "JSON is not an object"
    exit 1
fi

if echo "$CONTENT" | jq -e 'has("data")' >/dev/null 2>&1; then
    test_pass "JSON has 'data' field"
else
    test_fail "JSON does not have 'data' field"
    exit 1
fi

if echo "$CONTENT" | jq -e '.data | type == "array"' >/dev/null 2>&1; then
    test_pass "'data' field is an array"
else
    test_fail "'data' field is not an array"
    exit 1
fi

# Test 5: Entry format validation (if entries exist)
test_start "Verify log entry format (if entries exist)"

ENTRY_COUNT=$(echo "$CONTENT" | jq '.data | length')
log_info "Found $ENTRY_COUNT entries in cluster log"

if [ "$ENTRY_COUNT" -gt 0 ]; then
    # Required fields according to C implementation
    REQUIRED_FIELDS=("uid" "time" "pri" "tag" "pid" "node" "user" "msg")

    FIRST_ENTRY=$(echo "$CONTENT" | jq '.data[0]')

    ALL_FIELDS_PRESENT=true
    for field in "${REQUIRED_FIELDS[@]}"; do
        if echo "$FIRST_ENTRY" | jq -e "has(\"$field\")" >/dev/null 2>&1; then
            log_info "  ✓ Field '$field' present"
        else
            log_error "  ✗ Field '$field' missing"
            ALL_FIELDS_PRESENT=false
        fi
    done

    if [ "$ALL_FIELDS_PRESENT" = true ]; then
        test_pass "All required fields present"
    else
        test_fail "Some required fields missing"
        exit 1
    fi

    # Validate field types
    test_start "Verify field types"

    # uid should be number
    if echo "$FIRST_ENTRY" | jq -e '.uid | type == "number"' >/dev/null 2>&1; then
        test_pass "uid is a number"
    else
        test_fail "uid is not a number"
    fi

    # time should be number
    if echo "$FIRST_ENTRY" | jq -e '.time | type == "number"' >/dev/null 2>&1; then
        test_pass "time is a number"
    else
        test_fail "time is not a number"
    fi

    # pri should be number
    if echo "$FIRST_ENTRY" | jq -e '.pri | type == "number"' >/dev/null 2>&1; then
        test_pass "pri is a number"
    else
        test_fail "pri is not a number"
    fi

    # pid should be number
    if echo "$FIRST_ENTRY" | jq -e '.pid | type == "number"' >/dev/null 2>&1; then
        test_pass "pid is a number"
    else
        test_fail "pid is not a number"
    fi

    # tag should be string
    if echo "$FIRST_ENTRY" | jq -e '.tag | type == "string"' >/dev/null 2>&1; then
        test_pass "tag is a string"
    else
        test_fail "tag is not a string"
    fi

    # node should be string
    if echo "$FIRST_ENTRY" | jq -e '.node | type == "string"' >/dev/null 2>&1; then
        test_pass "node is a string"
    else
        test_fail "node is not a string"
    fi

    # user should be string
    if echo "$FIRST_ENTRY" | jq -e '.user | type == "string"' >/dev/null 2>&1; then
        test_pass "user is a string"
    else
        test_fail "user is not a string"
    fi

    # msg should be string
    if echo "$FIRST_ENTRY" | jq -e '.msg | type == "string"' >/dev/null 2>&1; then
        test_pass "msg is a string"
    else
        test_fail "msg is not a string"
    fi
else
    log_warning "No entries in cluster log, skipping entry format tests"
fi

# Test 6: Multiple reads return consistent data
test_start "Verify multiple reads return consistent data"

CONTENT1=$(cat "$CLUSTERLOG_FILE")
sleep 0.1
CONTENT2=$(cat "$CLUSTERLOG_FILE")

if [ "$CONTENT1" = "$CONTENT2" ]; then
    test_pass "Multiple reads return consistent data"
else
    test_fail "Multiple reads returned different data"
    log_info "This may be normal if new entries were added between reads"
fi

# Test 7: File metadata is accessible
test_start "Verify file metadata is accessible"

if stat "$CLUSTERLOG_FILE" >/dev/null 2>&1; then
    test_pass "stat() succeeds on .clusterlog"

    # Get file type
    FILE_TYPE=$(stat -c "%F" "$CLUSTERLOG_FILE" 2>/dev/null || stat -f "%HT" "$CLUSTERLOG_FILE" 2>/dev/null || echo "unknown")
    log_info "File type: $FILE_TYPE"

    # Get permissions
    PERMS=$(stat -c "%a" "$CLUSTERLOG_FILE" 2>/dev/null || stat -f "%Lp" "$CLUSTERLOG_FILE" 2>/dev/null || echo "unknown")
    log_info "Permissions: $PERMS"

    test_pass "File metadata accessible"
else
    test_fail "stat() failed on .clusterlog"
fi

# Test 8: File should be read-only (writes should fail)
test_start "Verify .clusterlog is read-only"

if echo "test data" > "$CLUSTERLOG_FILE" 2>/dev/null; then
    test_fail ".clusterlog should be read-only but write succeeded"
else
    test_pass ".clusterlog is read-only (write correctly rejected)"
fi

# Test 9: File appears in directory listing
test_start "Verify .clusterlog appears in directory listing"

if ls -la "$MOUNT_PATH" | grep -q "\.clusterlog"; then
    test_pass ".clusterlog appears in directory listing"
else
    test_fail ".clusterlog does not appear in directory listing"
    log_info "Directory listing:"
    ls -la "$MOUNT_PATH"
fi

# Test 10: Concurrent reads work correctly
test_start "Verify concurrent reads work correctly"

# Start 5 parallel reads
PIDS=()
TEMP_DIR=$(mktemp -d)

for i in {1..5}; do
    (
        CONTENT=$(cat "$CLUSTERLOG_FILE")
        echo "$CONTENT" > "$TEMP_DIR/read_$i.json"
        echo ${#CONTENT} > "$TEMP_DIR/size_$i.txt"
    ) &
    PIDS+=($!)
done

# Wait for all reads to complete
for pid in "${PIDS[@]}"; do
    wait $pid
done

# Check if all reads succeeded and returned same size
FIRST_SIZE=$(cat "$TEMP_DIR/size_1.txt")
ALL_SAME=true

for i in {2..5}; do
    SIZE=$(cat "$TEMP_DIR/size_$i.txt")
    if [ "$SIZE" != "$FIRST_SIZE" ]; then
        ALL_SAME=false
        log_warning "Read $i returned different size: $SIZE vs $FIRST_SIZE"
    fi
done

if [ "$ALL_SAME" = true ]; then
    test_pass "Concurrent reads all returned same size ($FIRST_SIZE bytes)"
else
    log_warning "Concurrent reads returned different sizes (may indicate race condition)"
fi

# Cleanup
rm -rf "$TEMP_DIR"

# Test 11: Verify file size matches content length
test_start "Verify file size consistency"

CONTENT=$(cat "$CLUSTERLOG_FILE")
CONTENT_LEN=${#CONTENT}
FILE_SIZE=$(stat -c "%s" "$CLUSTERLOG_FILE" 2>/dev/null || stat -f "%z" "$CLUSTERLOG_FILE" 2>/dev/null || echo "0")

log_info "Content length: $CONTENT_LEN bytes"
log_info "File size (stat): $FILE_SIZE bytes"

# File size might be 0 for special files or might match content
if [ "$FILE_SIZE" -eq "$CONTENT_LEN" ] || [ "$FILE_SIZE" -eq 0 ]; then
    test_pass "File size is consistent"
else
    log_warning "File size ($FILE_SIZE) differs from content length ($CONTENT_LEN)"
    log_info "This may be normal for FUSE plugin files"
fi

# Summary
echo ""
echo "========================================="
echo "Test Summary"
echo "========================================="
echo "Total tests: $TOTAL_TESTS"
echo "Passed: $TESTS_PASSED"
echo "Failed: $TESTS_FAILED"
echo ""

if [ $TESTS_FAILED -eq 0 ]; then
    log_success "✓ All tests PASSED"
    echo ""
    log_info "ClusterLog plugin FUSE file is working correctly!"
    exit 0
else
    log_error "✗ Some tests FAILED"
    exit 1
fi
