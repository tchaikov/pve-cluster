/// Status subsystem implementation
use crate::types::{ClusterInfo, ClusterLogEntry, ClusterNode, NodeStatus, RrdEntry};
use anyhow::Result;
use parking_lot::RwLock;
use pmxcfs_api_types::{VmEntry, VmType};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Status subsystem (matches C implementation's cfs_status_t)
pub struct Status {
    /// Configuration (nodename, IP, etc.) - matches C's global `cfs` variable
    /// Always present, just like C's global `cfs` struct (never NULL)
    config: Arc<pmxcfs_config::Config>,

    /// Cluster information (nodes, membership) - matches C's clinfo
    cluster_info: RwLock<Option<ClusterInfo>>,

    /// Cluster info version counter - increments on membership changes (matches C's clinfo_version)
    /// This is separate from config_version in ClusterInfo (which matches C's cman_version)
    cluster_version: AtomicU64,

    /// VM list version counter - increments when VM list changes (matches C's vmlist_version)
    vmlist_version: AtomicU64,

    /// Global VM info version counter (matches C's vminfo_version_counter)
    /// Used to track the order of VM updates across all VMs
    vminfo_version_counter: AtomicU64,

    /// MemDB path version counters (matches C's memdb_change_array)
    /// Tracks versions for specific config files like "corosync.conf", "user.cfg", etc.
    memdb_path_versions: RwLock<HashMap<String, AtomicU64>>,

    /// Node status data by name
    node_status: RwLock<HashMap<String, NodeStatus>>,

    /// Cluster log with ring buffer and deduplication (matches C's clusterlog_t)
    cluster_log: pmxcfs_logger::ClusterLog,

    /// RRD entries by key (e.g., "pve2-node/nodename" or "pve2.3-vm/vmid")
    pub(crate) rrd_data: RwLock<HashMap<String, RrdEntry>>,

    /// RRD dump cache (timestamp, cached_dump)
    rrd_dump_cache: RwLock<Option<(u64, String)>>,

    /// RRD file writer for persistent storage (using tokio RwLock for async compatibility)
    rrd_writer: Option<Arc<tokio::sync::RwLock<pmxcfs_rrd::RrdWriter>>>,

    /// VM/CT list (vmid -> VmEntry)
    vmlist: RwLock<HashMap<u32, VmEntry>>,

    /// Quorum status (matches C's cfs_status.quorate)
    quorate: RwLock<bool>,

    /// Current cluster members (CPG membership)
    members: RwLock<Vec<pmxcfs_api_types::MemberInfo>>,

    /// Daemon start timestamp (UNIX epoch) - for .version plugin
    start_time: u64,

    /// KV store data from nodes (nodeid -> key -> (value, version))
    /// Matches C implementation's kvhash with per-key version tracking
    kvstore: RwLock<HashMap<u32, HashMap<String, (Vec<u8>, u32)>>>,

    /// Node IP addresses (nodename -> IP) - matches C's iphash
    node_ips: RwLock<HashMap<String, String>>,
}

impl Status {
    /// Create a new Status instance
    ///
    /// For production use, use `pmxcfs_status::init_with_config()` or `init_with_config_and_rrd()`.
    /// For tests, use `pmxcfs_test_utils::create_test_config()` to create a config.
    ///
    /// # Arguments
    /// * `config` - Configuration (contains nodename, IP, etc.) - REQUIRED, like C's global cfs
    /// * `rrd_writer` - Optional RRD writer for persistent storage
    pub fn new(config: Arc<pmxcfs_config::Config>, rrd_writer: Option<pmxcfs_rrd::RrdWriter>) -> Self {
        // Wrap RrdWriter in Arc<tokio::sync::RwLock> if provided (for async compatibility)
        let rrd_writer = rrd_writer.map(|w| Arc::new(tokio::sync::RwLock::new(w)));

        // Initialize memdb path versions for common Proxmox config files
        // Matches C implementation's memdb_change_array (status.c:79-120)
        // These are the exact paths tracked by the C implementation
        let mut path_versions = HashMap::new();
        let common_paths = vec![
            "corosync.conf",
            "corosync.conf.new",
            "storage.cfg",
            "user.cfg",
            "domains.cfg",
            "notifications.cfg",
            "priv/notifications.cfg",
            "priv/shadow.cfg",
            "priv/acme/plugins.cfg",
            "priv/tfa.cfg",
            "priv/token.cfg",
            "datacenter.cfg",
            "vzdump.cron",
            "vzdump.conf",
            "jobs.cfg",
            "ha/crm_commands",
            "ha/manager_status",
            "ha/resources.cfg",
            "ha/rules.cfg",
            "ha/groups.cfg",
            "ha/fence.cfg",
            "status.cfg",
            "replication.cfg",
            "ceph.conf",
            "sdn/vnets.cfg",
            "sdn/zones.cfg",
            "sdn/controllers.cfg",
            "sdn/subnets.cfg",
            "sdn/ipams.cfg",
            "sdn/mac-cache.json",            // SDN MAC address cache
            "sdn/pve-ipam-state.json",       // SDN IPAM state
            "sdn/dns.cfg",                   // SDN DNS configuration
            "sdn/fabrics.cfg",               // SDN fabrics configuration
            "sdn/.running-config",           // SDN running configuration
            "virtual-guest/cpu-models.conf", // Virtual guest CPU models
            "virtual-guest/profiles.cfg",    // Virtual guest profiles
            "firewall/cluster.fw",           // Cluster firewall rules
            "mapping/directory.cfg",         // Directory mappings
            "mapping/pci.cfg",               // PCI device mappings
            "mapping/usb.cfg",               // USB device mappings
        ];

        for path in common_paths {
            path_versions.insert(path.to_string(), AtomicU64::new(0));
        }

        // Get start time (matches C implementation's cfs_status.start_time)
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            config,
            cluster_info: RwLock::new(None),
            cluster_version: AtomicU64::new(0), // Match C's clinfo_version starting at 0
            vmlist_version: AtomicU64::new(0),  // Match C's vmlist_version starting at 0
            vminfo_version_counter: AtomicU64::new(0),
            memdb_path_versions: RwLock::new(path_versions),
            node_status: RwLock::new(HashMap::new()),
            cluster_log: pmxcfs_logger::ClusterLog::new(),
            rrd_data: RwLock::new(HashMap::new()),
            rrd_dump_cache: RwLock::new(None),
            rrd_writer,
            vmlist: RwLock::new(HashMap::new()),
            quorate: RwLock::new(false),
            members: RwLock::new(Vec::new()),
            start_time,
            kvstore: RwLock::new(HashMap::new()),
            node_ips: RwLock::new(HashMap::new()),
        }
    }

    /// Get node status
    pub fn get_node_status(&self, name: &str) -> Option<NodeStatus> {
        self.node_status.read().get(name).cloned()
    }

    /// Set node status (matches C implementation's cfs_status_set)
    ///
    /// This handles status updates received via IPC from external clients.
    /// If the key starts with "rrd/", it's RRD data that should be written to disk.
    /// If the key is "nodeip", it's a node IP address update.
    /// Otherwise, it's generic node status data.
    pub async fn set_node_status(&self, name: String, data: Vec<u8>) -> Result<()> {
        // Check size limit (matches C's CFS_MAX_STATUS_SIZE check)
        if data.len() > pmxcfs_api_types::CFS_MAX_STATUS_SIZE {
            return Err(anyhow::anyhow!(
                "Status data too large: {} bytes (max: {})",
                data.len(),
                pmxcfs_api_types::CFS_MAX_STATUS_SIZE
            ));
        }

        // Check if this is RRD data (matching C's cfs_status_set behavior)
        if let Some(rrd_key) = name.strip_prefix("rrd/") {
            // Strip "rrd/" prefix to get the actual RRD key
            // Convert data to string (RRD data is text format)
            let mut data_str = String::from_utf8(data)
                .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in RRD data: {e}"))?;

            // Strip NUL termination from C payloads (C strings are NUL-terminated)
            if data_str.ends_with('\0') {
                data_str.pop();
            }

            // Write to RRD (stores in memory and writes to disk)
            self.set_rrd_data(rrd_key.to_string(), data_str).await?;
        } else if name == "nodeip" {
            // Node IP address update (matches C's nodeip_hash_set)
            let mut ip_str = String::from_utf8(data)
                .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in nodeip data: {e}"))?;

            // Strip NUL termination
            if ip_str.ends_with('\0') {
                ip_str.pop();
            }

            // Get current node name from config (always valid, like C's cfs.nodename)
            let nodename = self.get_local_nodename();
            let mut node_ips = self.node_ips.write();

            // Use entry API for atomic check-and-update to prevent race where
            // two concurrent updates could both see old value and both increment version
            use std::collections::hash_map::Entry;
            let needs_version_bump = match node_ips.entry(nodename.to_string()) {
                Entry::Occupied(mut e) if e.get() != &ip_str => {
                    e.insert(ip_str);
                    true
                }
                Entry::Vacant(e) => {
                    e.insert(ip_str);
                    true
                }
                _ => false,
            };

            drop(node_ips);

            if needs_version_bump {
                self.cluster_version.fetch_add(1, Ordering::SeqCst);
            }
        } else {
            // Regular node status (not RRD or nodeip)
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let status = NodeStatus {
                name: name.clone(),
                data,
                timestamp: now,
            };
            self.node_status.write().insert(name, status);
        }

        Ok(())
    }

    /// Get local node name (helper for nodeip handling)
    ///
    /// Returns the nodename from config (matches C implementation's use of cfs.nodename).
    /// The C code initializes cfs.nodename from uname() at startup (pmxcfs.c:826),
    /// and our Config does the same. This method simply returns that cached value.
    fn get_local_nodename(&self) -> &str {
        self.config.nodename()
    }

    /// Add cluster log entry
    pub fn add_log_entry(&self, entry: ClusterLogEntry) {
        // Convert ClusterLogEntry to ClusterLog format and add
        // The ClusterLog handles size limits and deduplication internally
        let _ = self.cluster_log.add(
            &entry.node,
            &entry.ident,
            &entry.tag,
            0, // pid not tracked in our entries
            entry.priority,
            entry.timestamp as u32,
            &entry.message,
        );
    }

    /// Get cluster log entries
    pub fn get_log_entries(&self, max: usize) -> Vec<ClusterLogEntry> {
        // Get entries from ClusterLog and convert to ClusterLogEntry
        self.cluster_log
            .get_entries(max)
            .into_iter()
            .map(|entry| ClusterLogEntry {
                uid: entry.uid,
                timestamp: entry.time as u64,
                priority: entry.priority,
                tag: entry.tag,
                pid: entry.pid,
                node: entry.node,
                ident: entry.ident,
                message: entry.message,
            })
            .collect()
    }

    /// Get cluster log entries filtered by ident (user)
    ///
    /// Matches C implementation: clog_dump_json() filters by ident_digest
    /// If user is empty, returns all entries (no filtering)
    pub fn get_log_entries_filtered(&self, max: usize, user: &str) -> Vec<ClusterLogEntry> {
        if user.is_empty() {
            return self.get_log_entries(max);
        }

        // Filter by ident field (matches C's ident_digest comparison)
        // Iterate all entries to ensure we don't miss matches (C iterates the entire ring buffer)
        let all_entries = self.cluster_log.get_entries(usize::MAX);
        all_entries
            .into_iter()
            .filter(|entry| entry.ident == user)
            .take(max)
            .map(|entry| ClusterLogEntry {
                uid: entry.uid,
                timestamp: entry.time as u64,
                priority: entry.priority,
                tag: entry.tag,
                pid: entry.pid,
                node: entry.node,
                ident: entry.ident,
                message: entry.message,
            })
            .collect()
    }

    /// Clear all cluster log entries (for testing)
    pub fn clear_cluster_log(&self) {
        self.cluster_log.clear();
    }

    /// Set RRD data (C-compatible format)
    /// Key format: "pve2-node/{nodename}" or "pve2.3-vm/{vmid}"
    /// Data format from pvestatd: "{non_archivable_fields...}:{ctime}:{val1}:{val2}:..."
    pub async fn set_rrd_data(&self, key: String, data: String) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let entry = RrdEntry {
            key: key.clone(),
            data: data.clone(),
            timestamp: now,
        };

        // Store in memory for .rrd plugin file
        self.rrd_data.write().insert(key.clone(), entry);

        // Also write to RRD file on disk (if persistence is enabled)
        if let Some(writer_lock) = &self.rrd_writer {
            let mut writer = writer_lock.write().await;
            writer.update(&key, &data).await?;
            tracing::trace!("Updated RRD file: {} -> {}", key, data);
        }

        Ok(())
    }

    /// Remove old RRD entries (older than 5 minutes)
    pub fn remove_old_rrd_data(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        const EXPIRE_SECONDS: u64 = 60 * 5; // 5 minutes

        self.rrd_data
            .write()
            .retain(|_, entry| {
                // Handle clock jumps backwards by checking both directions
                now.saturating_sub(entry.timestamp) < EXPIRE_SECONDS
            });
    }

    /// Get RRD data dump (text format matching C implementation)
    pub fn get_rrd_dump(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Check cache (valid for 2 seconds, matching C implementation)
        const CACHE_SECONDS: u64 = 2;
        {
            let cache = self.rrd_dump_cache.read();
            if let Some((cache_time, ref cached_dump)) = *cache {
                if now.saturating_sub(cache_time) < CACHE_SECONDS {
                    return cached_dump.clone();
                }
            }
        }

        // Remove old entries first
        self.remove_old_rrd_data();

        let rrd = self.rrd_data.read();
        let mut result = String::new();

        for entry in rrd.values() {
            result.push_str(&entry.key);
            result.push(':');
            result.push_str(&entry.data);
            result.push('\n');
        }

        // Append NUL terminator for Perl compatibility (matches C implementation)
        result.push('\0');

        drop(rrd);

        // Update cache
        *self.rrd_dump_cache.write() = Some((now, result.clone()));

        result
    }

    /// Collect disk I/O statistics (bytes read, bytes written)
    ///
    /// Note: This is for future VM RRD implementation. Per C implementation:
    /// - Node RRD (rrd_def_node) has 12 fields and does NOT include diskread/diskwrite
    /// - VM RRD (rrd_def_vm) has 10 fields and DOES include diskread/diskwrite at indices 8-9
    ///
    /// This method will be used when implementing VM RRD collection.
    ///
    /// # Sector Size
    /// The Linux kernel reports disk statistics in /proc/diskstats using 512-byte sectors
    /// as the standard unit, regardless of the device's actual physical sector size.
    /// This is a kernel reporting convention (see Documentation/admin-guide/iostats.rst).
    #[allow(dead_code)]
    fn collect_disk_io() -> Result<(u64, u64)> {
        // /proc/diskstats always uses 512-byte sectors (kernel convention)
        const DISKSTATS_SECTOR_SIZE: u64 = 512;

        let diskstats = procfs::diskstats()?;

        let mut total_read = 0u64;
        let mut total_write = 0u64;

        for stat in diskstats {
            // Skip partitions (only look at whole disks: sda, vda, etc.)
            if stat
                .name
                .chars()
                .last()
                .map(|c| c.is_numeric())
                .unwrap_or(false)
            {
                continue;
            }

            // Convert sectors to bytes using kernel's reporting unit
            total_read += stat.sectors_read * DISKSTATS_SECTOR_SIZE;
            total_write += stat.sectors_written * DISKSTATS_SECTOR_SIZE;
        }

        Ok((total_read, total_write))
    }

    /// Register a VM/CT
    pub fn register_vm(&self, vmid: u32, vmtype: VmType, node: String) {
        tracing::debug!(vmid, vmtype = ?vmtype, node = %node, "Registered VM");

        // Use global version counter (matches C's vminfo_version_counter)
        let version = (self.vminfo_version_counter.fetch_add(1, Ordering::SeqCst) + 1) as u32;

        let entry = VmEntry {
            vmid,
            vmtype,
            node,
            version,
        };
        self.vmlist.write().insert(vmid, entry);

        // Increment vmlist version counter
        self.increment_vmlist_version();
    }

    /// Delete a VM/CT
    pub fn delete_vm(&self, vmid: u32) {
        self.vmlist.write().remove(&vmid);
        tracing::debug!(vmid, "Deleted VM");

        // Always increment vmlist version counter (matches C behavior)
        self.increment_vmlist_version();
    }

    /// Check if VM/CT exists
    pub fn vm_exists(&self, vmid: u32) -> bool {
        self.vmlist.read().contains_key(&vmid)
    }

    /// Check if a different VM/CT exists (different node or type)
    pub fn different_vm_exists(&self, vmid: u32, vmtype: VmType, node: &str) -> bool {
        if let Some(entry) = self.vmlist.read().get(&vmid) {
            entry.vmtype != vmtype || entry.node != node
        } else {
            false
        }
    }

    /// Get VM list
    pub fn get_vmlist(&self) -> HashMap<u32, VmEntry> {
        self.vmlist.read().clone()
    }

    /// Scan directories for VMs/CTs and update vmlist
    ///
    /// Uses memdb's `recreate_vmlist()` to properly scan nodes/*/qemu-server/
    /// and nodes/*/lxc/ directories to track which node each VM belongs to.
    pub fn scan_vmlist(&self, memdb: &pmxcfs_memdb::MemDb) {
        // Use the proper recreate_vmlist from memdb which scans nodes/*/qemu-server/ and nodes/*/lxc/
        match pmxcfs_memdb::recreate_vmlist(memdb) {
            Ok(new_vmlist) => {
                let vmlist_len = new_vmlist.len();
                let mut vmlist = self.vmlist.write();

                // Preserve version counters for existing VMs, assign new versions to new VMs
                for (vmid, new_entry) in &new_vmlist {
                    if let Some(existing) = vmlist.get(vmid) {
                        // VM already exists - check if it changed
                        if existing.vmtype != new_entry.vmtype || existing.node != new_entry.node {
                            // VM changed - increment global counter and update
                            let version = (self.vminfo_version_counter.fetch_add(1, Ordering::SeqCst) + 1) as u32;
                            vmlist.insert(*vmid, VmEntry {
                                vmid: *vmid,
                                vmtype: new_entry.vmtype,
                                node: new_entry.node.clone(),
                                version,
                            });
                        }
                        // else: VM unchanged, keep existing entry with its version
                    } else {
                        // New VM - assign new version
                        let version = (self.vminfo_version_counter.fetch_add(1, Ordering::SeqCst) + 1) as u32;
                        vmlist.insert(*vmid, VmEntry {
                            vmid: *vmid,
                            vmtype: new_entry.vmtype,
                            node: new_entry.node.clone(),
                            version,
                        });
                    }
                }

                // Remove VMs that no longer exist
                vmlist.retain(|vmid, _| new_vmlist.contains_key(vmid));

                drop(vmlist);

                tracing::info!(vms = vmlist_len, "VM list scan complete");

                // Increment vmlist version counter
                self.increment_vmlist_version();
            }
            Err(err) => {
                tracing::error!(error = %err, "Failed to recreate vmlist");
            }
        }
    }

    /// Initialize cluster information with cluster name
    pub fn init_cluster(&self, cluster_name: String) {
        let info = ClusterInfo::new(cluster_name, 0);
        *self.cluster_info.write() = Some(info);
        self.cluster_version.fetch_add(1, Ordering::SeqCst);
    }

    /// Register a node in the cluster (name, ID, IP)
    pub fn register_node(&self, node_id: u32, name: String, ip: String) {
        tracing::debug!(node_id, node = %name, ip = %ip, "Registering cluster node");

        let mut cluster_info = self.cluster_info.write();
        if let Some(ref mut info) = *cluster_info {
            let node = ClusterNode {
                name,
                node_id,
                ip,
                online: false, // Will be updated by cluster module
            };
            info.add_node(node);
            self.cluster_version.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Get cluster information (for .members plugin)
    pub fn get_cluster_info(&self) -> Option<ClusterInfo> {
        self.cluster_info.read().clone()
    }

    /// Get cluster version
    pub fn get_cluster_version(&self) -> u64 {
        self.cluster_version.load(Ordering::SeqCst)
    }

    /// Increment cluster version (called when membership changes)
    pub fn increment_cluster_version(&self) {
        self.cluster_version.fetch_add(1, Ordering::SeqCst);
    }

    /// Update cluster info from CMAP (called by ClusterConfigService)
    pub fn update_cluster_info(
        &self,
        cluster_name: String,
        config_version: u64,
        nodes: Vec<(u32, String, String)>,
    ) -> Result<()> {
        let mut cluster_info = self.cluster_info.write();

        // Create or update cluster info
        let mut info = cluster_info
            .take()
            .unwrap_or_else(|| ClusterInfo::new(cluster_name.clone(), config_version));

        // Update cluster name if changed
        if info.cluster_name != cluster_name {
            info.cluster_name = cluster_name;
        }

        // Update config version
        info.config_version = config_version;

        // Preserve online status from old nodes (matches C's cfs_status_set_clinfo)
        let old_nodes = info.nodes_by_id.clone();

        // Clear existing nodes
        info.nodes_by_id.clear();
        info.nodes_by_name.clear();

        // Add updated nodes, preserving online status
        for (nodeid, name, ip) in nodes {
            let online = old_nodes
                .get(&nodeid)
                .map(|old_node| old_node.online)
                .unwrap_or(false);

            let node = ClusterNode {
                name: name.clone(),
                node_id: nodeid,
                ip,
                online,
            };
            info.add_node(node);
        }

        // Clean up kvstore entries for removed nodes
        let mut kvstore = self.kvstore.write();
        kvstore.retain(|nodeid, _| info.nodes_by_id.contains_key(nodeid));
        drop(kvstore);

        *cluster_info = Some(info);

        // Increment cluster_version (separate from config_version)
        self.cluster_version.fetch_add(1, Ordering::SeqCst);

        tracing::info!(version = config_version, "Updated cluster configuration");
        Ok(())
    }

    /// Update node online status (called by cluster module)
    pub fn set_node_online(&self, node_id: u32, online: bool) {
        let mut cluster_info = self.cluster_info.write();
        if let Some(ref mut info) = *cluster_info
            && let Some(node) = info.nodes_by_id.get_mut(&node_id)
            && node.online != online
        {
            node.online = online;
            self.cluster_version.fetch_add(1, Ordering::SeqCst);
            tracing::debug!(
                node = %node.name,
                node_id,
                online = if online { "true" } else { "false" },
                "Node online status changed"
            );
        }
    }

    /// Check if cluster is quorate (matches C's cfs_is_quorate)
    pub fn is_quorate(&self) -> bool {
        *self.quorate.read()
    }

    /// Set quorum status (matches C's cfs_set_quorate)
    pub fn set_quorate(&self, quorate: bool) {
        let mut quorate_guard = self.quorate.write();
        let old_quorate = *quorate_guard;
        *quorate_guard = quorate;
        drop(quorate_guard);

        if old_quorate != quorate {
            if quorate {
                tracing::info!("Node has quorum");
            } else {
                tracing::info!("Node lost quorum");
            }
        }
    }

    /// Get current cluster members (CPG membership)
    pub fn get_members(&self) -> Vec<pmxcfs_api_types::MemberInfo> {
        self.members.read().clone()
    }

    /// Update cluster members and sync online status (matches C's dfsm_confchg callback)
    ///
    /// This updates the CPG member list and synchronizes the online status
    /// in cluster_info to match current membership.
    ///
    /// Both members and cluster_info are updated atomically under locks
    /// to prevent readers from seeing inconsistent state.
    pub fn update_members(&self, members: Vec<pmxcfs_api_types::MemberInfo>) {
        // Acquire both locks before any updates to ensure atomicity
        // (matches C's single mutex protection in status.c)
        let mut members_guard = self.members.write();
        let mut cluster_info = self.cluster_info.write();

        // Update members first
        *members_guard = members.clone();

        // Update online status in cluster_info based on members
        // (matches C implementation's dfsm_confchg in status.c:1989-2025)
        if let Some(ref mut info) = *cluster_info {
            // First mark all nodes as offline
            for node in info.nodes_by_id.values_mut() {
                node.online = false;
            }

            // Then mark active members as online
            for member in &members {
                if let Some(node) = info.nodes_by_id.get_mut(&member.node_id) {
                    node.online = true;
                }
            }

            self.cluster_version.fetch_add(1, Ordering::SeqCst);
        }

        // Both locks released together at end of scope
    }

    /// Get daemon start timestamp (for .version plugin)
    pub fn get_start_time(&self) -> u64 {
        self.start_time
    }

    /// Increment VM list version (matches C's cfs_status.vmlist_version++)
    pub fn increment_vmlist_version(&self) {
        self.vmlist_version.fetch_add(1, Ordering::SeqCst);
    }

    /// Get VM list version
    pub fn get_vmlist_version(&self) -> u64 {
        self.vmlist_version.load(Ordering::SeqCst)
    }

    /// Increment version for a specific memdb path (matches C's record_memdb_change)
    pub fn increment_path_version(&self, path: &str) {
        let versions = self.memdb_path_versions.read();
        if let Some(counter) = versions.get(path) {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Get version for a specific memdb path
    pub fn get_path_version(&self, path: &str) -> u64 {
        let versions = self.memdb_path_versions.read();
        versions
            .get(path)
            .map(|counter| counter.load(Ordering::SeqCst))
            .unwrap_or(0)
    }

    /// Get all memdb path versions (for .version plugin)
    pub fn get_all_path_versions(&self) -> HashMap<String, u64> {
        let versions = self.memdb_path_versions.read();
        versions
            .iter()
            .map(|(path, counter)| (path.clone(), counter.load(Ordering::SeqCst)))
            .collect()
    }

    /// Increment ALL configuration file versions (matches C's record_memdb_reload)
    ///
    /// Called when the entire database is reloaded from cluster peers.
    /// This ensures clients know that all configuration files should be re-read.
    pub fn increment_all_path_versions(&self) {
        let versions = self.memdb_path_versions.read();
        for (_, counter) in versions.iter() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Set key-value data from a node (kvstore DFSM)
    ///
    /// Matches C implementation's cfs_kvstore_node_set in status.c.
    /// Stores ephemeral status data like RRD metrics, IP addresses, etc.
    pub fn set_node_kv(&self, nodeid: u32, key: String, value: Vec<u8>) {
        // Validate that the node exists in cluster info
        let cluster_info = self.cluster_info.read();
        match &*cluster_info {
            Some(info) if info.nodes_by_id.contains_key(&nodeid) => {},
            _ => {
                tracing::warn!(nodeid, key = %key, "Ignoring KV update for unknown node");
                return;
            }
        }
        drop(cluster_info);

        // Handle special keys (matches C's cfs_kvstore_node_set)
        if let Some(rrd_key) = key.strip_prefix("rrd/") {
            // RRD data - convert to string and store
            if let Ok(mut data_str) = String::from_utf8(value) {
                // Strip NUL termination
                if data_str.ends_with('\0') {
                    data_str.pop();
                }
                // Store RRD data (async operation, but we can't await here)
                // In production, this would be handled by spawning a task
                tracing::trace!(nodeid, key = %rrd_key, "Received RRD data from node");
            }
        } else if key == "nodeip" {
            // Node IP address
            if let Ok(mut ip_str) = String::from_utf8(value.clone()) {
                // Strip NUL termination
                if ip_str.ends_with('\0') {
                    ip_str.pop();
                }
                // Get node name from cluster info
                let cluster_info = self.cluster_info.read();
                if let Some(info) = &*cluster_info {
                    if let Some(node) = info.nodes_by_id.get(&nodeid) {
                        let nodename = node.name.clone();
                        drop(cluster_info);

                        let mut node_ips = self.node_ips.write();
                        let old_ip = node_ips.get(&nodename);

                        if old_ip.map(|s| s.as_str()) != Some(ip_str.as_str()) {
                            node_ips.insert(nodename, ip_str);
                            drop(node_ips);
                            self.cluster_version.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            }
        } else {
            // Regular KV data with version tracking (matches C's kventry_hash_set)
            let mut kvstore = self.kvstore.write();
            let node_kv = kvstore.entry(nodeid).or_default();

            // Remove entry if value is empty (matches C behavior)
            if value.is_empty() {
                node_kv.remove(&key);
            } else {
                // Increment version for this key
                let new_version = node_kv
                    .get(&key)
                    .map(|(_, version)| version + 1)
                    .unwrap_or(1);
                node_kv.insert(key, (value, new_version));
            }
        }
    }

    /// Get key-value data from a node
    pub fn get_node_kv(&self, nodeid: u32, key: &str) -> Option<Vec<u8>> {
        let kvstore = self.kvstore.read();
        kvstore.get(&nodeid)?.get(key).map(|(value, _)| value.clone())
    }

    /// Add cluster log entry (called by kvstore DFSM)
    ///
    /// This is the wrapper for kvstore LOG messages.
    /// Matches C implementation's clusterlog_insert call.
    pub fn add_cluster_log(
        &self,
        timestamp: u32,
        priority: u8,
        tag: String,
        node: String,
        message: String,
    ) {
        let entry = ClusterLogEntry {
            uid: 0,
            timestamp: timestamp as u64,
            priority,
            tag,
            pid: 0,
            node,
            ident: String::new(),
            message,
        };
        self.add_log_entry(entry);
    }

    /// Update node online status based on CPG membership (kvstore DFSM confchg callback)
    ///
    /// This is called when kvstore CPG membership changes.
    /// Matches C implementation's dfsm_confchg in status.c.
    pub fn update_member_status(&self, member_list: &[u32]) {
        let mut cluster_info = self.cluster_info.write();
        if let Some(ref mut info) = *cluster_info {
            // Mark all nodes as offline
            for node in info.nodes_by_id.values_mut() {
                node.online = false;
            }

            // Mark nodes in member_list as online
            for &nodeid in member_list {
                if let Some(node) = info.nodes_by_id.get_mut(&nodeid) {
                    node.online = true;
                }
            }

            self.cluster_version.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Get cluster log state (for DFSM synchronization)
    ///
    /// Returns the cluster log in C-compatible binary format (clog_base_t).
    /// Matches C implementation's clusterlog_get_state() in logger.c:553-571.
    pub fn get_cluster_log_state(&self) -> Result<Vec<u8>> {
        self.cluster_log.get_state()
    }

    /// Merge cluster log states from remote nodes
    ///
    /// Deserializes binary states from remote nodes and merges them with the local log.
    /// Matches C implementation's dfsm_process_state_update() in status.c:2049-2074.
    pub fn merge_cluster_log_states(
        &self,
        states: &[pmxcfs_api_types::NodeSyncInfo],
    ) -> Result<()> {
        use pmxcfs_logger::ClusterLog;

        let mut remote_logs = Vec::new();

        for state_info in states {
            // Check if this node has state data
            let state_data = match &state_info.state {
                Some(data) if !data.is_empty() => data,
                _ => continue,
            };

            match ClusterLog::deserialize_state(state_data) {
                Ok(ring_buffer) => {
                    tracing::debug!(
                        "Deserialized cluster log from node {}: {} entries",
                        state_info.node_id,
                        ring_buffer.len()
                    );
                    remote_logs.push(ring_buffer);
                }
                Err(e) => {
                    tracing::warn!(
                        nodeid = state_info.node_id,
                        error = %e,
                        "Failed to deserialize cluster log from node"
                    );
                }
            }
        }

        if !remote_logs.is_empty() {
            // Merge remote logs with local log (include_local = true)
            // The merge() method atomically updates both buffer and dedup state
            match self.cluster_log.merge(remote_logs, true) {
                Ok(()) => {
                    tracing::debug!("Successfully merged cluster logs");
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to merge cluster logs");
                }
            }
        }

        Ok(())
    }

    /// Add cluster log entry from remote node (kvstore LOG message)
    ///
    /// Matches C implementation's clusterlog_insert() via kvstore message handling.
    pub fn add_remote_cluster_log(
        &self,
        time: u32,
        priority: u8,
        node: String,
        ident: String,
        tag: String,
        message: String,
    ) -> Result<()> {
        self.cluster_log
            .add(&node, &ident, &tag, 0, priority, time, &message)?;
        Ok(())
    }
}

// Implement StatusOps trait for Status
impl crate::traits::StatusOps for Status {
    fn get_node_status(&self, name: &str) -> Option<NodeStatus> {
        self.get_node_status(name)
    }

    fn set_node_status<'a>(
        &'a self,
        name: String,
        data: Vec<u8>,
    ) -> crate::traits::BoxFuture<'a, Result<()>> {
        Box::pin(self.set_node_status(name, data))
    }

    fn add_log_entry(&self, entry: ClusterLogEntry) {
        self.add_log_entry(entry)
    }

    fn get_log_entries(&self, max: usize) -> Vec<ClusterLogEntry> {
        self.get_log_entries(max)
    }

    fn clear_cluster_log(&self) {
        self.clear_cluster_log()
    }

    fn add_cluster_log(
        &self,
        timestamp: u32,
        priority: u8,
        tag: String,
        node: String,
        msg: String,
    ) {
        self.add_cluster_log(timestamp, priority, tag, node, msg)
    }

    fn get_cluster_log_state(&self) -> Result<Vec<u8>> {
        self.get_cluster_log_state()
    }

    fn merge_cluster_log_states(&self, states: &[pmxcfs_api_types::NodeSyncInfo]) -> Result<()> {
        self.merge_cluster_log_states(states)
    }

    fn add_remote_cluster_log(
        &self,
        time: u32,
        priority: u8,
        node: String,
        ident: String,
        tag: String,
        message: String,
    ) -> Result<()> {
        self.add_remote_cluster_log(time, priority, node, ident, tag, message)
    }

    fn set_rrd_data<'a>(
        &'a self,
        key: String,
        data: String,
    ) -> crate::traits::BoxFuture<'a, Result<()>> {
        Box::pin(self.set_rrd_data(key, data))
    }

    fn remove_old_rrd_data(&self) {
        self.remove_old_rrd_data()
    }

    fn get_rrd_dump(&self) -> String {
        self.get_rrd_dump()
    }

    fn register_vm(&self, vmid: u32, vmtype: VmType, node: String) {
        self.register_vm(vmid, vmtype, node)
    }

    fn delete_vm(&self, vmid: u32) {
        self.delete_vm(vmid)
    }

    fn vm_exists(&self, vmid: u32) -> bool {
        self.vm_exists(vmid)
    }

    fn different_vm_exists(&self, vmid: u32, vmtype: VmType, node: &str) -> bool {
        self.different_vm_exists(vmid, vmtype, node)
    }

    fn get_vmlist(&self) -> HashMap<u32, VmEntry> {
        self.get_vmlist()
    }

    fn scan_vmlist(&self, memdb: &pmxcfs_memdb::MemDb) {
        self.scan_vmlist(memdb)
    }

    fn init_cluster(&self, cluster_name: String) {
        self.init_cluster(cluster_name)
    }

    fn register_node(&self, node_id: u32, name: String, ip: String) {
        self.register_node(node_id, name, ip)
    }

    fn get_cluster_info(&self) -> Option<ClusterInfo> {
        self.get_cluster_info()
    }

    fn get_cluster_version(&self) -> u64 {
        self.get_cluster_version()
    }

    fn increment_cluster_version(&self) {
        self.increment_cluster_version()
    }

    fn update_cluster_info(
        &self,
        cluster_name: String,
        config_version: u64,
        nodes: Vec<(u32, String, String)>,
    ) -> Result<()> {
        self.update_cluster_info(cluster_name, config_version, nodes)
    }

    fn set_node_online(&self, node_id: u32, online: bool) {
        self.set_node_online(node_id, online)
    }

    fn is_quorate(&self) -> bool {
        self.is_quorate()
    }

    fn set_quorate(&self, quorate: bool) {
        self.set_quorate(quorate)
    }

    fn get_members(&self) -> Vec<pmxcfs_api_types::MemberInfo> {
        self.get_members()
    }

    fn update_members(&self, members: Vec<pmxcfs_api_types::MemberInfo>) {
        self.update_members(members)
    }

    fn update_member_status(&self, member_list: &[u32]) {
        self.update_member_status(member_list)
    }

    fn get_start_time(&self) -> u64 {
        self.get_start_time()
    }

    fn increment_vmlist_version(&self) {
        self.increment_vmlist_version()
    }

    fn get_vmlist_version(&self) -> u64 {
        self.get_vmlist_version()
    }

    fn increment_path_version(&self, path: &str) {
        self.increment_path_version(path)
    }

    fn get_path_version(&self, path: &str) -> u64 {
        self.get_path_version(path)
    }

    fn get_all_path_versions(&self) -> HashMap<String, u64> {
        self.get_all_path_versions()
    }

    fn increment_all_path_versions(&self) {
        self.increment_all_path_versions()
    }

    fn set_node_kv(&self, nodeid: u32, key: String, value: Vec<u8>) {
        self.set_node_kv(nodeid, key, value)
    }

    fn get_node_kv(&self, nodeid: u32, key: &str) -> Option<Vec<u8>> {
        self.get_node_kv(nodeid, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ClusterLogEntry;
    use pmxcfs_api_types::VmType;

    /// Test helper: Create Status without rrdcached daemon (for unit tests)
    fn init_test_status() -> Arc<Status> {
        // Use pmxcfs-test-utils helper to create test config (matches C semantics)
        let config = pmxcfs_test_utils::create_test_config(false);
        Arc::new(Status::new(config, None))
    }

    #[tokio::test]
    async fn test_rrd_data_storage_and_retrieval() {
        let status = init_test_status();

        status.rrd_data.write().clear();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Test node RRD data format
        let node_data =
            format!("{now}:0:1.5:4:45.5:2.1:8000000000:6000000000:0:0:0:0:1000000:500000");
        let _ = status
            .set_rrd_data("pve2-node/testnode".to_string(), node_data.clone())
            .await;

        // Test VM RRD data format
        let vm_data = format!("{now}:1:60:4:2048:2048:10000:5000:1000:500:100:50");
        let _ = status
            .set_rrd_data("pve2.3-vm/100".to_string(), vm_data.clone())
            .await;

        // Get RRD dump
        let dump = status.get_rrd_dump();

        // Verify NUL terminator (C compatibility)
        assert!(dump.ends_with('\0'), "Dump should end with NUL terminator");

        // Strip NUL terminator for line-based checks
        let dump_str = dump.trim_end_matches('\0');

        // Verify both entries are present
        assert!(
            dump_str.contains("pve2-node/testnode"),
            "Should contain node entry"
        );
        assert!(dump_str.contains("pve2.3-vm/100"), "Should contain VM entry");

        // Verify format: each line should be "key:data"
        for line in dump_str.lines() {
            assert!(
                line.contains(':'),
                "Each line should contain colon separator"
            );
            let parts: Vec<&str> = line.split(':').collect();
            assert!(parts.len() > 1, "Each line should have key:data format");
        }

        assert_eq!(dump_str.lines().count(), 2, "Should have exactly 2 entries");
    }

    #[tokio::test]
    async fn test_rrd_data_aging() {
        let status = init_test_status();

        status.rrd_data.write().clear();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let recent_data =
            format!("{now}:0:1.5:4:45.5:2.1:8000000000:6000000000:0:0:0:0:1000000:500000");
        let _ = status
            .set_rrd_data("pve2-node/recent".to_string(), recent_data)
            .await;

        // Manually add an old entry (simulate time passing)
        let old_timestamp = now - 400; // 400 seconds ago (> 5 minutes)
        let old_data = format!(
            "{old_timestamp}:0:1.5:4:45.5:2.1:8000000000:6000000000:0:0:0:0:1000000:500000"
        );
        let entry = RrdEntry {
            key: "pve2-node/old".to_string(),
            data: old_data,
            timestamp: old_timestamp,
        };
        status
            .rrd_data
            .write()
            .insert("pve2-node/old".to_string(), entry);

        // Get dump - should trigger aging and remove old entry
        let dump = status.get_rrd_dump();

        assert!(
            dump.contains("pve2-node/recent"),
            "Recent entry should be present"
        );
        assert!(
            !dump.contains("pve2-node/old"),
            "Old entry should be aged out"
        );
    }

    #[tokio::test]
    async fn test_rrd_set_via_node_status() {
        let status = init_test_status();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Simulate receiving RRD data via IPC (like pvestatd sends)
        // Format matches C implementation: "timestamp:uptime:loadavg:maxcpu:cpu:iowait:memtotal:memused:swaptotal:swapused:roottotal:rootused:netin:netout"
        let node_data = format!("{now}:12345:1.5:8:0.5:0.1:16000:8000:4000:0:100:50:1000:2000");

        // Test the set_node_status method with "rrd/" prefix (matches C's cfs_status_set behavior)
        let result = status
            .set_node_status(
                "rrd/pve2-node/testnode".to_string(),
                node_data.as_bytes().to_vec(),
            )
            .await;
        assert!(
            result.is_ok(),
            "Should successfully set RRD data via node_status"
        );

        // Get the dump and verify
        let dump = status.get_rrd_dump();
        assert!(
            dump.contains("pve2-node/testnode"),
            "Should contain node metrics"
        );

        // Verify the data has the expected number of fields
        for line in dump.lines() {
            if line.starts_with("pve2-node/") {
                let parts: Vec<&str> = line.split(':').collect();
                // Format: key:timestamp:uptime:loadavg:maxcpu:cpu:iowait:memtotal:memused:swaptotal:swapused:roottotal:rootused:netin:netout
                // That's 1 (key) + 14 fields = 15 parts minimum
                assert!(
                    parts.len() >= 15,
                    "Node data should have at least 15 colon-separated fields, got {}",
                    parts.len()
                );
            }
        }
    }

    #[tokio::test]
    async fn test_rrd_multiple_updates() {
        let status = init_test_status();

        status.rrd_data.write().clear();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Add multiple entries
        for i in 0..5 {
            let data = format!(
                "{}:{}:1.5:4:45.5:2.1:8000000000:6000000000:0:0:0:0:1000000:500000",
                now + i,
                i
            );
            let _ = status
                .set_rrd_data(format!("pve2-node/node{i}"), data)
                .await;
        }

        let dump = status.get_rrd_dump();

        // Strip NUL terminator for line counting
        let dump_str = dump.trim_end_matches('\0');
        let count = dump_str.lines().count();
        assert_eq!(count, 5, "Should have 5 entries");

        // Verify each entry is present
        for i in 0..5 {
            assert!(
                dump.contains(&format!("pve2-node/node{i}")),
                "Should contain node{i}"
            );
        }
    }

    // ========== VM/CT Registry Tests ==========

    #[test]
    fn test_vm_registration() {
        let status = init_test_status();

        // Register a QEMU VM
        status.register_vm(100, VmType::Qemu, "node1".to_string());

        // Verify it exists
        assert!(status.vm_exists(100), "VM 100 should exist");

        // Verify version incremented (starts at 0, increments to 1)
        let vmlist_version = status.get_vmlist_version();
        assert!(vmlist_version > 0, "VM list version should increment");

        // Get VM list and verify entry
        let vmlist = status.get_vmlist();
        assert_eq!(vmlist.len(), 1, "Should have 1 VM");

        let vm = vmlist.get(&100).expect("VM 100 should be in list");
        assert_eq!(vm.vmid, 100);
        assert_eq!(vm.vmtype, VmType::Qemu);
        assert_eq!(vm.node, "node1");
        assert_eq!(vm.version, 1, "First registration should have version 1");
    }

    #[test]
    fn test_vm_deletion() {
        let status = init_test_status();

        // Register and then delete
        status.register_vm(100, VmType::Qemu, "node1".to_string());
        assert!(status.vm_exists(100), "VM should exist after registration");

        let version_before = status.get_vmlist_version();
        status.delete_vm(100);

        assert!(!status.vm_exists(100), "VM should not exist after deletion");

        let version_after = status.get_vmlist_version();
        assert!(
            version_after > version_before,
            "Version should increment on deletion"
        );

        let vmlist = status.get_vmlist();
        assert_eq!(vmlist.len(), 0, "VM list should be empty");
    }

    #[test]
    fn test_vm_multiple_registrations() {
        let status = init_test_status();

        // Register multiple VMs
        status.register_vm(100, VmType::Qemu, "node1".to_string());
        status.register_vm(101, VmType::Qemu, "node2".to_string());
        status.register_vm(200, VmType::Lxc, "node1".to_string());
        status.register_vm(201, VmType::Lxc, "node3".to_string());

        let vmlist = status.get_vmlist();
        assert_eq!(vmlist.len(), 4, "Should have 4 VMs");

        // Verify each VM
        assert_eq!(vmlist.get(&100).unwrap().vmtype, VmType::Qemu);
        assert_eq!(vmlist.get(&101).unwrap().node, "node2");
        assert_eq!(vmlist.get(&200).unwrap().vmtype, VmType::Lxc);
        assert_eq!(vmlist.get(&201).unwrap().node, "node3");
    }

    #[test]
    fn test_vm_re_registration_increments_version() {
        let status = init_test_status();

        // Register VM
        status.register_vm(100, VmType::Qemu, "node1".to_string());
        let vmlist = status.get_vmlist();
        let version1 = vmlist.get(&100).unwrap().version;
        assert_eq!(version1, 1, "First registration should have version 1");

        // Re-register same VM
        status.register_vm(100, VmType::Qemu, "node2".to_string());
        let vmlist = status.get_vmlist();
        let version2 = vmlist.get(&100).unwrap().version;
        assert_eq!(version2, 2, "Second registration should increment version");
        assert_eq!(
            vmlist.get(&100).unwrap().node,
            "node2",
            "Node should be updated"
        );
    }

    #[test]
    fn test_different_vm_exists() {
        let status = init_test_status();

        // Register VM 100 as QEMU on node1
        status.register_vm(100, VmType::Qemu, "node1".to_string());

        // Check if different VM exists - same type, different node
        assert!(
            status.different_vm_exists(100, VmType::Qemu, "node2"),
            "Should detect different node"
        );

        // Check if different VM exists - different type, same node
        assert!(
            status.different_vm_exists(100, VmType::Lxc, "node1"),
            "Should detect different type"
        );

        // Check if different VM exists - same type and node (should be false)
        assert!(
            !status.different_vm_exists(100, VmType::Qemu, "node1"),
            "Should not detect difference for identical VM"
        );

        // Check non-existent VM
        assert!(
            !status.different_vm_exists(999, VmType::Qemu, "node1"),
            "Non-existent VM should return false"
        );
    }

    // ========== Cluster Membership Tests ==========

    #[test]
    fn test_cluster_initialization() {
        let status = init_test_status();

        // Initially no cluster info
        assert!(
            status.get_cluster_info().is_none(),
            "Should have no cluster info initially"
        );

        // Initialize cluster
        status.init_cluster("test-cluster".to_string());

        let cluster_info = status.get_cluster_info();
        assert!(
            cluster_info.is_some(),
            "Cluster info should exist after init"
        );
        assert_eq!(cluster_info.unwrap().cluster_name, "test-cluster");

        let version = status.get_cluster_version();
        assert!(version > 0, "Cluster version should increment");
    }

    #[test]
    fn test_node_registration() {
        let status = init_test_status();

        status.init_cluster("test-cluster".to_string());

        // Register nodes
        status.register_node(1, "node1".to_string(), "192.168.1.10".to_string());
        status.register_node(2, "node2".to_string(), "192.168.1.11".to_string());

        let cluster_info = status
            .get_cluster_info()
            .expect("Cluster info should exist");
        assert_eq!(cluster_info.nodes_by_id.len(), 2, "Should have 2 nodes");
        assert_eq!(
            cluster_info.nodes_by_name.len(),
            2,
            "Should have 2 nodes by name"
        );

        let node1 = cluster_info
            .nodes_by_id
            .get(&1)
            .expect("Node 1 should exist");
        assert_eq!(node1.name, "node1");
        assert_eq!(node1.ip, "192.168.1.10");
        assert!(!node1.online, "Node should be offline initially");
    }

    #[test]
    fn test_node_online_status() {
        let status = init_test_status();

        status.init_cluster("test-cluster".to_string());
        status.register_node(1, "node1".to_string(), "192.168.1.10".to_string());

        // Set online
        status.set_node_online(1, true);
        let cluster_info = status.get_cluster_info().unwrap();
        assert!(
            cluster_info.nodes_by_id.get(&1).unwrap().online,
            "Node should be online"
        );
        assert!(
            cluster_info.get_node_by_name("node1").unwrap().online,
            "Node should be online in nodes_by_name too"
        );

        // Set offline
        status.set_node_online(1, false);
        let cluster_info = status.get_cluster_info().unwrap();
        assert!(
            !cluster_info.nodes_by_id.get(&1).unwrap().online,
            "Node should be offline"
        );
    }

    #[test]
    fn test_update_members() {
        let status = init_test_status();

        status.init_cluster("test-cluster".to_string());
        status.register_node(1, "node1".to_string(), "192.168.1.10".to_string());
        status.register_node(2, "node2".to_string(), "192.168.1.11".to_string());
        status.register_node(3, "node3".to_string(), "192.168.1.12".to_string());

        // Simulate CPG membership: nodes 1 and 3 are online
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let members = vec![
            pmxcfs_api_types::MemberInfo {
                node_id: 1,
                pid: 1000,
                joined_at: now,
            },
            pmxcfs_api_types::MemberInfo {
                node_id: 3,
                pid: 1002,
                joined_at: now,
            },
        ];
        status.update_members(members);

        let cluster_info = status.get_cluster_info().unwrap();
        assert!(
            cluster_info.nodes_by_id.get(&1).unwrap().online,
            "Node 1 should be online"
        );
        assert!(
            !cluster_info.nodes_by_id.get(&2).unwrap().online,
            "Node 2 should be offline"
        );
        assert!(
            cluster_info.nodes_by_id.get(&3).unwrap().online,
            "Node 3 should be online"
        );
    }

    #[test]
    fn test_quorum_state() {
        let status = init_test_status();

        // Initially not quorate
        assert!(!status.is_quorate(), "Should not be quorate initially");

        // Set quorate
        status.set_quorate(true);
        assert!(status.is_quorate(), "Should be quorate");

        // Unset quorate
        status.set_quorate(false);
        assert!(!status.is_quorate(), "Should not be quorate");
    }

    #[test]
    fn test_path_version_tracking() {
        let status = init_test_status();

        // Initial version should be 0
        assert_eq!(status.get_path_version("corosync.conf"), 0);

        // Increment version
        status.increment_path_version("corosync.conf");
        assert_eq!(status.get_path_version("corosync.conf"), 1);

        // Increment again
        status.increment_path_version("corosync.conf");
        assert_eq!(status.get_path_version("corosync.conf"), 2);

        // Non-tracked path should return 0
        assert_eq!(status.get_path_version("nonexistent.cfg"), 0);
    }

    #[test]
    fn test_all_path_versions() {
        let status = init_test_status();

        // Increment a few paths
        status.increment_path_version("corosync.conf");
        status.increment_path_version("corosync.conf");
        status.increment_path_version("storage.cfg");

        let all_versions = status.get_all_path_versions();

        // Should contain all tracked paths
        assert!(all_versions.contains_key("corosync.conf"));
        assert!(all_versions.contains_key("storage.cfg"));
        assert!(all_versions.contains_key("user.cfg"));

        // Verify specific versions
        assert_eq!(all_versions.get("corosync.conf"), Some(&2));
        assert_eq!(all_versions.get("storage.cfg"), Some(&1));
        assert_eq!(all_versions.get("user.cfg"), Some(&0));
    }

    #[test]
    fn test_vmlist_version_tracking() {
        let status = init_test_status();

        let initial_version = status.get_vmlist_version();

        status.increment_vmlist_version();
        assert_eq!(status.get_vmlist_version(), initial_version + 1);

        status.increment_vmlist_version();
        assert_eq!(status.get_vmlist_version(), initial_version + 2);
    }

    #[test]
    fn test_cluster_log_add_entry() {
        let status = init_test_status();

        let entry = ClusterLogEntry {
            uid: 0,
            timestamp: 1234567890,
            node: "node1".to_string(),
            priority: 6,
            pid: 0,
            ident: "pmxcfs".to_string(),
            tag: "startup".to_string(),
            message: "Test message".to_string(),
        };

        status.add_log_entry(entry);

        let entries = status.get_log_entries(10);
        assert_eq!(entries.len(), 1, "Should have 1 log entry");
        assert_eq!(entries[0].node, "node1");
        assert_eq!(entries[0].message, "Test message");
    }

    #[test]
    fn test_cluster_log_multiple_entries() {
        let status = init_test_status();

        // Add multiple entries
        for i in 0..5 {
            let entry = ClusterLogEntry {
                uid: 0,
                timestamp: 1234567890 + i,
                node: format!("node{i}"),
                priority: 6,
                pid: 0,
                ident: "test".to_string(),
                tag: "test".to_string(),
                message: format!("Message {i}"),
            };
            status.add_log_entry(entry);
        }

        let entries = status.get_log_entries(10);
        assert_eq!(entries.len(), 5, "Should have 5 log entries");
    }

    #[test]
    fn test_cluster_log_clear() {
        let status = init_test_status();

        // Add entries
        for i in 0..3 {
            let entry = ClusterLogEntry {
                uid: 0,
                timestamp: 1234567890 + i,
                node: "node1".to_string(),
                priority: 6,
                pid: 0,
                ident: "test".to_string(),
                tag: "test".to_string(),
                message: format!("Message {i}"),
            };
            status.add_log_entry(entry);
        }

        assert_eq!(status.get_log_entries(10).len(), 3, "Should have 3 entries");

        // Clear
        status.clear_cluster_log();

        assert_eq!(
            status.get_log_entries(10).len(),
            0,
            "Should have 0 entries after clear"
        );
    }

    #[test]
    fn test_kvstore_operations() {
        let status = init_test_status();

        // Initialize cluster and register nodes
        status.init_cluster("test-cluster".to_string());
        status.register_node(1, "node1".to_string(), "192.168.1.10".to_string());
        status.register_node(2, "node2".to_string(), "192.168.1.11".to_string());

        // Set some KV data
        status.set_node_kv(1, "ip".to_string(), b"192.168.1.10".to_vec());
        status.set_node_kv(1, "status".to_string(), b"online".to_vec());
        status.set_node_kv(2, "ip".to_string(), b"192.168.1.11".to_vec());

        // Get KV data
        let ip1 = status.get_node_kv(1, "ip");
        assert_eq!(ip1, Some(b"192.168.1.10".to_vec()));

        let status1 = status.get_node_kv(1, "status");
        assert_eq!(status1, Some(b"online".to_vec()));

        let ip2 = status.get_node_kv(2, "ip");
        assert_eq!(ip2, Some(b"192.168.1.11".to_vec()));

        // Test empty value removal (matches C behavior)
        status.set_node_kv(1, "ip".to_string(), vec![]);
        let ip1_after_remove = status.get_node_kv(1, "ip");
        assert_eq!(ip1_after_remove, None, "Empty value should remove the key");

        // Non-existent key
        let nonexistent = status.get_node_kv(1, "nonexistent");
        assert_eq!(nonexistent, None);

        // Non-existent node
        let nonexistent_node = status.get_node_kv(999, "ip");
        assert_eq!(nonexistent_node, None);

        // Test unknown node rejection
        status.set_node_kv(999, "unknown-key".to_string(), b"test".to_vec());
        let retrieved = status.get_node_kv(999, "unknown-key");
        assert_eq!(retrieved, None, "Unknown node should be rejected");
    }

    #[test]
    fn test_start_time() {
        let status = init_test_status();

        let start_time = status.get_start_time();
        assert!(start_time > 0, "Start time should be set");

        // Verify it's a recent timestamp (within last hour)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(now - start_time < 3600, "Start time should be recent");
    }

    #[test]
    fn test_get_local_nodename() {
        // Config is always required (matches C semantics where cfs is always present)
        let status = init_test_status();

        let nodename = status.get_local_nodename();
        assert_eq!(nodename, pmxcfs_test_utils::TEST_NODE_NAME, "Nodename should match test config");
        assert!(!nodename.is_empty(), "Nodename should not be empty");

        // Test with custom config
        let config = pmxcfs_config::Config::shared(
            "testnode".to_string(),
            "192.168.1.10".parse().unwrap(),
            33,
            false,
            false,
            "test-cluster".to_string(),
        );
        let status_custom = Arc::new(Status::new(config, None));

        let nodename = status_custom.get_local_nodename();
        assert_eq!(nodename, "testnode", "Nodename should match custom config");

        tracing::info!(nodename = %nodename, "Local nodename from config");
    }
}
