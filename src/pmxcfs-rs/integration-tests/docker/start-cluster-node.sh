#!/bin/bash
set -e

# Determine which pmxcfs binary to use (rust or c)
# Default to rust for backward compatibility
PMXCFS_TYPE="${PMXCFS_TYPE:-rust}"

echo "Starting cluster node: ${NODE_NAME:-unknown} (ID: ${NODE_ID:-1}, Type: $PMXCFS_TYPE)"

# Initialize corosync.conf from template if not exists
if [ ! -f /etc/corosync/corosync.conf ]; then
    echo "Initializing corosync configuration from template..."

    # Use CLUSTER_TYPE environment variable to select template
    if [ -z "$CLUSTER_TYPE" ]; then
        echo "ERROR: CLUSTER_TYPE environment variable not set"
        echo "Please set CLUSTER_TYPE to either 'cluster' or 'mixed'"
        exit 1
    fi

    echo "Using CLUSTER_TYPE=$CLUSTER_TYPE to select template"
    if [ "$CLUSTER_TYPE" = "mixed" ]; then
        echo "Using mixed cluster configuration (172.21.0.0/16)"
        cp /workspace/src/pmxcfs-rs/integration-tests/docker/lib/corosync.conf.mixed.template /etc/corosync/corosync.conf
    elif [ "$CLUSTER_TYPE" = "cluster" ]; then
        echo "Using standard cluster configuration (172.20.0.0/16)"
        cp /workspace/src/pmxcfs-rs/integration-tests/docker/lib/corosync.conf.template /etc/corosync/corosync.conf
    else
        echo "ERROR: Invalid CLUSTER_TYPE=$CLUSTER_TYPE"
        echo "Must be either 'cluster' or 'mixed'"
        exit 1
    fi
fi

# Create authkey if not exists (shared across all nodes via volume)
if [ ! -f /etc/corosync/authkey ]; then
    echo "pmxcfs-test-cluster-2025" | sha256sum | awk '{print $1}' > /etc/corosync/authkey
    chmod 400 /etc/corosync/authkey
fi

# Start corosync in background
echo "Starting corosync..."
corosync -f &
COROSYNC_PID=$!

# Wait for corosync to initialize (reduced from 3s to 1s)
sleep 1

# Check corosync status
if corosync-quorumtool -s; then
    echo "Corosync cluster is operational"
else
    echo "Corosync started, waiting for quorum..."
fi

# Select pmxcfs binary based on PMXCFS_TYPE
if [ "$PMXCFS_TYPE" = "c" ]; then
    echo "Starting C pmxcfs..."
    PMXCFS_BIN="/workspace/src/pmxcfs/pmxcfs"
    PMXCFS_ARGS="-f -d"  # C pmxcfs uses different argument format

    # C pmxcfs uses /etc/pve as default mount point
    if [ ! -d "/etc/pve" ]; then
        mkdir -p /etc/pve
    fi

    if [ ! -x "$PMXCFS_BIN" ]; then
        echo "ERROR: C pmxcfs binary not found or not executable at $PMXCFS_BIN"
        echo "Please ensure the C binary is built and available in the workspace"
        exit 1
    fi

    # Run C pmxcfs in foreground
    # Set up signal forwarding to ensure SIGTERM reaches pmxcfs
    trap 'kill -TERM $PMXCFS_PID 2>/dev/null' TERM INT
    "$PMXCFS_BIN" $PMXCFS_ARGS &
    PMXCFS_PID=$!
    wait $PMXCFS_PID
else
    echo "Starting Rust pmxcfs..."
    export RUST_BACKTRACE=1
    PMXCFS_BIN="/workspace/src/pmxcfs-rs/target/release/pmxcfs"

    if [ ! -x "$PMXCFS_BIN" ]; then
        echo "ERROR: Rust pmxcfs binary not found or not executable at $PMXCFS_BIN"
        exit 1
    fi

    # Bootstrap corosync.conf into pmxcfs database BEFORE starting pmxcfs
    # This allows pmxcfs to start directly in cluster mode without restart
    if [ -f /etc/corosync/corosync.conf ] && [ ! -f /test/db/config.db ]; then
        echo "Pre-bootstrapping corosync.conf for cluster mode..."
        # Create database directory
        mkdir -p /test/db
        # Import corosync.conf by copying to a temporary location that pmxcfs will import
        # pmxcfs will import /etc/corosync/corosync.conf on first start if database is new
        echo "✓ Database directory prepared for corosync.conf import"
    fi

    # Run Rust pmxcfs in foreground
    # Set up signal forwarding to ensure SIGTERM reaches pmxcfs
    trap 'kill -TERM $PMXCFS_PID 2>/dev/null' TERM INT
    "$PMXCFS_BIN" --foreground --test-dir /test &
    PMXCFS_PID=$!
    wait $PMXCFS_PID
fi
