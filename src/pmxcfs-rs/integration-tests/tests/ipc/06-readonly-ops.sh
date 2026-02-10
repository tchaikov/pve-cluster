#!/bin/bash
# Test: Read-only IPC Operations
# Tests GET_FS_VERSION, GET_CLUSTER_INFO, GET_GUEST_LIST, GET_CONFIG, GET_STATUS

set -e

# Source common test configuration and library
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"
source "$SCRIPT_DIR/test-lib.sh"

print_section "Read-only IPC Operations Test"

check_perl_requirements

MOUNT_PATH="$TEST_MOUNT_PATH"

# =============================================================================
# GET_FS_VERSION (op 1)
# =============================================================================

print_subsection "GET_FS_VERSION (op 1)"

test_ipc_perl "GET_FS_VERSION basic call" '
my $result = PVE::IPCC::ipcc_send_rec(1, "");

if (defined $result) {
    my $data = decode_json($result);
    if (exists $data->{version} && exists $data->{protocol} && exists $data->{cluster}) {
        print "SUCCESS\n";
        print "version: $data->{version}\n";
        print "protocol: $data->{protocol}\n";
        print "cluster: " . ($data->{cluster} ? "true" : "false") . "\n";
        exit 0;
    } else {
        print "FAILED: Missing required fields\n";
        exit 1;
    }
} else {
    print "FAILED: $!\n";
    exit 1;
}
' || exit 1

test_ipc_perl "GET_FS_VERSION version values" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;
use JSON;

my $result = PVE::IPCC::ipcc_send_rec(1, "");

if (defined $result) {
    my $data = decode_json($result);
    if ($data->{version} == 1 && $data->{protocol} == 1) {
        print "SUCCESS\n";
        exit 0;
    } else {
        print "FAILED: Expected version=1, protocol=1\n";
        exit 1;
    }
} else {
    print "FAILED: $!\n";
    exit 1;
}
EOF
)" || exit 1

# =============================================================================
# GET_CLUSTER_INFO (op 2)
# =============================================================================

print_subsection "GET_CLUSTER_INFO (op 2)"

test_ipc_perl "GET_CLUSTER_INFO basic call" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;
use JSON;

my $result = PVE::IPCC::ipcc_send_rec(2, "");

if (defined $result) {
    my $data = decode_json($result);
    if (exists $data->{nodelist} && exists $data->{quorate}) {
        print "SUCCESS\n";
        print "quorate: " . ($data->{quorate} ? "true" : "false") . "\n";
        print "nodes: " . scalar(@{$data->{nodelist}}) . "\n";
        exit 0;
    } else {
        print "FAILED: Missing required fields\n";
        exit 1;
    }
} else {
    print "FAILED: $!\n";
    exit 1;
}
EOF
)" || exit 1

test_ipc_perl "GET_CLUSTER_INFO nodelist structure" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;
use JSON;

my $result = PVE::IPCC::ipcc_send_rec(2, "");

if (defined $result) {
    my $data = decode_json($result);

    if (ref($data->{nodelist}) ne 'ARRAY') {
        print "FAILED: nodelist is not an array\n";
        exit 1;
    }

    for my $node (@{$data->{nodelist}}) {
        if (!exists $node->{nodeid} || !exists $node->{name} ||
            !exists $node->{ip} || !exists $node->{online}) {
            print "FAILED: Node missing required fields\n";
            exit 1;
        }
    }

    print "SUCCESS\n";
    exit 0;
} else {
    print "FAILED: $!\n";
    exit 1;
}
EOF
)" || exit 1

# =============================================================================
# GET_GUEST_LIST (op 3)
# =============================================================================

print_subsection "GET_GUEST_LIST (op 3)"

test_ipc_perl "GET_GUEST_LIST basic call" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;
use JSON;

my $result = PVE::IPCC::ipcc_send_rec(3, "");

if (defined $result) {
    my $data = decode_json($result);
    if (exists $data->{version} && exists $data->{ids}) {
        print "SUCCESS\n";
        print "version: $data->{version}\n";
        print "vms: " . scalar(keys %{$data->{ids}}) . "\n";
        exit 0;
    } else {
        print "FAILED: Missing required fields\n";
        exit 1;
    }
} else {
    print "FAILED: $!\n";
    exit 1;
}
EOF
)" || exit 1

test_ipc_perl "GET_GUEST_LIST structure validation" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;
use JSON;

my $result = PVE::IPCC::ipcc_send_rec(3, "");

if (defined $result) {
    my $data = decode_json($result);

    if (ref($data->{ids}) ne 'HASH') {
        print "FAILED: ids is not a hash\n";
        exit 1;
    }

    # Check each VM entry has required fields
    for my $vmid (keys %{$data->{ids}}) {
        my $vm = $data->{ids}{$vmid};
        if (!exists $vm->{node} || !exists $vm->{type} || !exists $vm->{version}) {
            print "FAILED: VM $vmid missing required fields\n";
            exit 1;
        }
    }

    print "SUCCESS\n";
    exit 0;
} else {
    print "FAILED: $!\n";
    exit 1;
}
EOF
)" || exit 1

# =============================================================================
# GET_CONFIG (op 6)
# =============================================================================

print_subsection "GET_CONFIG (op 6)"

# Skip GET_CONFIG tests for now - they require writable filesystem
echo "Skipping GET_CONFIG tests (require writable filesystem)"

# =============================================================================
# GET_STATUS (op 5)
# =============================================================================

print_subsection "GET_STATUS (op 5)"

# Skip GET_STATUS tests - they test error conditions
echo "Skipping GET_STATUS tests (covered by 09-all-ipc-ops.sh)"

# =============================================================================
# Summary
# =============================================================================

print_section "✓ All read-only IPC operation tests passed"

echo "Verified operations:"
echo "  - GET_FS_VERSION (op 1): Returns version info"
echo "  - GET_CLUSTER_INFO (op 2): Returns cluster member list"
echo "  - GET_GUEST_LIST (op 3): Returns VM/CT list"
echo ""
echo "Note: GET_CONFIG and GET_STATUS tests are covered by 09-all-ipc-ops.sh"

exit 0
