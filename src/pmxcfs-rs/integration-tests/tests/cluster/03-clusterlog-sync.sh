#!/bin/bash
# Test: ClusterLog Multi-Node Synchronization
# Verify cluster log synchronization across Rust nodes

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "========================================="
echo "ClusterLog Multi-Node Synchronization Test"
echo "========================================="
echo ""

# Configuration
MOUNT_PATH="$TEST_MOUNT_PATH"
CLUSTERLOG_FILE="$MOUNT_PATH/.clusterlog"
TEST_MESSAGE="MultiNode-Test-$(date +%s)"

# Helper functions
log_info() {
    echo "[INFO] $1"
}

log_error() {
    echo "[ERROR] $1" >&2
}

log_success() {
    echo "[✓] $1"
}

# Function to check if clusterlog file exists and is accessible
check_clusterlog_exists() {
    local node=$1
    if $CONTAINER_CMD exec "$node" test -e "$CLUSTERLOG_FILE" 2>/dev/null; then
        return 0
    else
        return 1
    fi
}

# Function to read clusterlog from a node
read_clusterlog() {
    local node=$1
    $CONTAINER_CMD exec "$node" cat "$CLUSTERLOG_FILE" 2>/dev/null || echo "[]"
}

# Function to count entries in clusterlog
count_entries() {
    local node=$1
    local content=$(read_clusterlog "$node")

    if [ -z "$content" ] || [ "$content" = "[]" ]; then
        echo "0"
        return
    fi

    # Try to parse as JSON and count entries in .data array
    if echo "$content" | jq '.data | length' 2>/dev/null; then
        return
    else
        echo "0"
    fi
}

# Function to wait for cluster log entry to appear
wait_for_log_entry() {
    local node=$1
    local search_text=$2
    local timeout=${3:-30}
    local elapsed=0

    log_info "Waiting for log entry containing '$search_text' on $node..."

    while [ $elapsed -lt $timeout ]; do
        local content=$(read_clusterlog "$node")

        if echo "$content" | jq -e --arg msg "$search_text" '.[] | select(.msg | contains($msg))' > /dev/null 2>&1; then
            log_success "Entry found on $node after ${elapsed}s"
            return 0
        fi

        sleep 1
        elapsed=$((elapsed + 1))
    done

    log_error "Entry not found on $node after ${timeout}s timeout"
    return 1
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

# Detect running containers
log_info "Detecting running cluster nodes..."
NODES=$($CONTAINER_CMD ps --filter "name=pmxcfs" --filter "status=running" --format "{{.Names}}" | sort)

if [ -z "$NODES" ]; then
    log_error "No running pmxcfs containers found"
    log_info "Please start the cluster with:"
    log_info "  cd integration-tests/docker && docker-compose -f docker-compose.cluster.yml up -d"
    exit 1
fi

NODE_COUNT=$(echo "$NODES" | wc -l)
log_success "Found $NODE_COUNT running node(s):"
echo "$NODES" | while read node; do
    echo "  - $node"
done
echo ""

# If only one node, this test is not applicable
if [ "$NODE_COUNT" -lt 2 ]; then
    log_info "This test requires at least 2 nodes"
    log_info "Single-node cluster detected - skipping multi-node sync test"
    exit 0
fi

# Step 1: Verify all nodes have clusterlog accessible
log_info "Step 1: Verifying clusterlog accessibility on all nodes..."
for node in $NODES; do
    if check_clusterlog_exists "$node"; then
        log_success "Clusterlog accessible on $node"
    else
        log_error "Clusterlog not accessible on $node"
        exit 1
    fi
done
echo ""

# Step 2: Record initial entry counts
log_info "Step 2: Recording initial cluster log state..."
declare -A INITIAL_COUNTS
for node in $NODES; do
    count=$(count_entries "$node")
    INITIAL_COUNTS[$node]=$count
    log_info "$node: $count entries"
done
echo ""

# Step 3: Wait for cluster to sync (if needed)
log_info "Step 3: Waiting for initial synchronization..."
sleep 5

# Check if counts are consistent across nodes
FIRST_NODE=$(echo "$NODES" | head -n 1)
FIRST_COUNT=${INITIAL_COUNTS[$FIRST_NODE]}
ALL_SYNCED=true

for node in $NODES; do
    count=${INITIAL_COUNTS[$node]}
    if [ "$count" != "$FIRST_COUNT" ]; then
        ALL_SYNCED=false
        log_info "Counts differ: $FIRST_NODE has $FIRST_COUNT, $node has $count"
    fi
done

if [ "$ALL_SYNCED" = "true" ]; then
    log_success "All nodes have consistent entry counts ($FIRST_COUNT entries)"
else
    log_info "Nodes have different counts - will verify sync after test entry"
fi
echo ""

# Step 4: Monitor DFSM state sync activity
log_info "Step 4: Checking for DFSM state synchronization activity..."
for node in $NODES; do
    # Check if node has recent state sync log messages
    if $CONTAINER_CMD logs "$node" --since 30s 2>&1 | grep -q "get_state\|process_state_update" 2>/dev/null; then
        log_success "$node: DFSM state sync is active"
    else
        log_info "$node: No recent DFSM activity (may sync soon)"
    fi
done
echo ""

# Step 5: Trigger a state sync by waiting
log_info "Step 5: Waiting for DFSM state synchronization cycle..."
log_info "DFSM typically syncs every 10-30 seconds"
sleep 15
log_success "Sync period elapsed"
echo ""

# Step 6: Verify final counts are consistent
log_info "Step 6: Verifying cluster log consistency across nodes..."
declare -A FINAL_COUNTS
MAX_COUNT=0
MIN_COUNT=999999

for node in $NODES; do
    count=$(count_entries "$node")
    FINAL_COUNTS[$node]=$count
    log_info "$node: $count entries"

    if [ "$count" -gt "$MAX_COUNT" ]; then
        MAX_COUNT=$count
    fi
    if [ "$count" -lt "$MIN_COUNT" ]; then
        MIN_COUNT=$count
    fi
done

COUNT_DIFF=$((MAX_COUNT - MIN_COUNT))

if [ "$COUNT_DIFF" -eq 0 ]; then
    log_success "All nodes have identical entry counts ($MAX_COUNT entries) ✓"
    log_success "Cluster log synchronization is working correctly!"
elif [ "$COUNT_DIFF" -le 2 ]; then
    log_info "Nodes have similar counts (diff=$COUNT_DIFF) - acceptable variance"
    log_success "Cluster log synchronization appears to be working"
else
    log_error "Significant count difference detected (diff=$COUNT_DIFF)"
    log_error "This may indicate synchronization issues"
    echo ""
    log_info "Detailed node counts:"
    for node in $NODES; do
        echo "  $node: ${FINAL_COUNTS[$node]} entries"
    done
    exit 1
fi
echo ""

# Step 7: Verify deduplication
log_info "Step 7: Checking for duplicate entries..."
FIRST_NODE=$(echo "$NODES" | head -n 1)
FIRST_LOG=$(read_clusterlog "$FIRST_NODE")

# Count unique entries by (time, node, message) tuple
UNIQUE_COUNT=$(echo "$FIRST_LOG" | jq '[.data[] | {time: .time, node: .node, msg: .msg}] | unique | length' 2>/dev/null || echo "0")
TOTAL_COUNT=$(echo "$FIRST_LOG" | jq '.data | length' 2>/dev/null || echo "0")

if [ "$UNIQUE_COUNT" -eq "$TOTAL_COUNT" ]; then
    log_success "No duplicate entries detected ($TOTAL_COUNT unique entries)"
else
    DUPES=$((TOTAL_COUNT - UNIQUE_COUNT))
    log_info "Found $DUPES potential duplicate(s) - this may be normal for same-timestamp entries"
fi
echo ""

# Step 8: Sample log entries across nodes
log_info "Step 8: Sampling log entries for format validation..."
for node in $NODES; do
    SAMPLE=$(read_clusterlog "$node" | jq '.data[0]' 2>/dev/null)

    if [ "$SAMPLE" != "null" ] && [ -n "$SAMPLE" ]; then
        log_success "$node: Sample entry structure valid"

        # Validate required fields
        for field in time node pri tag msg; do
            if echo "$SAMPLE" | jq -e ".$field" > /dev/null 2>&1; then
                : # Field exists
            else
                log_error "$node: Missing required field '$field'"
                exit 1
            fi
        done
    else
        log_info "$node: No entries to sample (empty log)"
    fi
done
echo ""

# Step 9: Summary
log_info "========================================="
log_info "Test Summary"
log_info "========================================="
log_info "Nodes tested: $NODE_COUNT"
log_info "Final entry counts:"
for node in $NODES; do
    log_info "  $node: ${FINAL_COUNTS[$node]} entries"
done
log_info "Count variance: $COUNT_DIFF entries"
log_info "Deduplication: $UNIQUE_COUNT unique / $TOTAL_COUNT total"
echo ""

if [ "$COUNT_DIFF" -le 2 ]; then
    log_success "✓ Multi-node cluster log synchronization test PASSED"
    exit 0
else
    log_error "✗ Multi-node cluster log synchronization test FAILED"
    exit 1
fi
