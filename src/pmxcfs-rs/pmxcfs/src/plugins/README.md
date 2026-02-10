# PMXCFS Plugin System

## Overview

The plugin system provides dynamic virtual files in the `/etc/pve` filesystem that generate content on-the-fly. These files provide cluster status, configuration, and monitoring data.

## Plugin Types

### Function Plugins

These plugins generate dynamic content when read:

- `.version` - Cluster version and status information
- `.members` - Cluster membership information
- `.vmlist` - List of VMs and containers
- `.rrd` - Round-robin database dump
- `.clusterlog` - Cluster log entries
- `.debug` - Debug mode toggle

### Symlink Plugins

These plugins create symlinks to node-specific directories:

- `local/` → `nodes/{nodename}/`
- `qemu-server/` → `nodes/{nodename}/qemu-server/`
- `lxc/` → `nodes/{nodename}/lxc/`
- `openvz/` → `nodes/{nodename}/openvz/` (legacy)

## Plugin File Formats

### .version Plugin

**Format**: JSON

**Fields**:
- `api` - API version (integer)
- `clinfo` - Cluster info version (integer)
- `cluster` - Cluster information object
  - `name` - Cluster name (string)
  - `nodes` - Number of nodes (integer)
  - `quorate` - Quorum status (1 or 0)
- `starttime` - Daemon start time (Unix timestamp)
- `version` - Software version (string)
- `vmlist` - VM list version (integer)

**Example**:
```json
{
  "api": 1,
  "clinfo": 2,
  "cluster": {
    "name": "pmxcfs",
    "nodes": 3,
    "quorate": 1
  },
  "starttime": 1699876543,
  "version": "9.0.6",
  "vmlist": 5
}
```

### .members Plugin

**Format**: JSON with sections

**Fields**:
- `cluster` - Cluster information object
  - `name` - Cluster name (string)
  - `version` - Cluster version (integer)
  - `nodes` - Number of nodes (integer)
  - `quorate` - Quorum status (1 or 0)
- `nodelist` - Array of node objects
  - `id` - Node ID (integer)
  - `name` - Node name (string)
  - `online` - Online status (1 or 0)
  - `ip` - Node IP address (string)

**Example**:
```json
{
  "cluster": {
    "name": "pmxcfs",
    "version": 2,
    "nodes": 3,
    "quorate": 1
  },
  "nodelist": [
    {
      "id": 1,
      "name": "node1",
      "online": 1,
      "ip": "192.168.1.10"
    },
    {
      "id": 2,
      "name": "node2",
      "online": 1,
      "ip": "192.168.1.11"
    },
    {
      "id": 3,
      "name": "node3",
      "online": 0,
      "ip": "192.168.1.12"
    }
  ]
}
```

### .vmlist Plugin

**Format**: INI-style with sections

**Sections**:
- `[qemu]` - QEMU/KVM virtual machines
- `[lxc]` - Linux containers

**Entry Format**: `VMID<TAB>NODE<TAB>VERSION`
- `VMID` - VM/container ID (integer)
- `NODE` - Node name where the VM is defined (string)
- `VERSION` - Configuration version (integer)

**Example**:
```
[qemu]
100	node1	2
101	node2	1

[lxc]
200	node1	1
201	node3	2
```

### .rrd Plugin

**Format**: Text format with schema-based key-value pairs (one per line)

**Line Format**: `{schema}/{id}:{timestamp}:{field1}:{field2}:...`
- `schema` - RRD schema name (e.g., `pve-node-9.0`, `pve-vm-9.0`, `pve-storage-9.0`)
- `id` - Resource identifier (node name, VMID, or storage name)
- `timestamp` - Unix timestamp
- `fields` - Colon-separated metric values

Schemas include node metrics, VM metrics, and storage metrics with appropriate fields for each type.

### .clusterlog Plugin

**Format**: JSON with data array

**Fields**:
- `data` - Array of log entry objects
  - `time` - Unix timestamp (integer)
  - `node` - Node name (string)
  - `priority` - Syslog priority (integer)
  - `ident` - Process identifier (string)
  - `tag` - Log tag (string)
  - `message` - Log message (string)

**Example**:
```json
{
  "data": [
    {
      "time": 1699876543,
      "node": "node1",
      "priority": 6,
      "ident": "pvedaemon",
      "tag": "task",
      "message": "Started VM 100"
    }
  ]
}
```

### .debug Plugin

**Format**: Plain text (single character)

**Values**:
- `0` - Debug mode disabled
- `1` - Debug mode enabled

**Behavior**:
- Reading returns current debug state
- Writing `1` enables debug logging
- Writing `0` disables debug logging

## Implementation Details

### Registry

The plugin registry (`registry.rs`) maintains all plugin definitions and handles lookups.

### Plugin Trait

All plugins implement a common trait that defines:
- `get_content()` - Generate plugin content
- `set_content()` - Handle writes (for `.debug` plugin)
- `get_attr()` - Return file attributes

### Integration with FUSE

Plugins are integrated into the FUSE filesystem layer and appear as regular files in `/etc/pve`.
