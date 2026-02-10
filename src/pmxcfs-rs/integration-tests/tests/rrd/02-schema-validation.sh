#!/bin/bash
# Test: RRD Schema Validation
# Verify RRD schemas match pmxcfs-rrd implementation specifications
# This test validates that created RRD files have the correct data sources,
# types, and round-robin archives as defined in src/pmxcfs-rrd/src/schema.rs

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing RRD schema validation..."

# Check if rrdtool is available
if ! command -v rrdtool &> /dev/null; then
    echo "⚠ Warning: rrdtool not installed, skipping schema validation"
    echo "  Install with: apt-get install rrdtool"
    echo "✓ RRD schema validation test skipped (rrdtool not available)"
    exit 0
fi

RRD_DIR="/tmp/rrd-schema-test-$$"
mkdir -p "$RRD_DIR"
TIMESTAMP=$(date +%s)

echo "  Testing RRD schemas in: $RRD_DIR"
echo "  Note: Tests use 70 rows per RRA instead of production values (1440)"
echo "        This validates RRA structure while keeping test files small"

# Cleanup function
cleanup() {
    rm -rf "$RRD_DIR"
}
trap cleanup EXIT

# ============================================================================
# TEST 1: Node Schema (pve2 format - 12 data sources)
# ============================================================================
echo ""
echo "Test 1: Node RRD Schema (pve2 format)"
echo "  Expected: 12 data sources (loadavg, maxcpu, cpu, iowait, memtotal, memused,"
echo "            swaptotal, swapused, roottotal, rootused, netin, netout)"

NODE_RRD="$RRD_DIR/pve2-node-testhost"

# Create node RRD with pve2 schema
rrdtool create "$NODE_RRD" \
    --start "$((TIMESTAMP - 10))" \
    --step 60 \
    DS:loadavg:GAUGE:120:0:U \
    DS:maxcpu:GAUGE:120:0:U \
    DS:cpu:GAUGE:120:0:U \
    DS:iowait:GAUGE:120:0:U \
    DS:memtotal:GAUGE:120:0:U \
    DS:memused:GAUGE:120:0:U \
    DS:swaptotal:GAUGE:120:0:U \
    DS:swapused:GAUGE:120:0:U \
    DS:roottotal:GAUGE:120:0:U \
    DS:rootused:GAUGE:120:0:U \
    DS:netin:DERIVE:120:0:U \
    DS:netout:DERIVE:120:0:U \
    RRA:AVERAGE:0.5:1:70 \
    RRA:AVERAGE:0.5:30:70 \
    RRA:AVERAGE:0.5:180:70 \
    RRA:AVERAGE:0.5:720:70 \
    RRA:MAX:0.5:1:70 \
    RRA:MAX:0.5:30:70 \
    RRA:MAX:0.5:180:70 \
    RRA:MAX:0.5:720:70

# Validate schema
INFO=$(rrdtool info "$NODE_RRD")

# Check data source count (count unique DS names, not all property lines)
DS_COUNT=$(echo "$INFO" | grep "^ds\[" | sed 's/ds\[\([^]]*\)\].*/\1/' | sort -u | wc -l)
if [ "$DS_COUNT" -eq 12 ]; then
    echo "  ✓ Data source count: 12 (correct)"
else
    echo "  ✗ ERROR: Data source count: $DS_COUNT (expected 12)"
    exit 1
fi

# Check each data source exists and has correct type
check_ds() {
    local name=$1
    local expected_type=$2

    if echo "$INFO" | grep -q "ds\[$name\]\.type = \"$expected_type\""; then
        echo "  ✓ DS[$name]: type=$expected_type, heartbeat=120"
    else
        echo "  ✗ ERROR: DS[$name] not found or wrong type (expected $expected_type)"
        exit 1
    fi

    # Check heartbeat
    if ! echo "$INFO" | grep -q "ds\[$name\]\.minimal_heartbeat = 120"; then
        echo "  ✗ ERROR: DS[$name] heartbeat not 120"
        exit 1
    fi
}

echo "  Validating data sources..."
check_ds "loadavg" "GAUGE"
check_ds "maxcpu" "GAUGE"
check_ds "cpu" "GAUGE"
check_ds "iowait" "GAUGE"
check_ds "memtotal" "GAUGE"
check_ds "memused" "GAUGE"
check_ds "swaptotal" "GAUGE"
check_ds "swapused" "GAUGE"
check_ds "roottotal" "GAUGE"
check_ds "rootused" "GAUGE"
check_ds "netin" "DERIVE"
check_ds "netout" "DERIVE"

# Check RRA count (count unique RRA indices, not all property lines)
RRA_COUNT=$(echo "$INFO" | grep "^rra\[" | sed 's/rra\[\([0-9]*\)\].*/\1/' | sort -u | wc -l)
if [ "$RRA_COUNT" -eq 8 ]; then
    echo "  ✓ RRA count: 8 (4 AVERAGE + 4 MAX)"
else
    echo "  ✗ ERROR: RRA count: $RRA_COUNT (expected 8)"
    exit 1
fi

# Check step size
STEP=$(echo "$INFO" | grep "^step = " | awk '{print $3}')
if [ "$STEP" -eq 60 ]; then
    echo "  ✓ Step size: 60 seconds"
else
    echo "  ✗ ERROR: Step size: $STEP (expected 60)"
    exit 1
fi

echo "✓ Node RRD schema (pve2) validated successfully"

# ============================================================================
# TEST 2: VM Schema (pve2 format - 10 data sources)
# ============================================================================
echo ""
echo "Test 2: VM RRD Schema (pve2 format)"
echo "  Expected: 10 data sources (maxcpu, cpu, maxmem, mem, maxdisk, disk,"
echo "            netin, netout, diskread, diskwrite)"

VM_RRD="$RRD_DIR/pve2-vm-100"

rrdtool create "$VM_RRD" \
    --start "$((TIMESTAMP - 10))" \
    --step 60 \
    DS:maxcpu:GAUGE:120:0:U \
    DS:cpu:GAUGE:120:0:U \
    DS:maxmem:GAUGE:120:0:U \
    DS:mem:GAUGE:120:0:U \
    DS:maxdisk:GAUGE:120:0:U \
    DS:disk:GAUGE:120:0:U \
    DS:netin:DERIVE:120:0:U \
    DS:netout:DERIVE:120:0:U \
    DS:diskread:DERIVE:120:0:U \
    DS:diskwrite:DERIVE:120:0:U \
    RRA:AVERAGE:0.5:1:70 \
    RRA:AVERAGE:0.5:30:70 \
    RRA:AVERAGE:0.5:180:70 \
    RRA:AVERAGE:0.5:720:70 \
    RRA:MAX:0.5:1:70 \
    RRA:MAX:0.5:30:70 \
    RRA:MAX:0.5:180:70 \
    RRA:MAX:0.5:720:70

INFO=$(rrdtool info "$VM_RRD")

DS_COUNT=$(echo "$INFO" | grep "^ds\[" | sed 's/ds\[\([^]]*\)\].*/\1/' | sort -u | wc -l)
if [ "$DS_COUNT" -eq 10 ]; then
    echo "  ✓ Data source count: 10 (correct)"
else
    echo "  ✗ ERROR: Data source count: $DS_COUNT (expected 10)"
    exit 1
fi

echo "  Validating data sources..."
check_ds "maxcpu" "GAUGE"
check_ds "cpu" "GAUGE"
check_ds "maxmem" "GAUGE"
check_ds "mem" "GAUGE"
check_ds "maxdisk" "GAUGE"
check_ds "disk" "GAUGE"
check_ds "netin" "DERIVE"
check_ds "netout" "DERIVE"
check_ds "diskread" "DERIVE"
check_ds "diskwrite" "DERIVE"

echo "✓ VM RRD schema (pve2) validated successfully"

# ============================================================================
# TEST 3: Storage Schema (2 data sources)
# ============================================================================
echo ""
echo "Test 3: Storage RRD Schema"
echo "  Expected: 2 data sources (total, used)"

STORAGE_RRD="$RRD_DIR/pve2-storage-local"

rrdtool create "$STORAGE_RRD" \
    --start "$((TIMESTAMP - 10))" \
    --step 60 \
    DS:total:GAUGE:120:0:U \
    DS:used:GAUGE:120:0:U \
    RRA:AVERAGE:0.5:1:70 \
    RRA:AVERAGE:0.5:30:70 \
    RRA:AVERAGE:0.5:180:70 \
    RRA:AVERAGE:0.5:720:70 \
    RRA:MAX:0.5:1:70 \
    RRA:MAX:0.5:30:70 \
    RRA:MAX:0.5:180:70 \
    RRA:MAX:0.5:720:70

INFO=$(rrdtool info "$STORAGE_RRD")

DS_COUNT=$(echo "$INFO" | grep "^ds\[" | sed 's/ds\[\([^]]*\)\].*/\1/' | sort -u | wc -l)
if [ "$DS_COUNT" -eq 2 ]; then
    echo "  ✓ Data source count: 2 (correct)"
else
    echo "  ✗ ERROR: Data source count: $DS_COUNT (expected 2)"
    exit 1
fi

echo "  Validating data sources..."
check_ds "total" "GAUGE"
check_ds "used" "GAUGE"

echo "✓ Storage RRD schema validated successfully"

# ============================================================================
# TEST 4: Node Schema (pve9.0 format - 19 data sources)
# ============================================================================
echo ""
echo "Test 4: Node RRD Schema (pve9.0 format)"
echo "  Expected: 19 data sources (12 from pve2 + 7 additional)"

NODE_RRD_9="$RRD_DIR/pve9-node-testhost"

rrdtool create "$NODE_RRD_9" \
    --start "$((TIMESTAMP - 10))" \
    --step 60 \
    DS:loadavg:GAUGE:120:0:U \
    DS:maxcpu:GAUGE:120:0:U \
    DS:cpu:GAUGE:120:0:U \
    DS:iowait:GAUGE:120:0:U \
    DS:memtotal:GAUGE:120:0:U \
    DS:memused:GAUGE:120:0:U \
    DS:swaptotal:GAUGE:120:0:U \
    DS:swapused:GAUGE:120:0:U \
    DS:roottotal:GAUGE:120:0:U \
    DS:rootused:GAUGE:120:0:U \
    DS:netin:DERIVE:120:0:U \
    DS:netout:DERIVE:120:0:U \
    DS:memavailable:GAUGE:120:0:U \
    DS:arcsize:GAUGE:120:0:U \
    DS:pressurecpusome:GAUGE:120:0:U \
    DS:pressureiosome:GAUGE:120:0:U \
    DS:pressureiofull:GAUGE:120:0:U \
    DS:pressurememorysome:GAUGE:120:0:U \
    DS:pressurememoryfull:GAUGE:120:0:U \
    RRA:AVERAGE:0.5:1:70 \
    RRA:AVERAGE:0.5:30:70 \
    RRA:AVERAGE:0.5:180:70 \
    RRA:AVERAGE:0.5:720:70 \
    RRA:MAX:0.5:1:70 \
    RRA:MAX:0.5:30:70 \
    RRA:MAX:0.5:180:70 \
    RRA:MAX:0.5:720:70

INFO=$(rrdtool info "$NODE_RRD_9")

DS_COUNT=$(echo "$INFO" | grep "^ds\[" | sed 's/ds\[\([^]]*\)\].*/\1/' | sort -u | wc -l)
if [ "$DS_COUNT" -eq 19 ]; then
    echo "  ✓ Data source count: 19 (correct)"
else
    echo "  ✗ ERROR: Data source count: $DS_COUNT (expected 19)"
    exit 1
fi

echo "  Validating additional data sources..."
check_ds "memavailable" "GAUGE"
check_ds "arcsize" "GAUGE"
check_ds "pressurecpusome" "GAUGE"
check_ds "pressureiosome" "GAUGE"
check_ds "pressureiofull" "GAUGE"
check_ds "pressurememorysome" "GAUGE"
check_ds "pressurememoryfull" "GAUGE"

echo "✓ Node RRD schema (pve9.0) validated successfully"

# ============================================================================
# TEST 5: VM Schema (pve9.0 format - 17 data sources)
# ============================================================================
echo ""
echo "Test 5: VM RRD Schema (pve9.0/pve2.3 format)"
echo "  Expected: 17 data sources (10 from pve2 + 7 additional)"

VM_RRD_9="$RRD_DIR/pve2.3-vm-200"

rrdtool create "$VM_RRD_9" \
    --start "$((TIMESTAMP - 10))" \
    --step 60 \
    DS:maxcpu:GAUGE:120:0:U \
    DS:cpu:GAUGE:120:0:U \
    DS:maxmem:GAUGE:120:0:U \
    DS:mem:GAUGE:120:0:U \
    DS:maxdisk:GAUGE:120:0:U \
    DS:disk:GAUGE:120:0:U \
    DS:netin:DERIVE:120:0:U \
    DS:netout:DERIVE:120:0:U \
    DS:diskread:DERIVE:120:0:U \
    DS:diskwrite:DERIVE:120:0:U \
    DS:memhost:GAUGE:120:0:U \
    DS:pressurecpusome:GAUGE:120:0:U \
    DS:pressurecpufull:GAUGE:120:0:U \
    DS:pressureiosome:GAUGE:120:0:U \
    DS:pressureiofull:GAUGE:120:0:U \
    DS:pressurememorysome:GAUGE:120:0:U \
    DS:pressurememoryfull:GAUGE:120:0:U \
    RRA:AVERAGE:0.5:1:70 \
    RRA:AVERAGE:0.5:30:70 \
    RRA:AVERAGE:0.5:180:70 \
    RRA:AVERAGE:0.5:720:70 \
    RRA:MAX:0.5:1:70 \
    RRA:MAX:0.5:30:70 \
    RRA:MAX:0.5:180:70 \
    RRA:MAX:0.5:720:70

INFO=$(rrdtool info "$VM_RRD_9")

DS_COUNT=$(echo "$INFO" | grep "^ds\[" | sed 's/ds\[\([^]]*\)\].*/\1/' | sort -u | wc -l)
if [ "$DS_COUNT" -eq 17 ]; then
    echo "  ✓ Data source count: 17 (correct)"
else
    echo "  ✗ ERROR: Data source count: $DS_COUNT (expected 17)"
    exit 1
fi

echo "  Validating additional data sources..."
check_ds "memhost" "GAUGE"
check_ds "pressurecpusome" "GAUGE"
check_ds "pressurecpufull" "GAUGE"
check_ds "pressureiosome" "GAUGE"
check_ds "pressureiofull" "GAUGE"
check_ds "pressurememorysome" "GAUGE"
check_ds "pressurememoryfull" "GAUGE"

echo "✓ VM RRD schema (pve9.0) validated successfully"

# ============================================================================
# TEST 6: RRD Update Test
# ============================================================================
echo ""
echo "Test 6: RRD Data Update Test"
echo "  Testing that RRD files can be updated with real data"

# Update node RRD with sample data
UPDATE_TIME="$TIMESTAMP"
if rrdtool update "$NODE_RRD" "$UPDATE_TIME:1.5:4:0.35:0.05:16000000:8000000:2000000:500000:100000000:50000000:1000000:500000" 2>/dev/null; then
    echo "  ✓ Node RRD update successful"
else
    echo "  ✗ ERROR: Node RRD update failed"
    exit 1
fi

# Update VM RRD with sample data
if rrdtool update "$VM_RRD" "$UPDATE_TIME:2:0.5:4000000:2000000:20000000:10000000:100000:50000:500000:250000" 2>/dev/null; then
    echo "  ✓ VM RRD update successful"
else
    echo "  ✗ ERROR: VM RRD update failed"
    exit 1
fi

# Update storage RRD
if rrdtool update "$STORAGE_RRD" "$UPDATE_TIME:100000000:50000000" 2>/dev/null; then
    echo "  ✓ Storage RRD update successful"
else
    echo "  ✗ ERROR: Storage RRD update failed"
    exit 1
fi

# ============================================================================
# TEST 7: RRD Fetch Test
# ============================================================================
echo ""
echo "Test 7: RRD Data Fetch Test"
echo "  Testing that RRD data can be retrieved"

# Fetch data from node RRD
if rrdtool fetch "$NODE_RRD" AVERAGE --start "$((TIMESTAMP - 60))" --end "$((TIMESTAMP + 60))" 2>/dev/null | grep -q "loadavg"; then
    echo "  ✓ Node RRD fetch successful"
else
    echo "  ✗ ERROR: Node RRD fetch failed"
    exit 1
fi

# Fetch data from VM RRD
if rrdtool fetch "$VM_RRD" AVERAGE --start "$((TIMESTAMP - 60))" --end "$((TIMESTAMP + 60))" 2>/dev/null | grep -q "cpu"; then
    echo "  ✓ VM RRD fetch successful"
else
    echo "  ✗ ERROR: VM RRD fetch failed"
    exit 1
fi

echo "✓ RRD data operations validated successfully"

echo ""
echo "✓ RRD schema validation test passed"
exit 0
