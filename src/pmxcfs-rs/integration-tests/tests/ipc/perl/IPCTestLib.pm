#!/usr/bin/perl
# Common IPC test library
# Provides reusable functions for IPC tests

package IPCTestLib;

use strict;
use warnings;
use PVE::IPCC;
use JSON;

use Exporter 'import';
our @EXPORT = qw(
    ipc_call
    test_success
    test_failure
    check_json_fields
    check_errno
);

# Call an IPC operation
# Returns: (success, result_or_error)
sub ipc_call {
    my ($op_code, $request) = @_;

    my $result = PVE::IPCC::ipcc_send_rec($op_code, $request);
    my $errno = $! + 0;

    if (defined $result) {
        return (1, $result);
    } else {
        return (0, $errno);
    }
}

# Print success message and exit 0
sub test_success {
    my ($message, @details) = @_;
    print "SUCCESS\n";
    print "$message\n" if $message;
    print "$_\n" for @details;
    exit 0;
}

# Print failure message and exit 1
sub test_failure {
    my ($message) = @_;
    print "FAILED: $message\n";
    exit 1;
}

# Check if JSON has required fields
sub check_json_fields {
    my ($json_str, @fields) = @_;

    my $data = eval { decode_json($json_str) };
    if ($@) {
        return (0, "Invalid JSON: $@");
    }

    for my $field (@fields) {
        if (!exists $data->{$field}) {
            return (0, "Missing field: $field");
        }
    }

    return (1, $data);
}

# Check if errno matches expected value
sub check_errno {
    my ($errno, $expected, $name) = @_;

    if ($errno == $expected) {
        return 1;
    } else {
        return 0;
    }
}

1;
