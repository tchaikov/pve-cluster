#!/usr/bin/perl
# Test: LOG_CLUSTER_MSG (op 7)
# Writes to cluster log
# Usage: log-cluster-msg.pl <priority> <ident> <tag> <message>

use strict;
use warnings;
use FindBin qw($RealBin);
use lib $RealBin;
use IPCTestLib;

my $priority = $ARGV[0] // 6;  # LOG_INFO
my $ident = $ARGV[1] or die "Usage: $0 <priority> <ident> <tag> <message>\n";
my $tag = $ARGV[2] or die "Usage: $0 <priority> <ident> <tag> <message>\n";
my $message = $ARGV[3] or die "Usage: $0 <priority> <ident> <tag> <message>\n";

# Build LOG_CLUSTER_MSG request
# C struct format:
#   uint8_t priority;
#   uint8_t ident_len;  // Length INCLUDING null terminator
#   uint8_t tag_len;    // Length INCLUDING null terminator
#   char data[];        // ident\0 + tag\0 + message\0

my $ident_len = length($ident) + 1;
my $tag_len = length($tag) + 1;

my $request = pack("CCC", $priority, $ident_len, $tag_len);
$request .= $ident . "\0";
$request .= $tag . "\0";
$request .= $message . "\0";

my ($success, $result) = ipc_call(7, $request);

if (!$success) {
    my $errno = $result;
    if ($errno == 1) {  # EPERM
        test_success("Permission denied (EPERM - requires root)");
    } else {
        test_failure("Unexpected errno: $errno");
    }
}

test_success("");
