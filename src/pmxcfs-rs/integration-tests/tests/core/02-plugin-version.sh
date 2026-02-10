#!/bin/bash
# Test: Plugin .version
# Verify .version plugin returns valid data

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing .version plugin..."

VERSION_FILE="$PLUGIN_VERSION"

# Check file exists
if [ ! -f "$VERSION_FILE" ]; then
    echo "ERROR: .version plugin not found"
    exit 1
fi
echo "✓ .version file exists"

# Read content
CONTENT=$(cat "$VERSION_FILE")
if [ -z "$CONTENT" ]; then
    echo "ERROR: .version returned empty content"
    exit 1
fi
echo "✓ .version readable"

# Verify it's JSON
if ! echo "$CONTENT" | jq . &> /dev/null; then
    echo "ERROR: .version is not valid JSON"
    echo "Content: $CONTENT"
    exit 1
fi
echo "✓ .version is valid JSON"

# Check required fields exist
REQUIRED_FIELDS=("version" "cluster")
for field in "${REQUIRED_FIELDS[@]}"; do
    if ! echo "$CONTENT" | jq -e ".$field" &> /dev/null; then
        echo "ERROR: Missing required field: $field"
        echo "Content: $CONTENT"
        exit 1
    fi
done

# Validate version format (should be semver like "9.0.6")
VERSION=$(echo "$CONTENT" | jq -r '.version')
if ! echo "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "ERROR: Invalid version format: $VERSION (expected X.Y.Z)"
    exit 1
fi
echo "✓ Version format valid: $VERSION"

# Validate cluster.nodes is a positive number
if echo "$CONTENT" | jq -e '.cluster.nodes' &> /dev/null; then
    NODES=$(echo "$CONTENT" | jq -r '.cluster.nodes')
    if ! [[ "$NODES" =~ ^[0-9]+$ ]] || [ "$NODES" -lt 1 ]; then
        echo "ERROR: cluster.nodes should be positive integer, got: $NODES"
        exit 1
    fi
    echo "✓ Cluster nodes: $NODES"
fi

# Validate cluster.quorate is 0 or 1
if echo "$CONTENT" | jq -e '.cluster.quorate' &> /dev/null; then
    QUORATE=$(echo "$CONTENT" | jq -r '.cluster.quorate')
    if ! [[ "$QUORATE" =~ ^[01]$ ]]; then
        echo "ERROR: cluster.quorate should be 0 or 1, got: $QUORATE"
        exit 1
    fi
    echo "✓ Cluster quorate: $QUORATE"
fi

# Validate cluster.name is non-empty
if echo "$CONTENT" | jq -e '.cluster.name' &> /dev/null; then
    CLUSTER_NAME=$(echo "$CONTENT" | jq -r '.cluster.name')
    if [ -z "$CLUSTER_NAME" ] || [ "$CLUSTER_NAME" = "null" ]; then
        echo "ERROR: cluster.name should not be empty"
        exit 1
    fi
    echo "✓ Cluster name: $CLUSTER_NAME"
fi

echo "✓ .version plugin functional and validated"
exit 0
