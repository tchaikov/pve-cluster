# PMXCFS FUSE Filesystem

## Overview

PMXCFS provides a FUSE-based cluster filesystem mounted at `/etc/pve`. This filesystem exposes cluster configuration, VM/container configurations, and dynamic status information.

## Filesystem Structure

```
/etc/pve/
├── local -> nodes/{nodename}/                    # Symlink plugin
├── qemu-server -> nodes/{nodename}/qemu-server/  # Symlink plugin
├── lxc -> nodes/{nodename}/lxc/                  # Symlink plugin
├── openvz -> nodes/{nodename}/openvz/            # Symlink plugin (legacy)
│
├── .version                                       # Plugin file
├── .members                                       # Plugin file
├── .vmlist                                        # Plugin file
├── .rrd                                           # Plugin file
├── .clusterlog                                    # Plugin file
├── .debug                                         # Plugin file
│
├── nodes/
│   ├── {node1}/
│   │   ├── qemu-server/          # VM configs
│   │   │   └── {vmid}.conf
│   │   ├── lxc/                  # CT configs
│   │   │   └── {ctid}.conf
│   │   ├── openvz/               # Legacy (OpenVZ)
│   │   └── priv/                 # Node-specific private data
│   └── {node2}/
│       └── ...
│
├── corosync.conf                  # Cluster configuration
├── corosync.conf.new              # Staging for new config
├── storage.cfg                    # Storage configuration
├── user.cfg                       # User database
├── domains.cfg                    # Authentication domains
├── datacenter.cfg                 # Datacenter settings
├── vzdump.cron                    # Backup schedule
├── vzdump.conf                    # Backup configuration
├── jobs.cfg                       # Job definitions
│
├── ha/                            # High Availability
│   ├── crm_commands
│   ├── manager_status
│   ├── resources.cfg
│   ├── groups.cfg
│   ├── rules.cfg
│   └── fence.cfg
│
├── sdn/                           # Software Defined Networking
│   ├── vnets.cfg
│   ├── zones.cfg
│   ├── controllers.cfg
│   ├── subnets.cfg
│   └── ipams.cfg
│
├── firewall/
│   └── cluster.fw                # Cluster firewall rules
│
├── replication.cfg                # Replication configuration
├── ceph.conf                      # Ceph configuration
│
├── notifications.cfg              # Notification settings
│
└── priv/                          # Cluster-wide private data
    ├── shadow.cfg                 # Password hashes
    ├── tfa.cfg                    # Two-factor auth
    ├── token.cfg                  # API tokens
    ├── notifications.cfg          # Private notification config
    └── acme/
        └── plugins.cfg            # ACME plugin configs
```

## File Categories

### Plugin Files (Dynamic Content)

Files beginning with `.` are plugin files that generate content dynamically:
- `.version` - Cluster version and status
- `.members` - Cluster membership
- `.vmlist` - VM/container list
- `.rrd` - RRD metrics dump
- `.clusterlog` - Cluster log entries
- `.debug` - Debug mode toggle

See `../plugins/README.md` for detailed format specifications.

### Symlink Plugins

Convenience symlinks to node-specific directories:
- `local/` - Points to current node's directory
- `qemu-server/` - Points to current node's VM configs
- `lxc/` - Points to current node's container configs
- `openvz/` - Points to current node's OpenVZ configs (legacy)

### Configuration Files (40 tracked files)

The following files are tracked for version changes and synchronized across the cluster:

**Core Configuration**:
- `corosync.conf` - Corosync cluster configuration
- `corosync.conf.new` - Staged configuration before activation
- `storage.cfg` - Storage pool definitions
- `user.cfg` - User accounts and permissions
- `domains.cfg` - Authentication realm configuration
- `datacenter.cfg` - Datacenter-wide settings

**Backup Configuration**:
- `vzdump.cron` - Backup schedule
- `vzdump.conf` - Backup job settings
- `jobs.cfg` - Recurring job definitions

**High Availability** (6 files):
- `ha/crm_commands` - HA command queue
- `ha/manager_status` - HA manager status
- `ha/resources.cfg` - HA resource definitions
- `ha/groups.cfg` - HA service groups
- `ha/rules.cfg` - HA placement rules
- `ha/fence.cfg` - Fencing configuration

**Software Defined Networking** (5 files):
- `sdn/vnets.cfg` - Virtual networks
- `sdn/zones.cfg` - Network zones
- `sdn/controllers.cfg` - SDN controllers
- `sdn/subnets.cfg` - Subnet definitions
- `sdn/ipams.cfg` - IP address management

**Notification** (2 files):
- `notifications.cfg` - Public notification settings
- `priv/notifications.cfg` - Private notification credentials

**Security** (5 files):
- `priv/shadow.cfg` - Password hashes
- `priv/tfa.cfg` - Two-factor authentication
- `priv/token.cfg` - API tokens
- `priv/acme/plugins.cfg` - ACME DNS plugins
- `firewall/cluster.fw` - Cluster-wide firewall rules

**Other**:
- `replication.cfg` - Storage replication jobs
- `ceph.conf` - Ceph cluster configuration

### Node-Specific Directories

Each node has a directory under `nodes/{nodename}/` containing:
- `qemu-server/*.conf` - QEMU/KVM VM configurations
- `lxc/*.conf` - LXC container configurations
- `openvz/*.conf` - OpenVZ container configurations (legacy)
- `priv/` - Node-specific private data (not replicated)

## FUSE Operations

### Supported Operations

All standard FUSE operations are supported:

**Metadata Operations**:
- `getattr` - Get file/directory attributes
- `readdir` - List directory contents
- `statfs` - Get filesystem statistics

**Read Operations**:
- `read` - Read file contents
- `readlink` - Read symlink target

**Write Operations**:
- `write` - Write file contents
- `create` - Create new file
- `unlink` - Delete file
- `mkdir` - Create directory
- `rmdir` - Delete directory
- `rename` - Rename/move file
- `truncate` - Truncate file to size
- `utimens` - Update timestamps

**Permission Operations**:
- `chmod` - Change file mode
- `chown` - Change file ownership

### Permission Handling

- **Regular paths**: Standard Unix permissions apply
- **Private paths** (`priv/` directories): Restricted to root only
- **Plugin files**: Read-only for most users, special handling for `.debug`

### File Size Limits

- Maximum file size: 1 MiB (1024 × 1024 bytes)
- Maximum filesystem size: 128 MiB
- Maximum inodes: 256,000

## Implementation

The FUSE filesystem is implemented in `filesystem.rs` and integrates with:
- **MemDB**: Backend storage (SQLite + in-memory tree)
- **Plugin System**: Dynamic file generation
- **Cluster Sync**: Changes are propagated via DFSM protocol
