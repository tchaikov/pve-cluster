#!/bin/bash
# Unified test runner for pmxcfs integration tests
# Consolidates all test execution into a single script with subsystem filtering

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Configuration
SKIP_BUILD=${SKIP_BUILD:-false}
USE_PODMAN=${USE_PODMAN:-false}
SUBSYSTEM=${SUBSYSTEM:-all}
MODE=${MODE:-single}  # single, cluster, or mixed

# Detect container runtime - prefer podman
if command -v podman &> /dev/null; then
    CONTAINER_CMD="podman"
    COMPOSE_CMD="podman-compose"
elif command -v docker &> /dev/null; then
    CONTAINER_CMD="docker"
    COMPOSE_CMD="docker compose"
else
    echo -e "${RED}ERROR: Neither docker nor podman found${NC}"
    exit 1
fi

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --subsystem)
            SUBSYSTEM="$2"
            shift 2
            ;;
        --cluster)
            MODE="cluster"
            shift
            ;;
        --mixed)
            MODE="mixed"
            shift
            ;;
        --single|--single-node)
            MODE="single"
            shift
            ;;
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --help|-h)
            cat << EOF
Usage: $0 [OPTIONS]

Run pmxcfs integration tests organized by subsystem.

OPTIONS:
    --subsystem <name>   Run tests for specific subsystem
                         Options: core, fuse, memdb, ipc, rrd, status, locks,
                                  plugins, clusterlog, cluster, dfsm, all
                         Default: all

    --single            Run single-node tests only (default)
    --cluster           Run multi-node cluster tests
    --mixed             Run mixed C/Rust cluster tests

    --skip-build        Skip rebuilding pmxcfs binary

    --help, -h          Show this help message

SUBSYSTEMS:
    core        - Basic daemon functionality, paths
    fuse        - FUSE filesystem operations
    memdb       - Database access and operations
    ipc         - Socket and IPC communication
    rrd         - RRD file creation and metrics (NEW)
    status      - Status tracking and VM registry (NEW)
    locks       - Lock management and concurrent access (NEW)
    plugins     - Plugin file access and validation (NEW)
    clusterlog  - Cluster log functionality (NEW)
    cluster     - Multi-node cluster operations (requires --cluster)
    dfsm        - DFSM synchronization protocol (requires --cluster)
    mixed-cluster - Mixed C/Rust cluster compatibility (requires --mixed)
    all         - Run all applicable tests (default)

ENVIRONMENT VARIABLES:
    SKIP_BUILD=true     Skip build step
    USE_PODMAN=true     Use podman instead of docker

EXAMPLES:
    # Run all single-node tests
    $0

    # Run only FUSE tests
    $0 --subsystem fuse

    # Run DFSM cluster tests
    $0 --subsystem dfsm --cluster

    # Run all cluster tests without rebuilding
    SKIP_BUILD=true $0 --cluster

    # Run mixed C/Rust cluster tests
    $0 --mixed

EOF
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

echo -e "${CYAN}======== pmxcfs Integration Test Suite ==========${NC}"
echo ""
echo "Mode:       $MODE"
echo "Subsystem:  $SUBSYSTEM"
echo "Container:  $CONTAINER_CMD"
echo ""

# Build pmxcfs if needed
if [ "$SKIP_BUILD" != true ]; then
    echo -e "${BLUE}Building Rust pmxcfs...${NC}"
    cd "$PROJECT_ROOT"
    if ! cargo build --release; then
        echo -e "${RED}ERROR: Failed to build Rust pmxcfs${NC}"
        exit 1
    fi
    echo -e "${GREEN}✓ Rust pmxcfs built successfully${NC}"
    echo ""

    # Build C pmxcfs if running mixed mode tests
    if [ "$MODE" = "mixed" ]; then
        echo -e "${BLUE}Building C pmxcfs for mixed cluster testing...${NC}"
        C_PMXCFS_DIR="$(cd "$PROJECT_ROOT/../pmxcfs" && pwd)"

        if [ ! -d "$C_PMXCFS_DIR" ]; then
            echo -e "${RED}ERROR: C pmxcfs directory not found at $C_PMXCFS_DIR${NC}"
            echo -e "${YELLOW}Mixed cluster tests require the C implementation${NC}"
            exit 1
        fi

        cd "$C_PMXCFS_DIR"
        if ! make pmxcfs; then
            echo -e "${RED}ERROR: Failed to build C pmxcfs${NC}"
            echo -e "${YELLOW}Required packages: libcpg-dev libcorosync-common-dev libqb-dev libglib2.0-dev libfuse-dev libsqlite3-dev librrd-dev${NC}"
            exit 1
        fi
        echo -e "${GREEN}✓ C pmxcfs built successfully${NC}"
        echo ""
        cd "$PROJECT_ROOT"
    fi
fi

# Check Rust binary exists
if [ ! -f "$PROJECT_ROOT/target/release/pmxcfs" ]; then
    echo -e "${RED}ERROR: Rust pmxcfs binary not found${NC}"
    exit 1
fi

# Check C binary exists for mixed mode
if [ "$MODE" = "mixed" ]; then
    C_PMXCFS_BIN="$(cd "$PROJECT_ROOT/../pmxcfs" && pwd)/pmxcfs"
    if [ ! -f "$C_PMXCFS_BIN" ]; then
        echo -e "${RED}ERROR: C pmxcfs binary not found at $C_PMXCFS_BIN${NC}"
        echo -e "${YELLOW}Mixed cluster tests require the C implementation to be built${NC}"
        echo -e "${YELLOW}Run: cd ../pmxcfs && make pmxcfs${NC}"
        exit 1
    fi
    echo -e "${GREEN}✓ C pmxcfs binary found${NC}"
    echo ""
fi

# Determine compose file and test directory
if [ "$MODE" = "cluster" ]; then
    COMPOSE_FILE="docker-compose.cluster.yml"
elif [ "$MODE" = "mixed" ]; then
    COMPOSE_FILE="docker-compose.mixed.yml"
else
    COMPOSE_FILE="docker-compose.yml"
fi

# Change to docker directory for podman-compose compatibility
# (podman-compose 1.3.0 has issues with relative paths when using -f flag)
DOCKER_DIR="$SCRIPT_DIR/docker"
cd "$DOCKER_DIR"

# Map subsystem to test directories
get_test_dirs() {
    case "$SUBSYSTEM" in
        core)
            echo "tests/core"
            ;;
        fuse)
            echo "tests/fuse"
            ;;
        memdb)
            echo "tests/memdb"
            ;;
        ipc)
            echo "tests/ipc"
            ;;
        rrd)
            echo "tests/rrd"
            ;;
        status)
            echo "tests/status"
            ;;
        locks)
            echo "tests/locks"
            ;;
        plugins)
            echo "tests/plugins"
            ;;
        clusterlog)
            echo "tests/clusterlog"
            ;;
        cluster)
            if [ "$MODE" != "cluster" ]; then
                echo -e "${YELLOW}WARNING: cluster subsystem requires --cluster mode${NC}"
                exit 1
            fi
            echo "tests/cluster"
            ;;
        dfsm)
            if [ "$MODE" != "cluster" ]; then
                echo -e "${YELLOW}WARNING: dfsm subsystem requires --cluster mode${NC}"
                exit 1
            fi
            echo "tests/dfsm"
            ;;
        mixed-cluster)
            if [ "$MODE" != "mixed" ]; then
                echo -e "${YELLOW}WARNING: mixed-cluster subsystem requires --mixed mode${NC}"
                exit 1
            fi
            echo "tests/mixed-cluster"
            ;;
        all)
            if [ "$MODE" = "cluster" ]; then
                echo "tests/cluster tests/dfsm"
            elif [ "$MODE" = "mixed" ]; then
                echo "tests/mixed-cluster"
            else
                echo "tests/core tests/fuse tests/memdb tests/ipc tests/rrd tests/status tests/locks tests/plugins tests/clusterlog"
            fi
            ;;
        *)
            echo -e "${RED}ERROR: Unknown subsystem: $SUBSYSTEM${NC}"
            exit 1
            ;;
    esac
}

TEST_DIRS=$(get_test_dirs)

# Clean up previous runs
echo -e "${BLUE}Cleaning up previous containers...${NC}"
$COMPOSE_CMD -f $COMPOSE_FILE down -v 2>/dev/null || true
echo ""

# Start containers
echo -e "${BLUE}Starting containers (mode: $MODE)...${NC}"
# Note: Removed --build flag to use cached images. Rebuild manually if needed:
#   cd docker && podman-compose build
$COMPOSE_CMD -f $COMPOSE_FILE up -d

if [ "$MODE" = "cluster" ] || [ "$MODE" = "mixed" ]; then
    # Determine container name prefix
    if [ "$MODE" = "mixed" ]; then
        CONTAINER_PREFIX="pmxcfs-mixed"
    else
        CONTAINER_PREFIX="pmxcfs-cluster"
    fi

    # Wait for cluster to be healthy
    echo "Waiting for cluster nodes to become healthy..."
    HEALTHY=0
    for i in {1..60}; do
        HEALTHY=0
        for node in node1 node2 node3; do
            # For mixed cluster, node3 (C) uses /etc/pve, others use /test/pve
            if [ "$MODE" = "mixed" ] && [ "$node" = "node3" ]; then
                # C pmxcfs uses /etc/pve
                if $CONTAINER_CMD exec ${CONTAINER_PREFIX}-$node sh -c 'pgrep pmxcfs > /dev/null && test -d /etc/pve' 2>/dev/null; then
                    HEALTHY=$((HEALTHY + 1))
                fi
            else
                # Rust pmxcfs uses /test/pve
                if $CONTAINER_CMD exec ${CONTAINER_PREFIX}-$node sh -c 'pgrep pmxcfs > /dev/null && test -d /test/pve' 2>/dev/null; then
                    HEALTHY=$((HEALTHY + 1))
                fi
            fi
        done

        if [ $HEALTHY -eq 3 ]; then
            echo -e "${GREEN}✓ All 3 nodes are healthy${NC}"
            break
        fi

        echo "  Waiting... ($HEALTHY/3 nodes ready) - attempt $i/60"
        sleep 2
    done

    if [ $HEALTHY -ne 3 ]; then
        echo -e "${RED}ERROR: Not all nodes became healthy${NC}"
        $COMPOSE_CMD -f $COMPOSE_FILE logs
        $COMPOSE_CMD -f $COMPOSE_FILE down -v
        exit 1
    fi

    # Wait for corosync to stabilize
    if [ "$MODE" = "mixed" ]; then
        # Mixed clusters need extra time for C/Rust DFSM cross-sync to complete
        echo "Waiting for DFSM to complete initial synchronization..."
        sleep 10
    else
        sleep 5
    fi
else
    # Wait for single node
    echo "Waiting for node to become healthy..."
    NODE_HEALTHY=false
    for i in {1..30}; do
        if $CONTAINER_CMD exec pmxcfs-test sh -c 'pgrep pmxcfs > /dev/null && test -d /test/pve' 2>/dev/null; then
            echo -e "${GREEN}✓ Node is healthy${NC}"
            NODE_HEALTHY=true
            break
        fi
        echo "  Waiting... - attempt $i/30"
        sleep 2
    done

    if [ "$NODE_HEALTHY" = false ]; then
        echo -e "${RED}ERROR: Node did not become healthy${NC}"
        echo "Container logs:"
        $CONTAINER_CMD logs pmxcfs-test 2>&1 || echo "Failed to get container logs"
        $COMPOSE_CMD -f $COMPOSE_FILE down -v
        exit 1
    fi
fi

echo ""

# Run tests
TOTAL=0
PASSED=0
FAILED=0

echo -e "${CYAN}═══════════════════════════════════════════════${NC}"
echo -e "${CYAN}   Running Tests: $SUBSYSTEM${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════${NC}"
echo ""

# Create results directory
mkdir -p "$SCRIPT_DIR/results"
RESULTS_FILE="$SCRIPT_DIR/results/test-results_$(date +%Y%m%d_%H%M%S).log"

# Run tests from each directory
for test_dir in $TEST_DIRS; do
    # Convert to absolute path from SCRIPT_DIR
    ABS_TEST_DIR="$SCRIPT_DIR/$test_dir"

    if [ ! -d "$ABS_TEST_DIR" ]; then
        continue
    fi

    SUBSYS_NAME=$(basename "$test_dir")
    echo -e "${BLUE}━━━ Subsystem: $SUBSYS_NAME ━━━${NC}" | tee -a "$RESULTS_FILE"
    echo ""

    for test_script in "$ABS_TEST_DIR"/*.sh; do
        if [ ! -f "$test_script" ]; then
            continue
        fi

        TEST_NAME=$(basename "$test_script")
        echo "Running: $TEST_NAME" | tee -a "$RESULTS_FILE"

        TOTAL=$((TOTAL + 1))

        # Get path for container (under /workspace)
        REL_PATH="src/pmxcfs-rs/integration-tests/tests/$(basename "$test_dir")/$(basename "$test_script")"

        if [ "$MODE" = "cluster" ]; then
            # Run cluster tests from inside node1 (has access to cluster network)
            # Use pipefail to get exit code from test script, not tee
            set -o pipefail
            if $CONTAINER_CMD exec \
                -e NODE1_IP=172.20.0.11 \
                -e NODE2_IP=172.20.0.12 \
                -e NODE3_IP=172.20.0.13 \
                -e CONTAINER_CMD=$CONTAINER_CMD \
                pmxcfs-cluster-node1 bash "/workspace/$REL_PATH" 2>&1 | tee -a "$RESULTS_FILE"; then
                echo -e "${GREEN}✓ PASS${NC}" | tee -a "$RESULTS_FILE"
                PASSED=$((PASSED + 1))
            else
                echo -e "${RED}✗ FAIL${NC}" | tee -a "$RESULTS_FILE"
                FAILED=$((FAILED + 1))
            fi
            set +o pipefail
        elif [ "$MODE" = "mixed" ]; then
            # Run mixed cluster tests from HOST (not inside container)
            # These tests orchestrate across multiple containers using docker/podman exec
            # They don't need cluster network access, they need container runtime access
            set -o pipefail
            if NODE1_IP=172.21.0.11 NODE2_IP=172.21.0.12 NODE3_IP=172.21.0.13 \
                CONTAINER_CMD=$CONTAINER_CMD \
                bash "$test_script" 2>&1 | tee -a "$RESULTS_FILE"; then
                echo -e "${GREEN}✓ PASS${NC}" | tee -a "$RESULTS_FILE"
                PASSED=$((PASSED + 1))
            else
                echo -e "${RED}✗ FAIL${NC}" | tee -a "$RESULTS_FILE"
                FAILED=$((FAILED + 1))
            fi
            set +o pipefail
        else
            # Run single-node tests inside container
            # Use pipefail to get exit code from test script, not tee
            set -o pipefail
            if $CONTAINER_CMD exec pmxcfs-test bash "/workspace/$REL_PATH" 2>&1 | tee -a "$RESULTS_FILE"; then
                echo -e "${GREEN}✓ PASS${NC}" | tee -a "$RESULTS_FILE"
                PASSED=$((PASSED + 1))
            else
                echo -e "${RED}✗ FAIL${NC}" | tee -a "$RESULTS_FILE"
                FAILED=$((FAILED + 1))
                # Capture container logs on failure
                echo -e "${YELLOW}Capturing container logs...${NC}" | tee -a "$RESULTS_FILE"
                $CONTAINER_CMD logs pmxcfs-test 2>&1 | tail -100 | tee -a "$RESULTS_FILE"
            fi
            set +o pipefail
        fi
        echo ""
    done
done

# Cleanup
echo -e "${BLUE}Cleaning up containers...${NC}"
$COMPOSE_CMD -f $COMPOSE_FILE down -v

# Summary
echo ""
echo -e "${CYAN}═══════════════════════════════════════════════${NC}"
echo -e "${CYAN}   Test Summary${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════${NC}"
echo "Total tests:   $TOTAL"
echo -e "Passed:        ${GREEN}$PASSED${NC}"
echo -e "Failed:        ${RED}$FAILED${NC}"
echo ""
echo "Results saved to: $RESULTS_FILE"
echo ""

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}✓ All tests passed!${NC}"
    exit 0
else
    echo -e "${RED}✗ Some tests failed${NC}"
    exit 1
fi
