#!/bin/bash
# Common test library for IPC tests
# Provides reusable functions for testing IPC operations

# Check if Perl and PVE::IPCC are available
check_perl_requirements() {
    if ! command -v perl &> /dev/null; then
        echo "⚠ Warning: perl not available, skipping test"
        exit 0
    fi

    if ! perl -e 'use PVE::IPCC;' 2>/dev/null; then
        echo "⚠ Warning: PVE::IPCC module not available, skipping test"
        echo "  This test requires the Perl IPC client to be installed"
        exit 0
    fi
}

# Run a Perl script that uses PVE::IPCC
# Usage: run_perl_ipc <perl_script>
# The script should print "SUCCESS" on success, "FAILED" on failure
run_perl_ipc() {
    local perl_script="$1"

    # Add common Perl preamble
    local full_script="
use strict;
use warnings;
use PVE::IPCC;
use JSON;

$perl_script
"

    echo "$full_script" | perl 2>&1
}

# Test an IPC operation with Perl script
# Usage: test_ipc_perl <test_name> <perl_script>
test_ipc_perl() {
    local test_name="$1"
    local perl_script="$2"

    echo "Test: $test_name"
    echo "----------------------------------------"

    local result
    result=$(run_perl_ipc "$perl_script") || true

    if echo "$result" | grep -q "SUCCESS"; then
        echo "✓ $test_name passed"
        echo "$result" | grep -v "SUCCESS" | sed 's/^/  /'
        # Small delay to allow ring buffer cleanup to complete
        # This prevents rapid connection buildup during stress tests
        sleep 0.05
        return 0
    else
        echo "✗ $test_name failed"
        echo "$result" | sed 's/^/  /'
        return 1
    fi
}

# Simple IPC call wrapper
# Usage: ipc_call <op_code> <request_data>
# Returns: response data or empty on error (check $? for status)
ipc_call() {
    local op_code="$1"
    local request_data="$2"

    perl -e "
        use strict;
        use warnings;
        use PVE::IPCC;

        my \$result = PVE::IPCC::ipcc_send_rec($op_code, '$request_data');

        if (defined \$result) {
            print \$result;
            exit 0;
        } else {
            exit 1;
        }
    " 2>/dev/null
}

# Print test section header
print_section() {
    echo ""
    echo "========================================="
    echo "$1"
    echo "========================================="
    echo ""
}

# Print test subsection
print_subsection() {
    echo ""
    echo "$1"
    echo "----------------------------------------"
}
