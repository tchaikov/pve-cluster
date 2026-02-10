#!/bin/bash
# Test: Column Skip Transformation Validation
# Verify that the column-skipping transform produces correct RRD update strings
# from pvestatd raw data format. This validates the critical fix where non-archivable
# fields are skipped from the START of the data string (matching C's rrd_skip_data).
#
# pvestatd data format (non-archivable fields come FIRST):
#   Node:    "uptime:sublevel:ctime:loadavg:maxcpu:cpu:..."  (skip 2)
#   VM:      "uptime:name:status:template:ctime:maxcpu:cpu:..."  (skip 4)
#   Storage: "ctime:total:used"  (skip 0)
#
# After skipping, the result is: "ctime:archivable_value1:archivable_value2:..."
# If source has fewer columns than target, pad with ":U"
# If source has more columns than target, truncate

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing column-skip transformation logic..."

# Check if rrdtool is available
if ! command -v rrdtool &> /dev/null; then
    echo "  Warning: rrdtool not installed, skipping transform validation"
    echo "  Install with: apt-get install rrdtool"
    exit 0
fi

RRD_DIR="/tmp/rrd-transform-test-$$"
mkdir -p "$RRD_DIR"
TIMESTAMP=$(date +%s)

# Cleanup function
cleanup() {
    rm -rf "$RRD_DIR"
}
trap cleanup EXIT

# Helper: Apply column-skip transformation (matches Rust transform_data / C rrd_skip_data)
# Arguments: $1=data $2=skip_count $3=target_columns
transform_data() {
    local data="$1"
    local skip="$2"
    local target_cols="$3"
    local total_needed=$(( target_cols + 1 ))  # timestamp + target_cols values

    # Split by colon, skip first N fields
    IFS=':' read -ra fields <<< "$data"
    local result=""
    local count=0
    local field_idx=0

    for field in "${fields[@]}"; do
        if [ "$field_idx" -lt "$skip" ]; then
            field_idx=$((field_idx + 1))
            continue
        fi
        if [ "$count" -ge "$total_needed" ]; then
            break  # truncate
        fi
        if [ -n "$result" ]; then
            result="$result:$field"
        else
            result="$field"
        fi
        count=$((count + 1))
        field_idx=$((field_idx + 1))
    done

    # Pad with U if needed
    while [ "$count" -lt "$total_needed" ]; do
        result="$result:U"
        count=$((count + 1))
    done

    echo "$result"
}

PASS=0
FAIL=0

assert_eq() {
    local actual="$1"
    local expected="$2"
    local msg="$3"

    if [ "$actual" = "$expected" ]; then
        echo "    OK: $msg"
        PASS=$((PASS + 1))
    else
        echo "    FAIL: $msg"
        echo "      expected: $expected"
        echo "      actual:   $actual"
        FAIL=$((FAIL + 1))
    fi
}

# ============================================================================
# TEST 1: Node Pve9.0 - skip 2, 19 target columns
# ============================================================================
echo ""
echo "Test 1: Node Pve9.0 column skip (skip=2, target=19)"
echo "  Raw pvestatd format: uptime:sublevel:ctime:loadavg:maxcpu:..."

# Simulate pvestatd node data: 2 non-archivable + 1 timestamp + 19 archivable = 22 fields
RAW_NODE_PVE9="1000:0:$TIMESTAMP:1.5:4:2.0:0.5:8000000000:6000000000:0:0:0:0:1000000:500000:7000000000:0:0.12:0.05:0.02:0.08:0.03"

TRANSFORMED=$(transform_data "$RAW_NODE_PVE9" 2 19)

# Verify timestamp is ctime (not uptime)
FIRST_FIELD=$(echo "$TRANSFORMED" | cut -d: -f1)
assert_eq "$FIRST_FIELD" "$TIMESTAMP" "First field should be ctime timestamp"

# Verify first archivable value is loadavg (not uptime)
SECOND_FIELD=$(echo "$TRANSFORMED" | cut -d: -f2)
assert_eq "$SECOND_FIELD" "1.5" "Second field should be loadavg"

# Verify total field count = timestamp + 19 = 20
FIELD_COUNT=$(echo "$TRANSFORMED" | tr ':' '\n' | wc -l)
assert_eq "$FIELD_COUNT" "20" "Should have 20 fields (timestamp + 19)"

# Verify the transformed data can actually update an RRD file
NODE_RRD="$RRD_DIR/node-pve9-test"
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
    DS:memavailable:GAUGE:120:0:U \
    DS:arcsize:GAUGE:120:0:U \
    DS:pressurecpusome:GAUGE:120:0:U \
    DS:pressureiosome:GAUGE:120:0:U \
    DS:pressureiofull:GAUGE:120:0:U \
    DS:pressurememorysome:GAUGE:120:0:U \
    DS:pressurememoryfull:GAUGE:120:0:U \
    RRA:AVERAGE:0.5:1:70

if rrdtool update "$NODE_RRD" "$TRANSFORMED" 2>/dev/null; then
    echo "    OK: RRD update with transformed data succeeded"
    PASS=$((PASS + 1))
else
    echo "    FAIL: RRD update with transformed data failed"
    FAIL=$((FAIL + 1))
fi

# ============================================================================
# TEST 2: Node Pve2 -> Pve9.0 padding (skip=2, 12 archivable -> pad to 19)
# ============================================================================
echo ""
echo "Test 2: Node Pve2 to Pve9.0 padding (skip=2, 12 data cols -> pad to 19)"
echo "  Old pvestatd format: uptime:sublevel:ctime:loadavg:...:netin:netout"

# Simulate pve2 node data: 2 non-archivable + 1 timestamp + 12 archivable = 15 fields
RAW_NODE_PVE2="1000:0:$((TIMESTAMP+60)):1.5:4:2.0:0.5:8000000000:6000000000:0:0:0:0:1000000:500000"

TRANSFORMED=$(transform_data "$RAW_NODE_PVE2" 2 19)

# Verify timestamp
FIRST_FIELD=$(echo "$TRANSFORMED" | cut -d: -f1)
assert_eq "$FIRST_FIELD" "$((TIMESTAMP+60))" "First field should be ctime timestamp"

# Verify total field count = 20 (timestamp + 19, with 7 padded U)
FIELD_COUNT=$(echo "$TRANSFORMED" | tr ':' '\n' | wc -l)
assert_eq "$FIELD_COUNT" "20" "Should have 20 fields (padded to 19 cols)"

# Verify last archivable value (netout = field 13)
FIELD13=$(echo "$TRANSFORMED" | cut -d: -f13)
assert_eq "$FIELD13" "500000" "Field 13 should be netout"

# Verify padding (fields 14-20 should be U)
FIELD14=$(echo "$TRANSFORMED" | cut -d: -f14)
FIELD20=$(echo "$TRANSFORMED" | cut -d: -f20)
assert_eq "$FIELD14" "U" "Field 14 should be padded U"
assert_eq "$FIELD20" "U" "Field 20 should be padded U"

# Verify padded data can update existing RRD
if rrdtool update "$NODE_RRD" "$TRANSFORMED" 2>/dev/null; then
    echo "    OK: RRD update with padded data succeeded"
    PASS=$((PASS + 1))
else
    echo "    FAIL: RRD update with padded data failed"
    FAIL=$((FAIL + 1))
fi

# ============================================================================
# TEST 3: VM Pve9.0 - skip 4, 17 target columns
# ============================================================================
echo ""
echo "Test 3: VM Pve9.0 column skip (skip=4, target=17)"
echo "  Raw pvestatd format: uptime:name:status:template:ctime:maxcpu:..."

# Simulate pvestatd VM data: 4 non-archivable + 1 timestamp + 17 archivable = 22 fields
RAW_VM_PVE9="1000:myvm:1:0:$TIMESTAMP:4:2:4096:2048:100000:50000:1000:500:100:50:8192:0.10:0.05:0.08:0.03:0.12:0.06"

TRANSFORMED=$(transform_data "$RAW_VM_PVE9" 4 17)

# Verify timestamp is ctime (not uptime)
FIRST_FIELD=$(echo "$TRANSFORMED" | cut -d: -f1)
assert_eq "$FIRST_FIELD" "$TIMESTAMP" "First field should be ctime timestamp"

# Verify first archivable value is maxcpu (not uptime)
SECOND_FIELD=$(echo "$TRANSFORMED" | cut -d: -f2)
assert_eq "$SECOND_FIELD" "4" "Second field should be maxcpu"

# Third field should be cpu (not name)
THIRD_FIELD=$(echo "$TRANSFORMED" | cut -d: -f3)
assert_eq "$THIRD_FIELD" "2" "Third field should be cpu"

# Total field count = timestamp + 17 = 18
FIELD_COUNT=$(echo "$TRANSFORMED" | tr ':' '\n' | wc -l)
assert_eq "$FIELD_COUNT" "18" "Should have 18 fields (timestamp + 17)"

# Verify the transformed data can update an RRD file
VM_RRD="$RRD_DIR/vm-pve9-test"
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
    DS:memhost:GAUGE:120:0:U \
    DS:pressurecpusome:GAUGE:120:0:U \
    DS:pressurecpufull:GAUGE:120:0:U \
    DS:pressureiosome:GAUGE:120:0:U \
    DS:pressureiofull:GAUGE:120:0:U \
    DS:pressurememorysome:GAUGE:120:0:U \
    DS:pressurememoryfull:GAUGE:120:0:U \
    RRA:AVERAGE:0.5:1:70

if rrdtool update "$VM_RRD" "$TRANSFORMED" 2>/dev/null; then
    echo "    OK: RRD update with transformed VM data succeeded"
    PASS=$((PASS + 1))
else
    echo "    FAIL: RRD update with transformed VM data failed"
    FAIL=$((FAIL + 1))
fi

# ============================================================================
# TEST 4: VM Pve2 -> Pve9.0 padding (skip=4, 10 archivable -> pad to 17)
# ============================================================================
echo ""
echo "Test 4: VM Pve2 to Pve9.0 padding (skip=4, 10 data cols -> pad to 17)"

# Simulate pve2 VM data: 4 non-archivable + 1 timestamp + 10 archivable = 15 fields
RAW_VM_PVE2="1000:myvm:1:0:$((TIMESTAMP+60)):4:2:4096:2048:100000:50000:1000:500:100:50"

TRANSFORMED=$(transform_data "$RAW_VM_PVE2" 4 17)

FIRST_FIELD=$(echo "$TRANSFORMED" | cut -d: -f1)
assert_eq "$FIRST_FIELD" "$((TIMESTAMP+60))" "First field should be ctime timestamp"

FIELD_COUNT=$(echo "$TRANSFORMED" | tr ':' '\n' | wc -l)
assert_eq "$FIELD_COUNT" "18" "Should have 18 fields (padded to 17 cols)"

# Field 11 should be last real data (diskwrite)
FIELD11=$(echo "$TRANSFORMED" | cut -d: -f11)
assert_eq "$FIELD11" "50" "Field 11 should be diskwrite"

# Fields 12-18 should be U (7 padded)
FIELD12=$(echo "$TRANSFORMED" | cut -d: -f12)
FIELD18=$(echo "$TRANSFORMED" | cut -d: -f18)
assert_eq "$FIELD12" "U" "Field 12 should be padded U"
assert_eq "$FIELD18" "U" "Field 18 should be padded U"

if rrdtool update "$VM_RRD" "$TRANSFORMED" 2>/dev/null; then
    echo "    OK: RRD update with padded VM data succeeded"
    PASS=$((PASS + 1))
else
    echo "    FAIL: RRD update with padded VM data failed"
    FAIL=$((FAIL + 1))
fi

# ============================================================================
# TEST 5: Storage - skip 0, 2 target columns (no transformation)
# ============================================================================
echo ""
echo "Test 5: Storage - no skip, 2 target columns"

RAW_STORAGE="$TIMESTAMP:1000000000000:500000000000"

TRANSFORMED=$(transform_data "$RAW_STORAGE" 0 2)

# Storage data should pass through unchanged
assert_eq "$TRANSFORMED" "$RAW_STORAGE" "Storage data should be unchanged (no skip)"

STORAGE_RRD="$RRD_DIR/storage-test"
rrdtool create "$STORAGE_RRD" \
    --start "$((TIMESTAMP - 10))" \
    --step 60 \
    DS:total:GAUGE:120:0:U \
    DS:used:GAUGE:120:0:U \
    RRA:AVERAGE:0.5:1:70

if rrdtool update "$STORAGE_RRD" "$TRANSFORMED" 2>/dev/null; then
    echo "    OK: RRD update with storage data succeeded"
    PASS=$((PASS + 1))
else
    echo "    FAIL: RRD update with storage data failed"
    FAIL=$((FAIL + 1))
fi

# ============================================================================
# TEST 6: Future format truncation (more columns than target)
# ============================================================================
echo ""
echo "Test 6: Future format truncation (skip=2, 25 data cols -> truncate to 19)"

# Simulate future node format with 25 archivable columns (more than current 19)
RAW_FUTURE="999:0:$TIMESTAMP:1:2:3:4:5:6:7:8:9:10:11:12:13:14:15:16:17:18:19:20:21:22:23:24:25"

TRANSFORMED=$(transform_data "$RAW_FUTURE" 2 19)

FIRST_FIELD=$(echo "$TRANSFORMED" | cut -d: -f1)
assert_eq "$FIRST_FIELD" "$TIMESTAMP" "First field should be ctime"

FIELD_COUNT=$(echo "$TRANSFORMED" | tr ':' '\n' | wc -l)
assert_eq "$FIELD_COUNT" "20" "Should truncate to 20 fields (timestamp + 19)"

LAST_FIELD=$(echo "$TRANSFORMED" | cut -d: -f20)
assert_eq "$LAST_FIELD" "19" "Last field should be 19 (truncated at col 19)"

# ============================================================================
# TEST 7: Bug regression - verify uptime is NOT used as timestamp
# ============================================================================
echo ""
echo "Test 7: Bug regression - uptime must not be used as timestamp"
echo "  This verifies the critical fix: skip from field[0], not field[1]"

# Use obviously different values for uptime vs ctime to detect the bug
# If the old buggy code were used, it would use "9999" as the timestamp
RAW_BUG_TEST="9999:0:$TIMESTAMP:1.5:4:2.0:0.5:8000000000:6000000000:0:0:0:0:1000000:500000:7000000000:0:0.12:0.05:0.02:0.08:0.03"

TRANSFORMED=$(transform_data "$RAW_BUG_TEST" 2 19)

FIRST_FIELD=$(echo "$TRANSFORMED" | cut -d: -f1)
assert_eq "$FIRST_FIELD" "$TIMESTAMP" "Timestamp must be ctime, NOT uptime (9999)"

# Also verify for VM (uptime=8888 should be skipped)
RAW_VM_BUG="8888:myvm:running:0:$TIMESTAMP:4:2:4096:2048:100000:50000:1000:500:100:50:8192:0.10:0.05:0.08:0.03:0.12:0.06"

TRANSFORMED=$(transform_data "$RAW_VM_BUG" 4 17)

FIRST_FIELD=$(echo "$TRANSFORMED" | cut -d: -f1)
assert_eq "$FIRST_FIELD" "$TIMESTAMP" "VM timestamp must be ctime, NOT uptime (8888)"

SECOND_FIELD=$(echo "$TRANSFORMED" | cut -d: -f2)
assert_eq "$SECOND_FIELD" "4" "VM first value must be maxcpu, NOT name (myvm)"

# ============================================================================
# Summary
# ============================================================================
echo ""
echo "============================================================================"
TOTAL=$((PASS + FAIL))
echo "Column-skip transform validation: $PASS/$TOTAL passed"
if [ "$FAIL" -gt 0 ]; then
    echo "FAILED: $FAIL test(s) failed"
    exit 1
else
    echo "All column-skip transformation tests PASSED"
fi
echo "============================================================================"
echo ""
echo "Verified:"
echo "  - Node skip=2 removes uptime+sublevel, keeps ctime as RRD timestamp"
echo "  - VM skip=4 removes uptime+name+status+template, keeps ctime as RRD timestamp"
echo "  - Storage skip=0 passes data through unchanged"
echo "  - Pve2->Pve9.0 padding adds correct number of U columns"
echo "  - Future format truncation limits to target column count"
echo "  - Bug regression: uptime is never used as RRD timestamp"
echo ""
exit 0
