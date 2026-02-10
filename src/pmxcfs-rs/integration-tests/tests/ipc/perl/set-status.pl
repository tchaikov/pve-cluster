#!/usr/bin/perl
# Test: SET_STATUS (op 4)
# Updates node status
# Usage: set-status.pl <name> <data>

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my $name = $ARGV[0] or die "Usage: $0 <name> <data>\n";
my $data = $ARGV[1] // "";

# SET_STATUS: name (256 bytes) + data
my $name_padded = $name . "\0" . ("\0" x (255 - length($name)));
my $request = $name_padded . $data;

my ($success, $result) = ipc_call(4, $request);

if (!$success) {
    my $errno = $result;
    if ($errno == 1) {  # EPERM
        test_success("Permission denied (EPERM - requires root)");
    } else {
        test_failure("Unexpected errno: $errno");
    }
}

test_success("");
