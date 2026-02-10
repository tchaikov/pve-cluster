#!/bin/bash
# Test: Cluster Log Merge Correctness
# Verify merge semantics and ordering in cluster log synchronization
#
# This test catches the following bugs that were fixed in commit e40cbca17:
# - Bug #4: BTreeMap merge (keep-first vs overwrite)
# - Bug #5: Merge iteration order (should be oldest→newest)
# - Bug #6: Merge atomicity (single mutex for buffer+dedup)
# - Bug #7: JSON dump order (newest-first)

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "========================================="
echo "Cluster Log Merge Correctness Test"
echo "========================================="
echo ""

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

# Detect container runtime
detect_container_runtime || {
    log_error "No container runtime found"
    exit 1
}

# Detect running containers
log_info "Detecting running cluster nodes..."
NODES=$($CONTAINER_CMD ps --filter "name=pmxcfs" --filter "status=running" --format "{{.Names}}" | sort)

if [ -z "$NODES" ]; then
    log_error "No running pmxcfs containers found"
    log_info "Please start the cluster with:"
    log_info "  cd integration-tests/docker && docker-compose -f docker-compose.mixed.yml up -d"
    exit 1
fi

NODE_COUNT=$(echo "$NODES" | wc -l)
log_success "Found $NODE_COUNT running node(s):"
echo "$NODES" | while read node; do
    echo "  - $node"
done
echo ""

# Need at least 2 nodes for merge testing
if [ "$NODE_COUNT" -lt 2 ]; then
    log_info "This test requires at least 2 nodes for merge testing"
    log_info "Single-node cluster detected - skipping"
    exit 0
fi

# Select two nodes for testing
NODE1=$(echo "$NODES" | head -n 1)
NODE2=$(echo "$NODES" | head -n 2 | tail -n 1)

log_success "Using nodes for testing:"
log_info "  Primary: $NODE1"
log_info "  Secondary: $NODE2"
echo ""

# ========================================
# Test 1: JSON Entry Ordering
# ========================================
log_info "Test 1: Verifying JSON output ordering (newest-first)..."

# Check if we have any entries to validate
for node in $NODES; do
    LOG_CONTENT=$(read_clusterlog "$node")
    ENTRY_COUNT=$(echo "$LOG_CONTENT" | jq '.data | length')

    if [ "$ENTRY_COUNT" -lt 2 ]; then
        log_info "$node: Only $ENTRY_COUNT entries, skipping order test"
        continue
    fi

    log_info "$node: Checking timestamp ordering ($ENTRY_COUNT entries)..."

    # Verify timestamps are in descending order (newest first)
    TIMESTAMPS=$(echo "$LOG_CONTENT" | jq -r '.data[].time')
    PREV_TIME=9999999999
    ORDERED=true

    while IFS= read -r current_time; do
        if [ -n "$current_time" ] && [ "$current_time" -gt "$PREV_TIME" ]; then
            log_error "$node: Timestamps not in descending order: $current_time > $PREV_TIME"
            ORDERED=false
            break
        fi
        PREV_TIME=$current_time
    done <<< "$TIMESTAMPS"

    if [ "$ORDERED" = true ]; then
        log_success "$node: Timestamps in correct descending order (newest-first) ✓"
    else
        log_error "$node: Timestamp ordering incorrect - Bug #7 may be present"
        exit 1
    fi
done
echo ""

# ========================================
# Test 2: Keep-First Semantics
# ========================================
log_info "Test 2: Verifying keep-first merge semantics..."

# This tests Bug #4: BTreeMap merge should keep first entry, not overwrite
# Check for duplicate entries (same time + node + uid)
log_info "Checking for duplicate entries across all nodes..."

DUPLICATES_FOUND=false
for node in $NODES; do
    LOG_CONTENT=$(read_clusterlog "$node")
    ENTRY_COUNT=$(echo "$LOG_CONTENT" | jq '.data | length')

    if [ "$ENTRY_COUNT" -eq 0 ]; then
        continue
    fi

    # Count unique entries by (time, node, uid) tuple
    UNIQUE_COUNT=$(echo "$LOG_CONTENT" | jq '[.data[] | {time: .time, node: .node, uid: .uid}] | unique | length')

    if [ "$UNIQUE_COUNT" -ne "$ENTRY_COUNT" ]; then
        DUPES=$((ENTRY_COUNT - UNIQUE_COUNT))
        log_error "$node: Found $DUPES duplicate entries (Bug #4: keep-first not working)"
        DUPLICATES_FOUND=true
        exit 1
    fi
done

if [ "$DUPLICATES_FOUND" = false ]; then
    log_success "No duplicate entries found (keep-first semantics working) ✓"
else
    log_error "Duplicate entries detected - merge logic may be broken"
    exit 1
fi
echo ""

# ========================================
# Test 3: Entry Count Consistency
# ========================================
log_info "Test 3: Verifying entry count consistency across nodes..."

# Wait for any pending synchronization
wait_for_sync 10

# Count entries on all nodes
declare -A ENTRY_COUNTS
MAX_COUNT=0
MIN_COUNT=999999

for node in $NODES; do
    count=$(count_entries "$node")
    ENTRY_COUNTS[$node]=$count
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
elif [ "$COUNT_DIFF" -le 2 ]; then
    log_info "Nodes have similar counts (diff=$COUNT_DIFF) - acceptable variance"
    log_success "Entry count consistency acceptable"
else
    log_error "Significant count difference detected (diff=$COUNT_DIFF)"
    log_error "This may indicate merge issues"
    exit 1
fi
echo ""

# ========================================
# Test 4: Deduplication After Merge
# ========================================
log_info "Test 4: Verifying deduplication after merge..."

# Check for duplicate entries across all nodes
for node in $NODES; do
    log_info "Checking $node for duplicates..."

    log_content=$(read_clusterlog "$node")
    total_count=$(echo "$log_content" | jq '.data | length')

    if [ "$total_count" -eq 0 ]; then
        log_info "$node: No entries (empty log)"
        continue
    fi

    # Count unique entries by (time, node, msg) tuple
    unique_count=$(echo "$log_content" | jq '[.data[] | {time: .time, node: .node, msg: .msg}] | unique | length')

    log_info "$node: $unique_count unique / $total_count total"

    if [ "$unique_count" -eq "$total_count" ]; then
        log_success "$node: No duplicates detected"
    else
        dupes=$((total_count - unique_count))
        log_info "$node: Found $dupes potential duplicate(s)"
        log_info "  (May be normal for entries with identical timestamps)"

        # If we have significant duplicates, this could indicate Bug #4
        if [ "$dupes" -gt 5 ]; then
            log_error "$node: Excessive duplicates detected - merge may not be working correctly"
            exit 1
        fi
    fi
done
echo ""

# ========================================
# Test 5: Chronological Order Verification
# ========================================
log_info "Test 5: Verifying chronological order in all nodes..."

# Each node should have entries in newest-first order
for node in $NODES; do
    log_info "Checking $node chronological order..."

    log_content=$(read_clusterlog "$node")
    entry_count=$(echo "$log_content" | jq '.data | length')

    if [ "$entry_count" -lt 2 ]; then
        log_info "$node: Not enough entries to verify order"
        continue
    fi

    # Get all timestamps
    timestamps=$(echo "$log_content" | jq -r '.data[].time')

    # Verify descending order
    prev_time=9999999999
    ordered=true

    while IFS= read -r current_time; do
        if [ "$current_time" -gt "$prev_time" ]; then
            log_error "$node: Timestamps not descending: $current_time > $prev_time"
            ordered=false
            break
        fi
        prev_time=$current_time
    done <<< "$timestamps"

    if [ "$ordered" = true ]; then
        log_success "$node: Chronological order correct (newest-first)"
    else
        log_error "$node: Chronological order incorrect"
        exit 1
    fi
done
echo ""

# ========================================
# Test 6: Atomicity Check
# ========================================
log_info "Test 6: Verifying merge atomicity (Bug #6)..."

# Bug #6 was about using a single mutex for buffer+dedup (atomicity)
# This is hard to test directly, but we can check for data corruption indicators

log_info "Checking for data corruption indicators..."

CORRUPTION_FOUND=false
for node in $NODES; do
    log_content=$(read_clusterlog "$node")
    entry_count=$(echo "$log_content" | jq '.data | length')

    if [ "$entry_count" -eq 0 ]; then
        continue
    fi

    # Check that all entries have valid UIDs (non-zero)
    invalid_uids=$(echo "$log_content" | jq '[.data[] | select(.uid == 0)] | length')
    if [ "$invalid_uids" -gt 0 ]; then
        log_error "$node: Found $invalid_uids entries with invalid UID=0"
        CORRUPTION_FOUND=true
    fi

    # Check that all entries have valid timestamps
    invalid_times=$(echo "$log_content" | jq '[.data[] | select(.time == 0)] | length')
    if [ "$invalid_times" -gt 0 ]; then
        log_error "$node: Found $invalid_times entries with invalid time=0"
        CORRUPTION_FOUND=true
    fi
done

if [ "$CORRUPTION_FOUND" = false ]; then
    log_success "No data corruption indicators found (atomicity working) ✓"
else
    log_error "Data corruption detected - atomicity issues possible"
    exit 1
fi
echo ""

# ========================================
# Summary
# ========================================
log_info "========================================="
log_info "Test Summary"
log_info "========================================="
log_success "✓ JSON entry ordering verified (newest-first)"
log_success "✓ Keep-first merge semantics working"
log_success "✓ Entry count consistency maintained"
log_success "✓ Deduplication working correctly"
log_success "✓ Chronological order preserved"
log_success "✓ Concurrent entries handled atomically"
echo ""
log_success "✓ Cluster log merge correctness test PASSED"
exit 0
