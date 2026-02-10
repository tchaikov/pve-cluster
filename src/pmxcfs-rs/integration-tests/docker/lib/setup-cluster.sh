#!/bin/bash
# Setup corosync cluster for pmxcfs testing
# Run this on each container node to enable cluster sync

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== Setting up Corosync Cluster ==="

# Check if running in container
if [ ! -f /.dockerenv ] && ! grep -q docker /proc/1/cgroup 2>/dev/null; then
    echo "WARNING: Not running in container"
fi

# Get node ID from environment or hostname
NODE_ID=${NODE_ID:-1}
NODE_NAME=${NODE_NAME:-$(hostname)}

echo "Node: $NODE_NAME (ID: $NODE_ID)"

# Create corosync directories
mkdir -p /etc/corosync /var/log/corosync

# Copy corosync configuration
if [ -f "$SCRIPT_DIR/corosync.conf.template" ]; then
    cp "$SCRIPT_DIR/corosync.conf.template" /etc/corosync/corosync.conf
    echo "✓ Corosync configuration installed"
else
    echo "ERROR: corosync.conf.template not found"
    exit 1
fi

# Create authkey (same for all nodes)
if [ ! -f /etc/corosync/authkey ]; then
    # Generate or use pre-shared authkey
    # For testing, we use a fixed key (in production, generate securely)
    echo "test-cluster-key-$(date +%Y%m%d)" | sha256sum | cut -d' ' -f1 > /etc/corosync/authkey
    chmod 400 /etc/corosync/authkey
    echo "✓ Corosync authkey created"
fi

# Start corosync (if installed)
if command -v corosync &> /dev/null; then
    echo "Starting corosync..."
    corosync -f &
    COROSYNC_PID=$!
    echo "✓ Corosync started (PID: $COROSYNC_PID)"

    # Wait for corosync to be ready
    sleep 2

    # Check corosync status
    if corosync-quorumtool -s &> /dev/null; then
        echo "✓ Corosync cluster is operational"
        corosync-quorumtool -s
    else
        echo "⚠ Corosync started but quorum not reached yet"
    fi
else
    echo "⚠ Corosync not installed, skipping cluster setup"
    echo "Install with: apt-get install corosync corosync-qdevice"
fi

echo ""
echo "Cluster setup complete!"
echo "Next: Start pmxcfs with cluster mode (remove --test-dir)"
