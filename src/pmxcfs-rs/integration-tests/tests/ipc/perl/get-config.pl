#!/usr/bin/perl
# Test: GET_CONFIG (op 6)
# Reads configuration file
# Usage: get-config.pl <path> [expected_content]

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my $path = $ARGV[0] or die "Usage: $0 <path> [expected_content]\n";
my $expected_content = $ARGV[1];

my $request = $path . "\0";
my ($success, $result) = ipc_call(6, $request);

if (!$success) {
    my $errno = $result;
    # ENOENT (2) or EPERM (1) are acceptable for some tests
    if ($errno == 2) {
        test_success("File not found (ENOENT)");
    } elsif ($errno == 1) {
        test_success("Permission denied (EPERM)");
    } else {
        test_failure("Unexpected errno: $errno");
    }
}

# If expected content provided, verify it
if (defined $expected_content && $result !~ /\Q$expected_content\E/) {
    test_failure("Content mismatch");
}

test_success("",
    "content: " . length($result) . " bytes"
);
