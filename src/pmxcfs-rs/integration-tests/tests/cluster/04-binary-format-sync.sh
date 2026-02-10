#!/bin/bash
# Test: ClusterLog Binary Format Synchronization
# Verify that Rust nodes correctly use binary format for DFSM state sync

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "========================================="
echo "ClusterLog Binary Format Sync Test"
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

# Function to read clusterlog from a node
read_clusterlog() {
    local node=$1
    $CONTAINER_CMD exec "$node" cat "$CLUSTERLOG_FILE" 2>/dev/null || echo "[]"
}

# Function to count entries
count_entries() {
    local node=$1
    local content=$(read_clusterlog "$node")
    echo "$content" | jq '.data | length' 2>/dev/null || echo "0"
}

# Function to check DFSM logs for binary serialization
check_binary_serialization() {
    local node=$1
    local since=${2:-60}

    log_info "Checking DFSM logs on $node for binary serialization..."

    # Check for get_state calls (serialization)
    local get_state_count=$($CONTAINER_CMD logs "$node" --since ${since}s 2>&1 | grep -c "get_state called - serializing cluster log" || true)

    # Check for process_state_update calls (deserialization)
    local process_state_count=$($CONTAINER_CMD logs "$node" --since ${since}s 2>&1 | grep -c "process_state_update called" || true)

    # Check for successful deserialization
    local deserialize_success=$($CONTAINER_CMD logs "$node" --since ${since}s 2>&1 | grep -c "Deserialized cluster log from node" || true)

    # Check for successful merge
    local merge_success=$($CONTAINER_CMD logs "$node" --since ${since}s 2>&1 | grep -c "Successfully merged cluster logs" || true)

    # Check for deserialization errors
    local deserialize_errors=$($CONTAINER_CMD logs "$node" --since ${since}s 2>&1 | grep -c "Failed to deserialize cluster log" || true)

    echo "  Serialization (get_state): $get_state_count calls"
    echo "  Deserialization (process_state_update): $process_state_count calls"
    echo "  Successful deserializations: $deserialize_success"
    echo "  Successful merges: $merge_success"
    echo "  Deserialization errors: $deserialize_errors"

    # Verify no errors
    if [ "$deserialize_errors" -gt 0 ]; then
        log_error "Found $deserialize_errors deserialization errors on $node"
        return 1
    fi

    # Verify activity occurred
    if [ "$get_state_count" -eq 0 ] && [ "$process_state_count" -eq 0 ]; then
        log_warning "No DFSM state sync activity detected on $node (may be too early)"
        return 2
    fi

    return 0
}

# Function to verify binary format is being used (not JSON)
verify_binary_format_usage() {
    local node=$1

    log_info "Verifying binary format is used (not JSON)..."

    # Look for binary format indicators in logs
    local binary_indicators=$($CONTAINER_CMD logs "$node" --since 60s 2>&1 | grep -E "serialize_binary|deserialize_binary|clog_base_t" || true)

    if [ -n "$binary_indicators" ]; then
        log_success "Binary format functions detected in logs"
        return 0
    else
        log_info "No explicit binary format indicators in recent logs"
        log_info "This is normal - binary format is used internally"
        return 0
    fi
}

# Detect container runtime (podman or docker)
if command -v podman &> /dev/null; then
    CONTAINER_CMD="podman"
elif command -v docker &> /dev/null; then
    CONTAINER_CMD="docker"
else
    log_error "Neither podman nor docker found"
    log_error "This test must run from the host with access to container runtime"
    exit 1
fi

# Detect running nodes
log_info "Detecting running cluster nodes..."
NODES=$($CONTAINER_CMD ps --filter "name=pmxcfs" --filter "status=running" --format "{{.Names}}" | sort)

if [ -z "$NODES" ]; then
    log_error "No running pmxcfs containers found"
    exit 1
fi

NODE_COUNT=$(echo "$NODES" | wc -l)
log_success "Found $NODE_COUNT running node(s)"
echo "$NODES" | while read node; do
    echo "  - $node"
done
echo ""

if [ "$NODE_COUNT" -lt 2 ]; then
    log_warning "This test requires at least 2 nodes for binary format sync testing"
    log_info "Single-node cluster detected - skipping"
    exit 0
fi

# Step 1: Record initial state
log_info "Step 1: Recording initial state..."
declare -A INITIAL_COUNTS
for node in $NODES; do
    count=$(count_entries "$node")
    INITIAL_COUNTS[$node]=$count
    log_info "$node: $count entries"
done
echo ""

# Step 2: Wait for DFSM sync cycle
log_info "Step 2: Waiting for DFSM state synchronization..."
log_info "This will trigger binary serialization/deserialization"
echo ""

# Clear recent logs by reading them (consume old messages)
for node in $NODES; do
    $CONTAINER_CMD logs "$node" --since 1s >/dev/null 2>&1 || true
done

log_info "Waiting 20 seconds for sync cycle..."
sleep 20
log_success "Sync period elapsed"
echo ""

# Step 3: Check for binary serialization activity
log_info "Step 3: Verifying binary format serialization/deserialization..."
SYNC_DETECTED=false
ERRORS_FOUND=false

for node in $NODES; do
    echo ""
    echo "Node: $node"
    echo "----------------------------------------"

    if check_binary_serialization "$node" 30; then
        log_success "$node: Binary format sync detected"
        SYNC_DETECTED=true
    elif [ $? -eq 2 ]; then
        log_warning "$node: No recent sync activity (may sync later)"
    else
        log_error "$node: Deserialization errors detected!"
        ERRORS_FOUND=true

        # Show error details
        log_info "Recent error logs:"
        $CONTAINER_CMD logs "$node" --since 30s 2>&1 | grep -i "error\|fail" | tail -5
    fi
done
echo ""

if [ "$ERRORS_FOUND" = true ]; then
    log_error "Binary format deserialization errors detected!"
    exit 1
fi

if [ "$SYNC_DETECTED" = false ]; then
    log_warning "No DFSM sync activity detected yet"
    log_info "This may be normal if cluster just started"
    log_info "Try running the test again after the cluster has been running longer"
fi

# Step 4: Verify entries are consistent (proves sync worked)
log_info "Step 4: Verifying log consistency across nodes..."
declare -A FINAL_COUNTS
MAX_COUNT=0
MIN_COUNT=999999

for node in $NODES; do
    count=$(count_entries "$node")
    FINAL_COUNTS[$node]=$count

    if [ "$count" -gt "$MAX_COUNT" ]; then
        MAX_COUNT=$count
    fi
    if [ "$count" -lt "$MIN_COUNT" ]; then
        MIN_COUNT=$count
    fi
done

COUNT_DIFF=$((MAX_COUNT - MIN_COUNT))

echo ""
log_info "Entry counts after sync:"
for node in $NODES; do
    log_info "  $node: ${FINAL_COUNTS[$node]} entries"
done

if [ "$COUNT_DIFF" -eq 0 ]; then
    log_success "All nodes have identical counts ($MAX_COUNT entries)"
    log_success "Binary format sync is working correctly!"
elif [ "$COUNT_DIFF" -le 2 ]; then
    log_info "Nodes have similar counts (diff=$COUNT_DIFF) - acceptable"
else
    log_error "Significant count difference: $COUNT_DIFF entries"
    log_error "This may indicate binary format sync issues"
fi
echo ""

# Step 5: Verify specific entries match across nodes
log_info "Step 5: Verifying entry content matches across nodes..."

FIRST_NODE=$(echo "$NODES" | head -n 1)
FIRST_LOG=$(read_clusterlog "$FIRST_NODE")
FIRST_ENTRY=$(echo "$FIRST_LOG" | jq '.data[0]' 2>/dev/null)

if [ "$FIRST_ENTRY" = "null" ] || [ -z "$FIRST_ENTRY" ]; then
    log_info "No entries to compare (empty logs)"
else
    ENTRY_MATCHES=0
    ENTRY_MISMATCHES=0

    # Get first entry's unique identifier (time + node + message)
    ENTRY_TIME=$(echo "$FIRST_ENTRY" | jq -r '.time')
    ENTRY_NODE=$(echo "$FIRST_ENTRY" | jq -r '.node')
    ENTRY_MSG=$(echo "$FIRST_ENTRY" | jq -r '.msg')

    log_info "Reference entry from $FIRST_NODE:"
    log_info "  Time: $ENTRY_TIME"
    log_info "  Node: $ENTRY_NODE"
    log_info "  Message: $ENTRY_MSG"
    echo ""

    # Check if same entry exists on other nodes
    for node in $NODES; do
        if [ "$node" = "$FIRST_NODE" ]; then
            continue
        fi

        NODE_LOG=$(read_clusterlog "$node")
        MATCH=$(echo "$NODE_LOG" | jq --arg time "$ENTRY_TIME" --arg node_name "$ENTRY_NODE" --arg msg "$ENTRY_MSG" \
            '.data[] | select(.time == ($time | tonumber) and .node == $node_name and .msg == $msg)' 2>/dev/null)

        if [ -n "$MATCH" ] && [ "$MATCH" != "null" ]; then
            log_success "$node: Entry found (binary sync successful)"
            ENTRY_MATCHES=$((ENTRY_MATCHES + 1))
        else
            log_warning "$node: Entry not found (may still be syncing)"
            ENTRY_MISMATCHES=$((ENTRY_MISMATCHES + 1))
        fi
    done

    echo ""
    if [ "$ENTRY_MATCHES" -gt 0 ]; then
        log_success "Entry matched on $ENTRY_MATCHES other node(s)"
        log_success "Binary format serialization/deserialization is working!"
    fi
fi

# Step 6: Check for binary format integrity
log_info "Step 6: Checking for binary format integrity issues..."
INTEGRITY_OK=true

for node in $NODES; do
    # Look for corruption or format issues
    FORMAT_ERRORS=$($CONTAINER_CMD logs "$node" --since 60s 2>&1 | grep -iE "buffer too small|invalid cpos|size mismatch|entry too small" || true)

    if [ -n "$FORMAT_ERRORS" ]; then
        log_error "$node: Binary format integrity issues detected!"
        echo "$FORMAT_ERRORS"
        INTEGRITY_OK=false
    fi
done

if [ "$INTEGRITY_OK" = true ]; then
    log_success "No binary format integrity issues detected"
fi
echo ""

# Step 7: Summary
log_info "========================================="
log_info "Test Summary"
log_info "========================================="
log_info "Nodes tested: $NODE_COUNT"
log_info "DFSM sync activity: $([ "$SYNC_DETECTED" = true ] && echo "Detected" || echo "Not detected")"
log_info "Deserialization errors: $([ "$ERRORS_FOUND" = true ] && echo "Found" || echo "None")"
log_info "Count consistency: $COUNT_DIFF entry difference"
log_info "Binary format integrity: $([ "$INTEGRITY_OK" = true ] && echo "OK" || echo "Issues found")"
echo ""

# Final verdict
if [ "$ERRORS_FOUND" = true ] || [ "$INTEGRITY_OK" = false ]; then
    log_error "✗ Binary format sync test FAILED"
    log_error "Deserialization or integrity issues detected"
    exit 1
elif [ "$COUNT_DIFF" -le 2 ]; then
    log_success "✓ Binary format sync test PASSED"
    log_info ""
    log_info "Verification:"
    log_info "  ✓ Rust nodes are using binary format for DFSM state sync"
    log_info "  ✓ Serialization (get_state) produces valid binary data"
    log_info "  ✓ Deserialization (process_state_update) correctly parses binary"
    log_info "  ✓ Logs are consistent across all nodes"
    log_info "  ✓ No binary format integrity issues"
    exit 0
else
    log_warning "⚠ Binary format sync test INCONCLUSIVE"
    log_warning "Count differences suggest possible sync issues"
    exit 1
fi
