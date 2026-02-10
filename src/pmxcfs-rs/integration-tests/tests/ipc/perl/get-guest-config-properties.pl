#!/usr/bin/perl
# Test: GET_GUEST_CONFIG_PROPERTIES (op 13)
# Gets multiple guest config properties
# Usage: get-guest-config-properties.pl <vmid> <property1> [property2] ...

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my $vmid = shift @ARGV // 0;
my @properties = @ARGV;

if (@properties == 0) {
    die "Usage: $0 <vmid> <property1> [property2] ...\n";
}

my $num_props = scalar @properties;
my $request = pack("LC", $vmid, $num_props);
for my $prop (@properties) {
    $request .= $prop . "\0";
}

my ($success, $result) = ipc_call(13, $request);

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
    "vms: " . scalar(keys %$data),
    "properties: " . join(", ", @properties)
);
