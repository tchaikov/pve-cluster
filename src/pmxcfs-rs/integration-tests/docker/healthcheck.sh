#!/bin/sh
# Health check script for pmxcfs cluster nodes

# Check if corosync is running
if ! pgrep -x corosync >/dev/null 2>&1; then
    exit 1
fi

# Check if pmxcfs is running
if ! pgrep -x pmxcfs >/dev/null 2>&1; then
    exit 1
fi

# Check if FUSE filesystem is mounted
if [ ! -d /test/pve ]; then
    exit 1
fi

exit 0
