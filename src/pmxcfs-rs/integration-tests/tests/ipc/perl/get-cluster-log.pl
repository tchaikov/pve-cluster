#!/usr/bin/perl
# Test: GET_CLUSTER_LOG (op 8)
# Reads cluster log
# Usage: get-cluster-log.pl [max_entries] [user]

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my $max_entries = $ARGV[0] // 50;
my $user = $ARGV[1] // "";

# Build GET_CLUSTER_LOG request
# C struct format:
#   uint32_t max_entries;
#   uint32_t res1, res2, res3;  // reserved
#   char user[];  // null-terminated user string for filtering

my $request = pack("LLLL", $max_entries, 0, 0, 0);
$request .= $user . "\0";

my ($success, $result) = ipc_call(8, $request);

if (!$success) {
    test_failure("IPC call failed: errno=$result");
}

my ($ok, $data) = check_json_fields($result, 'data');
if (!$ok) {
    test_failure($data);
}

# Verify data is an array
if (ref($data->{data}) ne 'ARRAY') {
    test_failure("data is not an array");
}

# If user filter specified, verify all entries match
if ($user ne "") {
    for my $entry (@{$data->{data}}) {
        if ($entry->{user} ne $user) {
            test_failure("Found entry not from user '$user'");
        }
    }
}

test_success("",
    "entries: " . scalar(@{$data->{data}}),
    "user_filter: " . ($user eq "" ? "none" : $user)
);
