#!/bin/bash
# IPC Operations Test Driver
# Runs all IPC operation tests using Perl scripts

set -e

# Source common test configuration and library
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"
source "$SCRIPT_DIR/test-lib.sh"

PERL_DIR="$SCRIPT_DIR/perl"

print_section "IPC Operations Test Suite"

check_perl_requirements

# Helper function to run a Perl test script
run_perl_test() {
    local test_name="$1"
    local script="$2"
    shift 2
    local args=("$@")

    echo "Test: $test_name"
    echo "----------------------------------------"

    local result
    result=$("$PERL_DIR/$script" "${args[@]}" 2>&1) || true

    if echo "$result" | grep -q "SUCCESS"; then
        echo "✓ $test_name passed"
        echo "$result" | grep -v "SUCCESS" | sed 's/^/  /'
        return 0
    else
        echo "✗ $test_name failed"
        echo "$result" | sed 's/^/  /'
        return 1
    fi
}

# =============================================================================
# Read-only Operations
# =============================================================================

print_subsection "Read-only Operations"

run_perl_test "GET_FS_VERSION" "get-fs-version.pl" || exit 1
run_perl_test "GET_CLUSTER_INFO" "get-cluster-info.pl" || exit 1
run_perl_test "GET_GUEST_LIST" "get-guest-list.pl" || exit 1
run_perl_test "GET_RRD_DUMP" "get-rrd-dump.pl" || exit 1

# GET_CONFIG tests
echo ""
echo "GET_CONFIG tests:"
run_perl_test "GET_CONFIG - nonexistent file" "get-config.pl" "nonexistent-file-12345.txt" || exit 1
run_perl_test "GET_CONFIG - private path" "get-config.pl" "priv/token.cfg" || exit 1

# GET_STATUS tests
echo ""
echo "GET_STATUS tests:"
run_perl_test "GET_STATUS - empty name" "get-status.pl" "" || exit 1
run_perl_test "GET_STATUS - nonexistent key" "get-status.pl" "nonexistent-status-key-12345" || exit 1

# =============================================================================
# Cluster Log Operations
# =============================================================================

print_subsection "Cluster Log Operations"

# LOG_CLUSTER_MSG tests
run_perl_test "LOG_CLUSTER_MSG - basic" "log-cluster-msg.pl" 6 "testuser" "test-tag" "Test message" || exit 1
run_perl_test "LOG_CLUSTER_MSG - minimal strings" "log-cluster-msg.pl" 6 "a" "b" "c" || exit 1
run_perl_test "LOG_CLUSTER_MSG - long strings" "log-cluster-msg.pl" 6 "$(printf 'x%.0s' {1..50})" "$(printf 'y%.0s' {1..50})" "$(printf 'z%.0s' {1..100})" || exit 1

# Wait for log entries to be processed
sleep 0.5

# GET_CLUSTER_LOG tests
echo ""
echo "GET_CLUSTER_LOG tests:"
run_perl_test "GET_CLUSTER_LOG - no filter" "get-cluster-log.pl" 50 "" || exit 1
run_perl_test "GET_CLUSTER_LOG - with limit" "get-cluster-log.pl" 3 "" || exit 1
run_perl_test "GET_CLUSTER_LOG - default limit" "get-cluster-log.pl" 0 "" || exit 1

# =============================================================================
# Write Operations
# =============================================================================

print_subsection "Write Operations"

# SET_STATUS tests
run_perl_test "SET_STATUS - basic" "set-status.pl" "test-status-key" "test-value-123" || exit 1

# VERIFY_TOKEN tests
echo ""
echo "VERIFY_TOKEN tests:"
run_perl_test "VERIFY_TOKEN - empty token" "verify-token.pl" "" || exit 1
run_perl_test "VERIFY_TOKEN - token with newline" "verify-token.pl" "$(printf 'token\nwith\nnewline')" || exit 1
run_perl_test "VERIFY_TOKEN - nonexistent token" "verify-token.pl" "nonexistent-token-12345" || exit 1

# =============================================================================
# Guest Config Operations
# =============================================================================

print_subsection "Guest Config Operations"

# GET_GUEST_CONFIG_PROPERTY tests
run_perl_test "GET_GUEST_CONFIG_PROPERTY - invalid vmid" "get-guest-config-property.pl" 50 "name" || exit 1
run_perl_test "GET_GUEST_CONFIG_PROPERTY - nonexistent VM" "get-guest-config-property.pl" 99999 "name" || exit 1
run_perl_test "GET_GUEST_CONFIG_PROPERTY - all VMs" "get-guest-config-property.pl" 0 "name" || exit 1

# GET_GUEST_CONFIG_PROPERTIES tests
echo ""
echo "GET_GUEST_CONFIG_PROPERTIES tests:"
run_perl_test "GET_GUEST_CONFIG_PROPERTIES - invalid vmid" "get-guest-config-properties.pl" 50 "name" "memory" || exit 1
run_perl_test "GET_GUEST_CONFIG_PROPERTIES - invalid property name" "get-guest-config-properties.pl" 100 "Name" || exit 1
run_perl_test "GET_GUEST_CONFIG_PROPERTIES - multiple properties" "get-guest-config-properties.pl" 0 "name" "memory" "cores" || exit 1
run_perl_test "GET_GUEST_CONFIG_PROPERTIES - nonexistent VM" "get-guest-config-properties.pl" 99999 "name" || exit 1

# =============================================================================
# Summary
# =============================================================================

print_section "✓ All IPC operation tests passed"

echo "Test coverage:"
echo "  - Read-only operations: 5 operations"
echo "  - Cluster log operations: 2 operations"
echo "  - Write operations: 2 operations"
echo "  - Guest config operations: 2 operations"
echo "  - Authentication operations: 1 operation"
echo ""
echo "Total: 12/12 IPC operations (100% coverage)"

exit 0
