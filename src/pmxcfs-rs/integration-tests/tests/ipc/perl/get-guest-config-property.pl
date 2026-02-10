#!/usr/bin/perl
# Test: GET_GUEST_CONFIG_PROPERTY (op 11)
# Gets single guest config property
# Usage: get-guest-config-property.pl <vmid> <property>

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my $vmid = $ARGV[0] // 0;
my $property = $ARGV[1] or die "Usage: $0 <vmid> <property>\n";

my $request = pack("L", $vmid) . $property . "\0";

my ($success, $result) = ipc_call(11, $request);

if (!$success) {
    my $errno = $result;
    if ($errno == 22) {  # EINVAL
        test_success("Invalid parameter (EINVAL)");
    } elsif ($errno == 2) {  # ENOENT
        test_success("Not found (ENOENT)");
    } else {
        test_failure("Unexpected errno: $errno");
    }
}

# Parse JSON response
my $data = eval { JSON::decode_json($result) };
if ($@) {
    test_failure("Invalid JSON: $@");
}

if (ref($data) ne 'HASH') {
    test_failure("Response is not a hash");
}

test_success("",
    "vms: " . scalar(keys %$data)
);
