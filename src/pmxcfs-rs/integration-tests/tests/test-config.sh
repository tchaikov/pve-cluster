#!/bin/bash
# Common test configuration
# Source this file at the beginning of each test script

# Test directory paths (set by --test-dir flag to pmxcfs)
# Default: /test (in container), but configurable for different environments
TEST_DIR="${TEST_DIR:-/test}"

# Derived paths based on TEST_DIR
TEST_DB_PATH="${TEST_DB_PATH:-$TEST_DIR/db/config.db}"
TEST_DB_DIR="${TEST_DB_DIR:-$TEST_DIR/db}"
TEST_MOUNT_PATH="${TEST_MOUNT_PATH:-$TEST_DIR/pve}"
TEST_RUN_DIR="${TEST_RUN_DIR:-$TEST_DIR/run}"
TEST_RRD_DIR="${TEST_RRD_DIR:-$TEST_DIR/rrd}"
TEST_ETC_DIR="${TEST_ETC_DIR:-$TEST_DIR/etc}"
TEST_COROSYNC_DIR="${TEST_COROSYNC_DIR:-$TEST_DIR/etc/corosync}"

# Socket paths
TEST_SOCKET="${TEST_SOCKET:-@pve2}"  # Abstract socket
TEST_SOCKET_PATH="${TEST_SOCKET_PATH:-$TEST_RUN_DIR/pmxcfs.sock}"

# PID file
TEST_PID_FILE="${TEST_PID_FILE:-$TEST_RUN_DIR/pmxcfs.pid}"

# Plugin file paths (in FUSE mount)
PLUGIN_VERSION="${PLUGIN_VERSION:-$TEST_MOUNT_PATH/.version}"
PLUGIN_MEMBERS="${PLUGIN_MEMBERS:-$TEST_MOUNT_PATH/.members}"
PLUGIN_VMLIST="${PLUGIN_VMLIST:-$TEST_MOUNT_PATH/.vmlist}"
PLUGIN_RRD="${PLUGIN_RRD:-$TEST_MOUNT_PATH/.rrd}"
PLUGIN_CLUSTERLOG="${PLUGIN_CLUSTERLOG:-$TEST_MOUNT_PATH/.clusterlog}"
PLUGIN_DEBUG="${PLUGIN_DEBUG:-$TEST_MOUNT_PATH/.debug}"

# Export for subprocesses
export TEST_DIR
export TEST_DB_PATH
export TEST_DB_DIR
export TEST_MOUNT_PATH
export TEST_RUN_DIR
export TEST_RRD_DIR
export TEST_ETC_DIR
export TEST_COROSYNC_DIR
export TEST_SOCKET
export TEST_SOCKET_PATH
export TEST_PID_FILE
export PLUGIN_VERSION
export PLUGIN_MEMBERS
export PLUGIN_VMLIST
export PLUGIN_RRD
export PLUGIN_CLUSTERLOG
export PLUGIN_DEBUG

# Helper function to get test script directory
get_test_dir() {
    cd "$(dirname "${BASH_SOURCE[1]}")" && pwd
}

# Helper function for temporary test files
make_test_file() {
    local prefix="${1:-test}"
    echo "$TEST_MOUNT_PATH/.${prefix}-$$-$(date +%s)"
}

# Helper function to check if running in test mode
is_test_mode() {
    [ -d "$TEST_MOUNT_PATH" ] && [ -f "$TEST_DB_PATH" ]
}

# Verify test environment is set up
verify_test_environment() {
    local errors=0

    if [ ! -d "$TEST_DIR" ]; then
        echo "ERROR: Test directory not found: $TEST_DIR" >&2
        ((errors++))
    fi

    if [ ! -d "$TEST_MOUNT_PATH" ]; then
        echo "ERROR: FUSE mount path not found: $TEST_MOUNT_PATH" >&2
        ((errors++))
    fi

    if [ ! -f "$TEST_DB_PATH" ]; then
        echo "ERROR: Database not found: $TEST_DB_PATH" >&2
        ((errors++))
    fi

    return $errors
}

# ========================================
# Helper functions for multi-node testing
# ========================================

# Detect container runtime (podman or docker)
detect_container_runtime() {
    if [ -n "$CONTAINER_CMD" ]; then
        # Already set by environment
        return 0
    elif command -v podman &> /dev/null; then
        export CONTAINER_CMD="podman"
    elif command -v docker &> /dev/null; then
        export CONTAINER_CMD="docker"
    else
        echo "ERROR: Neither podman nor docker found" >&2
        return 1
    fi
}

# Read clusterlog from any node
# Usage: read_clusterlog <container_name>
# Returns: JSON content of .clusterlog
read_clusterlog() {
    local container_name=$1

    detect_container_runtime || return 1

    # Determine mount path based on node type
    # C nodes use /etc/pve, Rust nodes use /test/pve
    if [[ "$container_name" == *"node3"* ]] || [[ "$container_name" == *"-c-"* ]]; then
        # C node
        $CONTAINER_CMD exec "$container_name" cat /etc/pve/.clusterlog 2>/dev/null || echo '{"data":[]}'
    else
        # Rust node
        $CONTAINER_CMD exec "$container_name" cat "$TEST_MOUNT_PATH/.clusterlog" 2>/dev/null || echo '{"data":[]}'
    fi
}

# Read binary state from database
# Usage: read_binary_state <container_name>
# Returns: Binary state (raw bytes)
read_binary_state() {
    local container_name=$1

    detect_container_runtime || return 1

    # Determine database path based on node type
    if [[ "$container_name" == *"node3"* ]] || [[ "$container_name" == *"-c-"* ]]; then
        # C node uses /var/lib/pve-cluster/config.db
        $CONTAINER_CMD exec "$container_name" cat /var/lib/pve-cluster/config.db 2>/dev/null
    else
        # Rust node uses TEST_DB_PATH
        $CONTAINER_CMD exec "$container_name" cat "$TEST_DB_PATH" 2>/dev/null
    fi
}

# Trigger a log entry on a node
# Usage: trigger_log_entry <container_name> <message> [tag]
# Note: Cluster log entries are triggered by memdb changes, not syslog
trigger_log_entry() {
    local container_name=$1
    local message=$2
    local tag="${3:-test}"

    detect_container_runtime || return 1

    # Cluster log entries are triggered by file modifications in pmxcfs
    # Create/modify a test file to trigger record_memdb_change()
    local timestamp=$(date +%s)
    local test_file="test-trigger-${timestamp}-${RANDOM}.tmp"

    # Determine mount path based on node type
    if [[ "$container_name" == *"node3"* ]] || [[ "$container_name" == *"-c-"* ]]; then
        # C node uses /etc/pve
        $CONTAINER_CMD exec "$container_name" sh -c "echo '$message' > /etc/pve/$test_file 2>/dev/null || true"
        sleep 0.5
        $CONTAINER_CMD exec "$container_name" rm -f "/etc/pve/$test_file" 2>/dev/null || true
    else
        # Rust node uses /test/pve
        $CONTAINER_CMD exec "$container_name" sh -c "echo '$message' > /test/pve/$test_file 2>/dev/null || true"
        sleep 0.5
        $CONTAINER_CMD exec "$container_name" rm -f "/test/pve/$test_file" 2>/dev/null || true
    fi
}

# Wait for DFSM synchronization
# Usage: wait_for_sync [seconds]
wait_for_sync() {
    local seconds=${1:-15}
    sleep "$seconds"
}

# Count entries in clusterlog
# Usage: count_entries <container_name>
count_entries() {
    local container_name=$1
    local content=$(read_clusterlog "$container_name")

    if [ -z "$content" ] || [ "$content" = '{"data":[]}' ]; then
        echo "0"
        return
    fi

    # Parse JSON and count entries in .data array
    echo "$content" | jq '.data | length' 2>/dev/null || echo "0"
}
