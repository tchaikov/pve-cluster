#!/usr/bin/perl
# Test: GET_FS_VERSION (op 1)
# Returns filesystem version information

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my ($success, $result) = ipc_call(1, "");

if (!$success) {
    test_failure("IPC call failed: errno=$result");
}

my ($ok, $data) = check_json_fields($result, 'version', 'protocol', 'cluster');
if (!$ok) {
    test_failure($data);
}

# Verify expected values
if ($data->{version} != 1 || $data->{protocol} != 1) {
    test_failure("Unexpected version values (version=$data->{version}, protocol=$data->{protocol})");
}

test_success("",
    "version: $data->{version}",
    "protocol: $data->{protocol}",
    "cluster: " . ($data->{cluster} ? "true" : "false")
);
