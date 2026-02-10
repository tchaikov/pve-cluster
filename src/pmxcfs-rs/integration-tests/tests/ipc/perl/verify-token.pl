#!/usr/bin/perl
# Test: VERIFY_TOKEN (op 12)
# Verifies authentication token
# Usage: verify-token.pl <token>

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my $token = $ARGV[0] // "";

my $request = $token . "\0";

my ($success, $result) = ipc_call(12, $request);

if (!$success) {
    my $errno = $result;
    if ($errno == 22) {  # EINVAL
        test_success("Invalid token (EINVAL)");
    } elsif ($errno == 2) {  # ENOENT
        test_success("Token not found (ENOENT)");
    } else {
        test_failure("Unexpected errno: $errno");
    }
}

test_success("Token valid");
