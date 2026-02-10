#!/usr/bin/perl
# Test: GET_GUEST_LIST (op 3)
# Returns VM/CT list

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my ($success, $result) = ipc_call(3, "");

if (!$success) {
    test_failure("IPC call failed: errno=$result");
}

my ($ok, $data) = check_json_fields($result, 'version', 'ids');
if (!$ok) {
    test_failure($data);
}

# Verify ids is a hash
if (ref($data->{ids}) ne 'HASH') {
    test_failure("ids is not a hash");
}

# Check each VM entry has required fields
for my $vmid (keys %{$data->{ids}}) {
    my $vm = $data->{ids}{$vmid};
    if (!exists $vm->{node} || !exists $vm->{type} || !exists $vm->{version}) {
        test_failure("VM $vmid missing required fields");
    }
}

test_success("",
    "version: $data->{version}",
    "vms: " . scalar(keys %{$data->{ids}})
);
