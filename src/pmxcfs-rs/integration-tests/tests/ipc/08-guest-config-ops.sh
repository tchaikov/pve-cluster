#!/bin/bash
# Test: Guest Config IPC Operations
# Tests GET_GUEST_CONFIG_PROPERTY and GET_GUEST_CONFIG_PROPERTIES

set -e

# Source common test configuration and library
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"
source "$SCRIPT_DIR/test-lib.sh"

print_section "Guest Config IPC Operations Test"

check_perl_requirements

MOUNT_PATH="$TEST_MOUNT_PATH"

# =============================================================================
# GET_GUEST_CONFIG_PROPERTY (op 11)
# =============================================================================

print_subsection "GET_GUEST_CONFIG_PROPERTY (op 11)"

test_ipc_perl "GET_GUEST_CONFIG_PROPERTY invalid vmid range" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;

# vmid < 100 is invalid (except 0 which means all VMs)
my $vmid = 50;
my $property = "name\0";
my $request = pack("L", $vmid) . $property;

my $result = PVE::IPCC::ipcc_send_rec(11, $request);

if (defined $result) {
    print "FAILED: Should return EINVAL for vmid < 100\n";
    exit 1;
} else {
    my $errno = $! + 0;
    if ($errno == 22) {  # EINVAL
        print "SUCCESS\n";
        exit 0;
    } else {
        print "FAILED: Wrong errno (expected 22/EINVAL, got $errno)\n";
        exit 1;
    }
}
EOF
)" || exit 1

test_ipc_perl "GET_GUEST_CONFIG_PROPERTY nonexistent VM" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;

# vmid 99999 should not exist
my $vmid = 99999;
my $property = "name\0";
my $request = pack("L", $vmid) . $property;

my $result = PVE::IPCC::ipcc_send_rec(11, $request);

if (defined $result) {
    # Empty result is OK (VM doesn't exist)
    print "SUCCESS\n";
    exit 0;
} else {
    my $errno = $! + 0;
    if ($errno == 2) {  # ENOENT
        print "SUCCESS\n";
        exit 0;
    } else {
        print "FAILED: Unexpected errno $errno\n";
        exit 1;
    }
}
EOF
)" || exit 1

test_ipc_perl "GET_GUEST_CONFIG_PROPERTY vmid=0 (all VMs)" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;
use JSON;

# vmid=0 means get property from all VMs
my $vmid = 0;
my $property = "name\0";
my $request = pack("L", $vmid) . $property;

my $result = PVE::IPCC::ipcc_send_rec(11, $request);

if (defined $result) {
    # Should return JSON object (possibly empty if no VMs)
    my $data = decode_json($result);
    if (ref($data) eq 'HASH') {
        print "SUCCESS\n";
        print "vms: " . scalar(keys %$data) . "\n";
        exit 0;
    } else {
        print "FAILED: Response is not a hash\n";
        exit 1;
    }
} else {
    print "FAILED: $!\n";
    exit 1;
}
EOF
)" || exit 1

# =============================================================================
# GET_GUEST_CONFIG_PROPERTIES (op 13)
# =============================================================================

print_subsection "GET_GUEST_CONFIG_PROPERTIES (op 13)"

test_ipc_perl "GET_GUEST_CONFIG_PROPERTIES invalid vmid range" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;

# vmid < 100 is invalid (except 0)
my $vmid = 50;
my $num_props = 2;
my $request = pack("LC", $vmid, $num_props) . "name\0memory\0";

my $result = PVE::IPCC::ipcc_send_rec(13, $request);

if (defined $result) {
    print "FAILED: Should return EINVAL for vmid < 100\n";
    exit 1;
} else {
    my $errno = $! + 0;
    if ($errno == 22) {  # EINVAL
        print "SUCCESS\n";
        exit 0;
    } else {
        print "FAILED: Wrong errno (expected 22/EINVAL, got $errno)\n";
        exit 1;
    }
}
EOF
)" || exit 1

test_ipc_perl "GET_GUEST_CONFIG_PROPERTIES zero properties" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;

my $vmid = 100;
my $num_props = 0;
my $request = pack("LC", $vmid, $num_props);

my $result = PVE::IPCC::ipcc_send_rec(13, $request);

if (defined $result) {
    print "FAILED: Should return EINVAL for zero properties\n";
    exit 1;
} else {
    my $errno = $! + 0;
    if ($errno == 22) {  # EINVAL
        print "SUCCESS\n";
        exit 0;
    } else {
        print "FAILED: Wrong errno (expected 22/EINVAL, got $errno)\n";
        exit 1;
    }
}
EOF
)" || exit 1

test_ipc_perl "GET_GUEST_CONFIG_PROPERTIES invalid property name" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;

my $vmid = 100;
my $num_props = 1;
# Property must start with lowercase letter
my $request = pack("LC", $vmid, $num_props) . "Name\0";

my $result = PVE::IPCC::ipcc_send_rec(13, $request);

if (defined $result) {
    print "FAILED: Should return EINVAL for property not starting with [a-z]\n";
    exit 1;
} else {
    my $errno = $! + 0;
    if ($errno == 22) {  # EINVAL
        print "SUCCESS\n";
        exit 0;
    } else {
        print "FAILED: Wrong errno (expected 22/EINVAL, got $errno)\n";
        exit 1;
    }
}
EOF
)" || exit 1

test_ipc_perl "GET_GUEST_CONFIG_PROPERTIES multiple properties" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;
use JSON;

my $vmid = 0;  # All VMs
my $num_props = 3;
my $request = pack("LC", $vmid, $num_props) . "name\0memory\0cores\0";

my $result = PVE::IPCC::ipcc_send_rec(13, $request);

if (defined $result) {
    my $data = decode_json($result);
    if (ref($data) eq 'HASH') {
        print "SUCCESS\n";
        print "vms: " . scalar(keys %$data) . "\n";
        exit 0;
    } else {
        print "FAILED: Response is not a hash\n";
        exit 1;
    }
} else {
    print "FAILED: $!\n";
    exit 1;
}
EOF
)" || exit 1

test_ipc_perl "GET_GUEST_CONFIG_PROPERTIES nonexistent VM" "$(cat <<'EOF'
use strict;
use warnings;
use PVE::IPCC;

my $vmid = 99999;
my $num_props = 1;
my $request = pack("LC", $vmid, $num_props) . "name\0";

my $result = PVE::IPCC::ipcc_send_rec(13, $request);

if (defined $result) {
    print "FAILED: Should return ENOENT for nonexistent VM\n";
    exit 1;
} else {
    my $errno = $! + 0;
    if ($errno == 2) {  # ENOENT
        print "SUCCESS\n";
        exit 0;
    } else {
        print "FAILED: Wrong errno (expected 2/ENOENT, got $errno)\n";
        exit 1;
    }
}
EOF
)" || exit 1

# =============================================================================
# Summary
# =============================================================================

print_section "✓ All guest config IPC operation tests passed"

echo "Verified operations:"
echo "  - GET_GUEST_CONFIG_PROPERTY (op 11): Gets single guest config property"
echo "  - GET_GUEST_CONFIG_PROPERTIES (op 13): Gets multiple guest config properties"
echo ""
echo "Verified behaviors:"
echo "  - vmid validation (must be 0 or >= 100)"
echo "  - Property name validation (must start with [a-z])"
echo "  - Error handling for nonexistent VMs"
echo "  - Multiple property requests"

exit 0
