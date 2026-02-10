#!/usr/bin/perl
# Test: GET_RRD_DUMP (op 10)
# Gets RRD data dump

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my ($success, $result) = ipc_call(10, "");

if (!$success) {
    test_failure("IPC call failed: errno=$result");
}

# Verify last byte is NUL (M2 fix)
my $last_byte = substr($result, -1, 1);
my $last_byte_ord = ord($last_byte);

if ($last_byte_ord != 0) {
    test_failure("Last byte is not NUL (got $last_byte_ord)");
}

test_success("",
    "size: " . length($result) . " bytes",
    "nul_terminated: yes"
);
