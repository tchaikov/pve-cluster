#!/usr/bin/perl
# Test: GET_CLUSTER_INFO (op 2)
# Returns cluster member list

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my ($success, $result) = ipc_call(2, "");

if (!$success) {
    test_failure("IPC call failed: errno=$result");
}

my ($ok, $data) = check_json_fields($result, 'nodelist', 'quorate');
if (!$ok) {
    test_failure($data);
}

# Verify nodelist is an array
if (ref($data->{nodelist}) ne 'ARRAY') {
    test_failure("nodelist is not an array");
}

# Check each node has required fields
for my $node (@{$data->{nodelist}}) {
    if (!exists $node->{nodeid} || !exists $node->{name} ||
        !exists $node->{ip} || !exists $node->{online}) {
        test_failure("Node missing required fields");
    }
}

test_success("",
    "quorate: " . ($data->{quorate} ? "true" : "false"),
    "nodes: " . scalar(@{$data->{nodelist}})
);
