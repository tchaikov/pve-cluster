#!/usr/bin/perl
# Test: GET_STATUS (op 5)
# Gets node status
# Usage: get-status.pl <name> [nodename]

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my $name = $ARGV[0] // "";
my $nodename = $ARGV[1] // "";

# GET_STATUS: name (256 bytes) + nodename (256 bytes)
my $name_padded = $name . "\0" . ("\0" x (255 - length($name)));
my $nodename_padded = $nodename . "\0" . ("\0" x (255 - length($nodename)));
my $request = $name_padded . $nodename_padded;

my ($success, $result) = ipc_call(5, $request);

if (!$success) {
    my $errno = $result;
    if ($errno == 2) {  # ENOENT
        test_success("Not found (ENOENT)");
    } else {
        test_failure("Unexpected errno: $errno");
    }
}

test_success("",
    "data: " . length($result) . " bytes"
);
