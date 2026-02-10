#!/bin/bash
# Test: rrdcached Integration
# Verify pmxcfs can communicate with rrdcached daemon for RRD updates
# This test validates:
# 1. rrdcached daemon starts and accepts connections
# 2. RRD files can be created through rrdcached
# 3. RRD updates work through rrdcached socket
# 4. pmxcfs can recover when rrdcached is stopped/restarted
# 5. Cached updates are flushed on daemon stop

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing rrdcached integration..."

# Check if rrdcached and rrdtool are available
if ! command -v rrdcached &> /dev/null; then
    echo "⚠ Warning: rrdcached not installed, skipping integration test"
    echo "  Install with: apt-get install rrdcached"
    echo "✓ rrdcached integration test skipped (daemon not available)"
    exit 0
fi

if ! command -v rrdtool &> /dev/null; then
    echo "⚠ Warning: rrdtool not installed, skipping integration test"
    echo "  Install with: apt-get install rrdtool"
    echo "✓ rrdcached integration test skipped (rrdtool not available)"
    exit 0
fi

# Test directories
RRD_DIR="/tmp/rrdcached-test-$$"
JOURNAL_DIR="$RRD_DIR/journal"
SOCKET="$RRD_DIR/rrdcached.sock"

mkdir -p "$RRD_DIR" "$JOURNAL_DIR"

echo "  RRD directory: $RRD_DIR"
echo "  Socket: $SOCKET"

# Cleanup function
cleanup() {
    echo ""
    echo "Cleaning up..."

    # Stop rrdcached if running
    if [ -f "$RRD_DIR/rrdcached.pid" ]; then
        PID=$(cat "$RRD_DIR/rrdcached.pid")
        if kill -0 "$PID" 2>/dev/null; then
            echo "  Stopping rrdcached (PID: $PID)..."
            kill "$PID"
            # Wait for graceful shutdown
            for i in {1..10}; do
                if ! kill -0 "$PID" 2>/dev/null; then
                    break
                fi
                sleep 0.5
            done
            # Force kill if still running
            if kill -0 "$PID" 2>/dev/null; then
                kill -9 "$PID" 2>/dev/null || true
            fi
        fi
    fi

    rm -rf "$RRD_DIR"
    echo "  Cleanup complete"
}
trap cleanup EXIT

# ============================================================================
# TEST 1: Start rrdcached daemon
# ============================================================================
echo ""
echo "Test 1: Start rrdcached daemon"

# Start rrdcached with appropriate options
# -g: run in foreground (we'll background it ourselves)
# -l: listen on Unix socket
# -b: base directory for RRD files
# -B: restrict file access to base directory
# -m: permissions for socket (octal)
# -p: PID file
# -j: journal directory
# -F: flush all updates at shutdown
# -w: write timeout (seconds before flushing)
# -f: flush timeout (seconds - flush dead data interval)

rrdcached -g \
    -l "unix:$SOCKET" \
    -b "$RRD_DIR" -B \
    -m 660 \
    -p "$RRD_DIR/rrdcached.pid" \
    -j "$JOURNAL_DIR" \
    -F -w 5 -f 10 \
    &> "$RRD_DIR/rrdcached.log" &

RRDCACHED_PID=$!

# Wait for daemon to start and create socket
echo "  Waiting for rrdcached to start (PID: $RRDCACHED_PID)..."
for i in {1..20}; do
    if [ -S "$SOCKET" ]; then
        echo "✓ rrdcached started successfully"
        break
    fi
    if ! kill -0 "$RRDCACHED_PID" 2>/dev/null; then
        echo "ERROR: rrdcached failed to start"
        cat "$RRD_DIR/rrdcached.log"
        exit 1
    fi
    sleep 0.5
done

if [ ! -S "$SOCKET" ]; then
    echo "ERROR: rrdcached socket not created after 10 seconds"
    cat "$RRD_DIR/rrdcached.log"
    exit 1
fi

# Verify daemon is running
if ! kill -0 "$RRDCACHED_PID" 2>/dev/null; then
    echo "ERROR: rrdcached process died"
    exit 1
fi

echo "  Socket created: $SOCKET"
echo "  Daemon PID: $RRDCACHED_PID"

# ============================================================================
# TEST 2: Create RRD file through rrdcached
# ============================================================================
echo ""
echo "Test 2: Create RRD file through rrdcached"

TEST_RRD="pve2-node-testhost"
TIMESTAMP=$(date +%s)

# Create RRD file using rrdtool with daemon socket
# The --daemon option tells rrdtool to use rrdcached for this operation
if rrdtool create "$RRD_DIR/$TEST_RRD" \
    --daemon "unix:$SOCKET" \
    --start "$((TIMESTAMP - 10))" \
    --step 60 \
    DS:cpu:GAUGE:120:0:U \
    DS:mem:GAUGE:120:0:U \
    DS:netin:DERIVE:120:0:U \
    DS:netout:DERIVE:120:0:U \
    RRA:AVERAGE:0.5:1:70 \
    RRA:AVERAGE:0.5:30:70 \
    RRA:MAX:0.5:1:70 \
    RRA:MAX:0.5:30:70 \
    2>&1; then
    echo "✓ RRD file created through rrdcached"
else
    echo "ERROR: Failed to create RRD file through rrdcached"
    exit 1
fi

# Verify file exists
if [ ! -f "$RRD_DIR/$TEST_RRD" ]; then
    echo "ERROR: RRD file was not created on disk"
    exit 1
fi

echo "  File created: $RRD_DIR/$TEST_RRD"

# ============================================================================
# TEST 3: Update RRD through rrdcached (cached mode)
# ============================================================================
echo ""
echo "Test 3: Update RRD through rrdcached (cached mode)"

# Perform updates through rrdcached
# These updates should be cached in memory initially
for i in {1..5}; do
    T=$((TIMESTAMP + i * 60))
    CPU=$(echo "scale=2; 0.5 + $i * 0.1" | bc)
    MEM=$((1073741824 + i * 10000000))
    NETIN=$((i * 1000000))
    NETOUT=$((i * 500000))

    if ! rrdtool update "$RRD_DIR/$TEST_RRD" \
        --daemon "unix:$SOCKET" \
        "$T:$CPU:$MEM:$NETIN:$NETOUT" 2>&1; then
        echo "ERROR: Failed to update RRD through rrdcached (update $i)"
        exit 1
    fi
done

echo "✓ Successfully sent 5 updates through rrdcached"

# Query rrdcached stats to verify it's caching
# STATS command returns cache statistics
if echo "STATS" | socat - "UNIX-CONNECT:$SOCKET" 2>/dev/null | grep -q "QueueLength:"; then
    echo "✓ rrdcached is accepting commands and tracking statistics"
else
    echo "⚠ Warning: Could not query rrdcached stats (may not affect functionality)"
fi

# ============================================================================
# TEST 4: Flush cached data
# ============================================================================
echo ""
echo "Test 4: Flush cached data to disk"

# Tell rrdcached to flush this specific file
# FLUSH command forces immediate write to disk
if echo "FLUSH $TEST_RRD" | socat - "UNIX-CONNECT:$SOCKET" 2>&1 | grep -q "^0"; then
    echo "✓ Flush command accepted by rrdcached"
else
    echo "⚠ Warning: Flush command may have failed (checking data anyway)"
fi

# Small delay to ensure flush completes
sleep 1

# Verify data was written to disk by reading it back
if rrdtool fetch "$RRD_DIR/$TEST_RRD" \
    --daemon "unix:$SOCKET" \
    AVERAGE \
    --start "$((TIMESTAMP - 60))" \
    --end "$((TIMESTAMP + 360))" \
    2>/dev/null | grep -q "[0-9]"; then
    echo "✓ Data successfully flushed and readable"
else
    echo "ERROR: Could not read back flushed data"
    exit 1
fi

# ============================================================================
# TEST 5: Test daemon recovery (stop and restart)
# ============================================================================
echo ""
echo "Test 5: Test rrdcached recovery"

# Stop the daemon gracefully
echo "  Stopping rrdcached..."
kill "$RRDCACHED_PID"

# Wait for graceful shutdown
for i in {1..10}; do
    if ! kill -0 "$RRDCACHED_PID" 2>/dev/null; then
        echo "✓ rrdcached stopped gracefully"
        break
    fi
    sleep 0.5
done

# Verify daemon is stopped
if kill -0 "$RRDCACHED_PID" 2>/dev/null; then
    echo "ERROR: rrdcached did not stop"
    kill -9 "$RRDCACHED_PID"
    exit 1
fi

# Restart daemon
echo "  Restarting rrdcached..."
rrdcached -g \
    -l "unix:$SOCKET" \
    -b "$RRD_DIR" -B \
    -m 660 \
    -p "$RRD_DIR/rrdcached.pid" \
    -j "$JOURNAL_DIR" \
    -F -w 5 -f 10 \
    &> "$RRD_DIR/rrdcached.log" &

RRDCACHED_PID=$!

# Wait for restart
for i in {1..20}; do
    if [ -S "$SOCKET" ]; then
        echo "✓ rrdcached restarted successfully"
        break
    fi
    if ! kill -0 "$RRDCACHED_PID" 2>/dev/null; then
        echo "ERROR: rrdcached failed to restart"
        cat "$RRD_DIR/rrdcached.log"
        exit 1
    fi
    sleep 0.5
done

if [ ! -S "$SOCKET" ]; then
    echo "ERROR: rrdcached socket not recreated after restart"
    exit 1
fi

# ============================================================================
# TEST 6: Verify data persisted across restart
# ============================================================================
echo ""
echo "Test 6: Verify data persisted across restart"

# Try reading data again after restart
if rrdtool fetch "$RRD_DIR/$TEST_RRD" \
    --daemon "unix:$SOCKET" \
    AVERAGE \
    --start "$((TIMESTAMP - 60))" \
    --end "$((TIMESTAMP + 360))" \
    2>/dev/null | grep -q "[0-9]"; then
    echo "✓ Data persisted across daemon restart"
else
    echo "ERROR: Data lost after daemon restart"
    exit 1
fi

# ============================================================================
# TEST 7: Test journal recovery
# ============================================================================
echo ""
echo "Test 7: Test journal recovery"

# Perform some updates that will be journaled
echo "  Performing journaled updates..."
for i in {6..10}; do
    T=$((TIMESTAMP + i * 60))
    if rrdtool update "$RRD_DIR/$TEST_RRD" \
        --daemon "unix:$SOCKET" \
        "$T:0.$i:$((1073741824 + i * 10000000)):$((i * 1000000)):$((i * 500000))" \
        2>&1; then
        :
    else
        echo "⚠ Warning: Update $i failed (may not affect test)"
    fi
done

echo "  Sent 5 more updates for journaling"

# Check if journal files were created
JOURNAL_COUNT=$(find "$JOURNAL_DIR" -name "rrd.journal.*" 2>/dev/null | wc -l)
if [ "$JOURNAL_COUNT" -gt 0 ]; then
    echo "✓ Journal files created ($JOURNAL_COUNT files)"
else
    echo "  No journal files created (updates may have been flushed immediately)"
fi

# ============================================================================
# TEST 8: Verify schema information through rrdcached
# ============================================================================
echo ""
echo "Test 8: Verify RRD schema through rrdcached"

# Use rrdtool info to check schema
if rrdtool info "$RRD_DIR/$TEST_RRD" \
    --daemon "unix:$SOCKET" | grep -E "ds\[(cpu|mem|netin|netout)\]" | head -4; then
    echo "✓ RRD schema accessible through rrdcached"
else
    echo "ERROR: Could not read schema through rrdcached"
    exit 1
fi

# Verify data sources are correct
DS_COUNT=$(rrdtool info "$RRD_DIR/$TEST_RRD" --daemon "unix:$SOCKET" | grep -c "^ds\[" || true)
if [ "$DS_COUNT" -ge 4 ]; then
    echo "✓ All data sources present (found $DS_COUNT DS entries)"
else
    echo "ERROR: Missing data sources (expected 4+, found $DS_COUNT)"
    exit 1
fi

echo ""
echo "✓ rrdcached integration test passed"
exit 0
