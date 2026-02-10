# Perl IPC Test Scripts

Standalone Perl scripts for testing individual IPC operations.

## Structure

- **IPCTestLib.pm** - Common library (ipc_call, test_success, test_failure, check_json_fields)
- **12 operation scripts** - One per IPC operation

## Usage

```bash
# Run independently
./get-fs-version.pl
./get-cluster-log.pl 50 alice
./log-cluster-msg.pl 6 testuser test-tag "Test message"
./get-guest-config-properties.pl 0 name memory cores
```

## Scripts

| Script | Op | Arguments |
|--------|----|-----------|
| get-fs-version.pl | 1 | (none) |
| get-cluster-info.pl | 2 | (none) |
| get-guest-list.pl | 3 | (none) |
| set-status.pl | 4 | `<name> <data>` |
| get-status.pl | 5 | `<name> [nodename]` |
| get-config.pl | 6 | `<path> [expected_content]` |
| log-cluster-msg.pl | 7 | `<priority> <ident> <tag> <message>` |
| get-cluster-log.pl | 8 | `[max_entries] [user]` |
| get-rrd-dump.pl | 10 | (none) |
| get-guest-config-property.pl | 11 | `<vmid> <property>` |
| verify-token.pl | 12 | `<token>` |
| get-guest-config-properties.pl | 13 | `<vmid> <prop1> [prop2] ...` |

## Output

Success: `SUCCESS\n<details>` (exit 0)
Failure: `FAILED: <error>\n` (exit 1)

## Dependencies

- Perl 5, PVE::IPCC, JSON
- Running pmxcfs daemon
