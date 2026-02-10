#!/bin/bash
# Test runner for C tests inside container
# This script runs inside the container with all dependencies available

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}╔════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║  Running C Tests Against Rust pmxcfs (In Container)       ║${NC}"
echo -e "${BLUE}╚════════════════════════════════════════════════════════════╝${NC}"
echo ""

# Test results tracking
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_SKIPPED=0

print_status() {
    local status=$1
    local message=$2
    case $status in
        "OK")
            echo -e "${GREEN}[✓]${NC} $message"
            ;;
        "FAIL")
            echo -e "${RED}[✗]${NC} $message"
            ;;
        "WARN")
            echo -e "${YELLOW}[!]${NC} $message"
            ;;
        "INFO")
            echo -e "${BLUE}[i]${NC} $message"
            ;;
    esac
}

# Cleanup function
cleanup() {
    echo ""
    echo "Cleaning up..."

    # Stop pmxcfs if running
    if pgrep pmxcfs > /dev/null 2>&1; then
        print_status "INFO" "Stopping pmxcfs..."
        pkill pmxcfs || true
        sleep 1
    fi

    # Unmount if still mounted
    if mountpoint -q /etc/pve 2>/dev/null; then
        print_status "INFO" "Unmounting /etc/pve..."
        umount -l /etc/pve 2>/dev/null || true
    fi

    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}                     Test Summary                          ${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}Passed:  ${TESTS_PASSED}${NC}"
    echo -e "${RED}Failed:  ${TESTS_FAILED}${NC}"
    echo -e "${YELLOW}Skipped: ${TESTS_SKIPPED}${NC}"
    echo ""

    # Exit with error if any tests failed
    if [ $TESTS_FAILED -gt 0 ]; then
        exit 1
    fi
}

trap cleanup EXIT INT TERM

echo "Environment Information:"
echo "  Hostname: $(hostname)"
echo "  Kernel: $(uname -r)"
echo "  Perl: $(perl -v | grep -oP '\(v\K[0-9.]+' | head -1)"
echo "  Container: Docker/Podman"
echo ""

# Check if pmxcfs binary exists
if [ ! -f /usr/local/bin/pmxcfs ]; then
    print_status "FAIL" "pmxcfs binary not found at /usr/local/bin/pmxcfs"
    exit 1
fi
print_status "OK" "pmxcfs binary found"

# Check PVE modules
print_status "INFO" "Checking PVE Perl modules..."
if perl -e 'use PVE::Cluster; use PVE::IPCC;' 2>/dev/null; then
    print_status "OK" "PVE Perl modules available"
    HAS_PVE_MODULES=true
else
    print_status "WARN" "PVE Perl modules not available - some tests will be skipped"
    HAS_PVE_MODULES=false
fi

echo ""
echo -e "${BLUE}═══════════════════════════════════════════════════════════${NC}"
echo -e "${BLUE}              Starting Rust pmxcfs                          ${NC}"
echo -e "${BLUE}═══════════════════════════════════════════════════════════${NC}"
echo ""

# Start pmxcfs in background
print_status "INFO" "Starting Rust pmxcfs..."
/usr/local/bin/pmxcfs --foreground --local &
PMXCFS_PID=$!

# Wait for startup
print_status "INFO" "Waiting for pmxcfs to start (PID: $PMXCFS_PID)..."
for i in {1..30}; do
    if mountpoint -q /etc/pve 2>/dev/null; then
        break
    fi
    sleep 0.5
    if ! ps -p $PMXCFS_PID > /dev/null 2>&1; then
        print_status "FAIL" "pmxcfs process died during startup"
        exit 1
    fi
done

if ! mountpoint -q /etc/pve 2>/dev/null; then
    print_status "FAIL" "Failed to mount filesystem after 15 seconds"
    exit 1
fi
print_status "OK" "Rust pmxcfs running (PID: $PMXCFS_PID)"
print_status "OK" "Filesystem mounted at /etc/pve"

# Check IPC socket
if [ -S /var/run/pve2 ]; then
    print_status "OK" "IPC socket available at /var/run/pve2"
else
    print_status "WARN" "IPC socket not found at /var/run/pve2"
fi

echo ""
echo -e "${BLUE}═══════════════════════════════════════════════════════════${NC}"
echo -e "${BLUE}                  Running Tests                             ${NC}"
echo -e "${BLUE}═══════════════════════════════════════════════════════════${NC}"
echo ""

cd /test/c-tests

# Test 1: Corosync parser test
echo -e "${YELLOW}Test 1: Corosync Configuration Parser${NC}"
if [ -f corosync_parser_test.pl ]; then
    if ./corosync_parser_test.pl > /tmp/corosync_test.log 2>&1; then
        print_status "OK" "Corosync parser test passed"
        ((TESTS_PASSED++))
    else
        print_status "FAIL" "Corosync parser test failed"
        cat /tmp/corosync_test.log | tail -20
        ((TESTS_FAILED++))
    fi
else
    print_status "SKIP" "corosync_parser_test.pl not found"
    ((TESTS_SKIPPED++))
fi
echo ""

# Wait a bit for daemon to be fully ready
sleep 2

# Test 2: VM config creation
echo -e "${YELLOW}Test 2: VM Config Creation${NC}"
print_status "INFO" "Creating test VM configuration..."
NODENAME=$(hostname)
if mkdir -p /etc/pve/nodes/$NODENAME/qemu-server 2>/dev/null; then
    if echo "name: test-vm" > /etc/pve/nodes/$NODENAME/qemu-server/100.conf 2>&1; then
        if [ -f /etc/pve/nodes/$NODENAME/qemu-server/100.conf ]; then
            print_status "OK" "VM config creation successful"
            ((TESTS_PASSED++))
        else
            print_status "FAIL" "VM config not readable"
            ((TESTS_FAILED++))
        fi
    else
        print_status "FAIL" "Failed to write VM config"
        ((TESTS_FAILED++))
    fi
else
    print_status "FAIL" "Failed to create directory"
    ((TESTS_FAILED++))
fi
echo ""

# Test 3: Config property access (requires PVE modules)
if [ "$HAS_PVE_MODULES" = true ] && [ -f scripts/test-config-get-property.pl ]; then
    echo -e "${YELLOW}Test 3: Config Property Access${NC}"
    if [ -f /etc/pve/nodes/$NODENAME/qemu-server/100.conf ]; then
        echo "lock: test-lock" >> /etc/pve/nodes/$NODENAME/qemu-server/100.conf

        if ./scripts/test-config-get-property.pl 100 lock > /tmp/config_prop_test.log 2>&1; then
            print_status "OK" "Config property access test passed"
            ((TESTS_PASSED++))
        else
            print_status "WARN" "Config property access test failed"
            print_status "INFO" "This may fail if PVE::Cluster APIs are not fully compatible"
            cat /tmp/config_prop_test.log | tail -10
            ((TESTS_FAILED++))
        fi
    else
        print_status "SKIP" "Config property test skipped (no test VM)"
        ((TESTS_SKIPPED++))
    fi
else
    print_status "SKIP" "Config property test skipped (no PVE modules or script)"
    ((TESTS_SKIPPED++))
fi
echo ""

# Test 4: File operations
echo -e "${YELLOW}Test 4: File Operations${NC}"
print_status "INFO" "Testing file creation and deletion..."
TEST_COUNT=0
FAIL_COUNT=0

for i in {1..10}; do
    if touch "/etc/pve/test_file_$i" 2>/dev/null; then
        ((TEST_COUNT++))
    else
        ((FAIL_COUNT++))
    fi
done

for i in {1..10}; do
    if rm -f "/etc/pve/test_file_$i" 2>/dev/null; then
        ((TEST_COUNT++))
    else
        ((FAIL_COUNT++))
    fi
done

if [ $FAIL_COUNT -eq 0 ]; then
    print_status "OK" "File operations test passed ($TEST_COUNT operations)"
    ((TESTS_PASSED++))
else
    print_status "FAIL" "File operations test failed ($FAIL_COUNT failures)"
    ((TESTS_FAILED++))
fi
echo ""

# Test 5: Directory operations
echo -e "${YELLOW}Test 5: Directory Operations${NC}"
print_status "INFO" "Testing directory creation and deletion..."
if mkdir -p /etc/pve/test_dir/subdir 2>/dev/null; then
    if [ -d /etc/pve/test_dir/subdir ]; then
        if rmdir /etc/pve/test_dir/subdir /etc/pve/test_dir 2>/dev/null; then
            print_status "OK" "Directory operations test passed"
            ((TESTS_PASSED++))
        else
            print_status "FAIL" "Directory deletion failed"
            ((TESTS_FAILED++))
        fi
    else
        print_status "FAIL" "Directory not readable"
        ((TESTS_FAILED++))
    fi
else
    print_status "FAIL" "Directory creation failed"
    ((TESTS_FAILED++))
fi
echo ""

# Test 6: Directory listing
echo -e "${YELLOW}Test 6: Directory Listing${NC}"
if ls -la /etc/pve/ > /tmp/pve_ls.log 2>&1; then
    print_status "OK" "Directory listing successful"
    print_status "INFO" "Contents:"
    cat /tmp/pve_ls.log | head -20
    ((TESTS_PASSED++))
else
    print_status "FAIL" "Directory listing failed"
    ((TESTS_FAILED++))
fi
echo ""

# Test 7: Large file operations (if test exists)
if [ -f scripts/create_large_files.pl ] && [ "$HAS_PVE_MODULES" = true ]; then
    echo -e "${YELLOW}Test 7: Large File Operations${NC}"
    print_status "INFO" "Creating large files..."
    if timeout 30 ./scripts/create_large_files.pl > /tmp/large_files.log 2>&1; then
        print_status "OK" "Large file operations test passed"
        ((TESTS_PASSED++))
    else
        print_status "WARN" "Large file operations test failed or timed out"
        ((TESTS_FAILED++))
    fi
    echo ""
fi

# Test 8: VM list test (if we have multiple VMs)
echo -e "${YELLOW}Test 8: VM List Test${NC}"
print_status "INFO" "Creating multiple VM configs..."
for vmid in 101 102 103; do
    echo "name: test-vm-$vmid" > /etc/pve/nodes/$NODENAME/qemu-server/$vmid.conf 2>/dev/null || true
done

# List all VMs
if ls -1 /etc/pve/nodes/$NODENAME/qemu-server/*.conf 2>/dev/null | wc -l | grep -q "[1-9]"; then
    VM_COUNT=$(ls -1 /etc/pve/nodes/$NODENAME/qemu-server/*.conf 2>/dev/null | wc -l)
    print_status "OK" "VM list test passed ($VM_COUNT VMs found)"
    ((TESTS_PASSED++))
else
    print_status "FAIL" "No VMs found"
    ((TESTS_FAILED++))
fi
echo ""

echo "Tests completed!"
echo ""

# Cleanup will be called by trap
