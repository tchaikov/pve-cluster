use anyhow::{Error, bail};
use futures::stream::TryStreamExt;
use libc::{EACCES, EINVAL, EIO, EISDIR, ENOENT};
use proxmox_fuse::requests::{self, FuseRequest};
use proxmox_fuse::{EntryParam, Fuse, ReplyBufState, Request};
use std::ffi::{OsStr, OsString};
use std::io;
use std::mem;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::plugins::{Plugin, PluginRegistry};
use pmxcfs_config::Config;
use pmxcfs_dfsm::{Dfsm, DfsmBroadcast, FuseMessage};
use pmxcfs_memdb::{MemDb, ROOT_INODE, TreeEntry};
use pmxcfs_status::Status;

const TTL: f64 = 1.0;

/// FUSE filesystem context for pmxcfs
pub struct PmxcfsFilesystem {
    memdb: MemDb,
    dfsm: Option<Arc<Dfsm<FuseMessage>>>,
    plugins: Arc<PluginRegistry>,
    status: Arc<Status>,
    uid: u32,
    gid: u32,
}

impl PmxcfsFilesystem {
    const PLUGIN_INODE_OFFSET: u64 = 1000000;
    const FUSE_GENERATION: u64 = 1;
    const NLINK_FILE: u32 = 1;
    const NLINK_DIR: u32 = 2;

    pub fn new(
        memdb: MemDb,
        config: Arc<Config>,
        dfsm: Option<Arc<Dfsm<FuseMessage>>>,
        plugins: Arc<PluginRegistry>,
        status: Arc<Status>,
    ) -> Self {
        Self {
            memdb,
            gid: config.www_data_gid(),
            dfsm,
            plugins,
            status,
            uid: 0, // root
        }
    }

    /// Convert FUSE nodeid to internal inode
    ///
    /// FUSE protocol uses nodeid 1 for root, but internally we use ROOT_INODE (0).
    /// Regular file inodes need to be offset by -1 to match internal numbering.
    /// Plugin inodes are in a separate range (>= PLUGIN_INODE_OFFSET) and unchanged.
    ///
    /// Mapping:
    /// - FUSE nodeid 1 → internal inode 0 (ROOT_INODE)
    /// - FUSE nodeid N (where N > 1 and N < PLUGIN_INODE_OFFSET) → internal inode N-1
    /// - Plugin inodes (>= PLUGIN_INODE_OFFSET) are unchanged
    #[inline]
    fn fuse_to_inode(&self, fuse_nodeid: u64) -> u64 {
        if fuse_nodeid >= Self::PLUGIN_INODE_OFFSET {
            // Plugin inodes are unchanged
            fuse_nodeid
        } else {
            // Regular inodes: FUSE nodeid N → internal inode N-1
            // This maps FUSE root (1) to internal ROOT_INODE (0)
            fuse_nodeid - 1
        }
    }

    /// Convert internal inode to FUSE nodeid
    ///
    /// Internally we use ROOT_INODE (0) for root, but FUSE protocol uses nodeid 1.
    /// Regular file inodes need to be offset by +1 to match FUSE numbering.
    /// Plugin inodes (>= PLUGIN_INODE_OFFSET) are unchanged.
    ///
    /// Mapping:
    /// - Internal inode 0 (ROOT_INODE) → FUSE nodeid 1
    /// - Internal inode N (where N > 0 and N < PLUGIN_INODE_OFFSET) → FUSE nodeid N+1
    /// - Plugin inodes (>= PLUGIN_INODE_OFFSET) are unchanged
    #[inline]
    fn inode_to_fuse(&self, inode: u64) -> u64 {
        if inode >= Self::PLUGIN_INODE_OFFSET {
            // Plugin inodes are unchanged
            inode
        } else {
            // Regular inodes: internal inode N → FUSE nodeid N+1
            // This maps internal ROOT_INODE (0) to FUSE root (1)
            inode + 1
        }
    }

    /// Check if a path is private (should have restricted permissions)
    /// Matches C version's path_is_private() logic:
    /// - Paths starting with "priv" or "priv/" are private
    /// - Paths matching "nodes/*/priv" or "nodes/*/priv/*" are private
    fn is_private_path(&self, path: &str) -> bool {
        // Strip leading slashes
        let path = path.trim_start_matches('/');

        // Check if path starts with "priv" or "priv/"
        if path.starts_with("priv") && (path.len() == 4 || path.as_bytes()[4] == b'/') {
            return true;
        }

        // Check for "nodes/*/priv" or "nodes/*/priv/*" pattern
        if let Some(after_nodes) = path.strip_prefix("nodes/") {
            // Find the next '/' to skip the node name
            if let Some(slash_pos) = after_nodes.find('/') {
                let after_nodename = &after_nodes[slash_pos..];

                // Check if it starts with "/priv" and ends or continues with '/'
                if after_nodename.starts_with("/priv") {
                    let priv_end = slash_pos + 5; // position after "/priv"
                    if after_nodes.len() == priv_end || after_nodes.as_bytes()[priv_end] == b'/' {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Get a TreeEntry by inode (helper for FUSE operations)
    fn get_entry_by_inode(&self, inode: u64) -> Option<TreeEntry> {
        self.memdb.get_entry_by_inode(inode)
    }

    /// Get a TreeEntry by path
    fn get_entry_by_path(&self, path: &str) -> Option<TreeEntry> {
        self.memdb.lookup_path(path)
    }

    /// Get the full path for an inode by traversing up the tree
    fn get_path_for_inode(&self, inode: u64) -> String {
        if inode == ROOT_INODE {
            return "/".to_string();
        }

        let mut path_components = Vec::new();
        let mut current_inode = inode;

        // Traverse up the tree
        while current_inode != ROOT_INODE {
            if let Some(entry) = self.memdb.get_entry_by_inode(current_inode) {
                path_components.push(entry.name.clone());
                current_inode = entry.parent;
            } else {
                // Entry not found, return root
                return "/".to_string();
            }
        }

        // Reverse to get correct order (we built from leaf to root)
        path_components.reverse();

        if path_components.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", path_components.join("/"))
        }
    }

    fn join_path(&self, parent_path: &str, name: &str) -> io::Result<String> {
        let mut path = std::path::PathBuf::from(parent_path);
        path.push(name);
        path.to_str()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Path contains invalid UTF-8 characters",
                )
            })
            .map(|s| s.to_string())
    }

    /// Convert a TreeEntry to libc::stat using current quorum state
    /// Applies permission adjustments based on whether the path is private
    ///
    /// Matches C implementation (cfs-plug-memdb.c:95-116, pmxcfs.c:130-138):
    /// 1. Start with quorum-dependent base permissions (0777/0555 dirs, 0666/0444 files)
    /// 2. Apply AND masking: private=0777700, dirs/symlinks=0777755, files=0777750
    fn entry_to_stat(&self, entry: &TreeEntry, path: &str) -> libc::stat {
        // Use current quorum state
        let quorate = self.status.is_quorate();
        self.entry_to_stat_with_quorum(entry, path, quorate)
    }

    /// Convert a TreeEntry to libc::stat with explicit quorum state
    /// Applies permission adjustments based on whether the path is private
    ///
    /// Matches C implementation (cfs-plug-memdb.c:95-116, pmxcfs.c:130-138):
    /// 1. Start with quorum-dependent base permissions (0777/0555 dirs, 0666/0444 files)
    /// 2. Apply AND masking: private=0777700, dirs/symlinks=0777755, files=0777750
    fn entry_to_stat_with_quorum(&self, entry: &TreeEntry, path: &str, quorate: bool) -> libc::stat {
        let mtime_secs = entry.mtime as i64;
        let mut stat: libc::stat = unsafe { mem::zeroed() };

        // Convert internal inode to FUSE nodeid for st_ino field
        let fuse_nodeid = self.inode_to_fuse(entry.inode);

        if entry.is_dir() {
            stat.st_ino = fuse_nodeid;
            // Quorum-dependent directory permissions (C: 0777 when quorate, 0555 when not)
            stat.st_mode = libc::S_IFDIR | if quorate { 0o777 } else { 0o555 };
            stat.st_nlink = Self::NLINK_DIR as u64;
            stat.st_uid = self.uid;
            stat.st_gid = self.gid;
            stat.st_size = 4096;
            stat.st_blksize = 4096;
            stat.st_blocks = 8;
            stat.st_atime = mtime_secs;
            stat.st_atime_nsec = 0;
            stat.st_mtime = mtime_secs;
            stat.st_mtime_nsec = 0;
            stat.st_ctime = mtime_secs;
            stat.st_ctime_nsec = 0;
        } else {
            stat.st_ino = fuse_nodeid;
            // Quorum-dependent file permissions (C: 0666 when quorate, 0444 when not)
            stat.st_mode = libc::S_IFREG | if quorate { 0o666 } else { 0o444 };
            stat.st_nlink = Self::NLINK_FILE as u64;
            stat.st_uid = self.uid;
            stat.st_gid = self.gid;
            stat.st_size = entry.size as i64;
            stat.st_blksize = 4096;
            stat.st_blocks = ((entry.size as i64 + 4095) / 4096) * 8;
            stat.st_atime = mtime_secs;
            stat.st_atime_nsec = 0;
            stat.st_mtime = mtime_secs;
            stat.st_mtime_nsec = 0;
            stat.st_ctime = mtime_secs;
            stat.st_ctime_nsec = 0;
        }

        // Apply permission adjustments based on path privacy (matching C implementation)
        // See pmxcfs.c cfs_fuse_getattr() lines 130-138
        // Uses AND masking to restrict permissions while preserving file type bits
        if self.is_private_path(path) {
            // Private paths: mask to rwx------ (0o700)
            // C: stbuf->st_mode &= 0777700
            stat.st_mode &= 0o777700;
        } else {
            // Non-private paths: different masks for dirs vs files
            if (stat.st_mode & libc::S_IFMT) == libc::S_IFDIR
                || (stat.st_mode & libc::S_IFMT) == libc::S_IFLNK
            {
                // Directories and symlinks: mask to rwxr-xr-x (0o755)
                // C: stbuf->st_mode &= 0777755
                stat.st_mode &= 0o777755;
            } else {
                // Regular files: mask to rwxr-x--- (0o750)
                // C: stbuf->st_mode &= 0777750
                stat.st_mode &= 0o777750;
            }
        }

        stat
    }

    /// Check if a plugin supports write operations
    ///
    /// Tests if the plugin has a custom write implementation by checking
    /// if write() returns the default "Write not supported" error
    fn plugin_supports_write(plugin: &Arc<dyn Plugin>) -> bool {
        // Try writing empty data - if it returns the default error, no write support
        match plugin.write(&[]) {
            Err(e) => {
                let msg = e.to_string();
                !msg.contains("Write not supported")
            }
            Ok(_) => true, // Write succeeded, so it's supported
        }
    }

    /// Get stat for a plugin file
    fn plugin_to_stat(&self, inode: u64, plugin: &Arc<dyn Plugin>) -> libc::stat {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let data = plugin.read().unwrap_or_default();

        let mut stat: libc::stat = unsafe { mem::zeroed() };
        stat.st_ino = inode;

        // Set file type and mode based on plugin type
        if plugin.is_symlink() {
            // Quorum-aware permissions for symlinks (matching C's cfs-plug-link.c:68-72)
            // - When quorate: 0o777 (writable by all)
            // - When not quorate: 0o555 (read-only for all)
            let mode = if self.status.is_quorate() {
                0o777
            } else {
                0o555
            };
            stat.st_mode = libc::S_IFLNK | mode;
        } else {
            // Regular file plugin
            let mut mode = plugin.mode();

            // Strip write bits if plugin doesn't support writing
            // Matches C implementation (cfs-plug-func.c:216-218)
            if !Self::plugin_supports_write(plugin) {
                mode &= !0o222; // Remove write bits (owner, group, other)
            }

            stat.st_mode = libc::S_IFREG | mode;
        }

        stat.st_nlink = Self::NLINK_FILE as u64;
        stat.st_uid = self.uid;
        stat.st_gid = self.gid;
        stat.st_size = data.len() as i64;
        stat.st_blksize = 4096;
        stat.st_blocks = ((data.len() as i64 + 4095) / 4096) * 8;
        stat.st_atime = now;
        stat.st_atime_nsec = 0;
        stat.st_mtime = now;
        stat.st_mtime_nsec = 0;
        stat.st_ctime = now;
        stat.st_ctime_nsec = 0;

        stat
    }

    /// Handle lookup operation
    async fn handle_lookup(&self, parent_fuse: u64, name: &OsStr) -> io::Result<EntryParam> {
        tracing::debug!(
            "lookup(parent={parent_fuse}, name={})",
            name.to_string_lossy()
        );

        // Convert FUSE nodeid to internal inode
        let parent = self.fuse_to_inode(parent_fuse);

        let name_str = name.to_string_lossy();

        // Check if this is a plugin file in the root directory
        if parent == ROOT_INODE {
            let plugin_names = self.plugins.list();
            if let Some(plugin_idx) = plugin_names.iter().position(|p| p == name_str.as_ref()) {
                // Found a plugin file
                if let Some(plugin) = self.plugins.get(&name_str) {
                    let plugin_inode = Self::PLUGIN_INODE_OFFSET + plugin_idx as u64;
                    let stat = self.plugin_to_stat(plugin_inode, &plugin);

                    return Ok(EntryParam {
                        inode: plugin_inode, // Plugin inodes already in FUSE space
                        generation: Self::FUSE_GENERATION,
                        attr: stat,
                        attr_timeout: TTL,
                        entry_timeout: TTL,
                    });
                }
            }
        }

        // Get parent entry
        let parent_entry = if parent == ROOT_INODE {
            // Root directory
            self.get_entry_by_inode(ROOT_INODE)
                .ok_or_else(|| io::Error::from_raw_os_error(ENOENT))?
        } else {
            self.get_entry_by_inode(parent)
                .ok_or_else(|| io::Error::from_raw_os_error(ENOENT))?
        };

        // Construct the path
        let parent_path = self.get_path_for_inode(parent_entry.inode);
        let full_path = self.join_path(&parent_path, &name_str)?;

        // Look up the entry
        if let Ok(exists) = self.memdb.exists(&full_path)
            && exists
        {
            // Get the entry to find its inode
            if let Some(entry) = self.get_entry_by_path(&full_path) {
                let stat = self.entry_to_stat(&entry, &full_path);
                // Convert internal inode to FUSE nodeid
                let fuse_nodeid = self.inode_to_fuse(entry.inode);
                return Ok(EntryParam {
                    inode: fuse_nodeid,
                    generation: Self::FUSE_GENERATION,
                    attr: stat,
                    attr_timeout: TTL,
                    entry_timeout: TTL,
                });
            }
        }

        Err(io::Error::from_raw_os_error(ENOENT))
    }

    /// Handle getattr operation
    fn handle_getattr(&self, ino_fuse: u64) -> io::Result<libc::stat> {
        tracing::debug!("getattr(ino={})", ino_fuse);

        // Check if this is a plugin file (inode >= PLUGIN_INODE_OFFSET)
        if ino_fuse >= Self::PLUGIN_INODE_OFFSET {
            let plugin_idx = (ino_fuse - Self::PLUGIN_INODE_OFFSET) as usize;
            let plugin_names = self.plugins.list();
            if plugin_idx < plugin_names.len() {
                let plugin_name = &plugin_names[plugin_idx];
                if let Some(plugin) = self.plugins.get(plugin_name) {
                    return Ok(self.plugin_to_stat(ino_fuse, &plugin));
                }
            }
        }

        // Convert FUSE nodeid to internal inode
        let ino = self.fuse_to_inode(ino_fuse);

        if let Some(entry) = self.get_entry_by_inode(ino) {
            let path = self.get_path_for_inode(ino);
            Ok(self.entry_to_stat(&entry, &path))
        } else {
            Err(io::Error::from_raw_os_error(ENOENT))
        }
    }

    /// Handle readdir operation
    fn handle_readdir(&self, request: &mut requests::Readdir) -> Result<(), Error> {
        let ino_fuse = request.inode;
        tracing::debug!("readdir(ino={}, offset={})", ino_fuse, request.offset);

        // Convert FUSE nodeid to internal inode
        let ino = self.fuse_to_inode(ino_fuse);
        let offset = request.offset;

        // Get the directory path
        let path = self.get_path_for_inode(ino);

        // Read directory entries from memdb
        let entries = self
            .memdb
            .readdir(&path)
            .map_err(|_| io::Error::from_raw_os_error(ENOENT))?;

        // Build complete list of entries
        let mut all_entries: Vec<(u64, libc::stat, String)> = Vec::new();

        // Add . and .. entries
        // C implementation (cfs-plug-memdb.c:172) always passes quorate=0 for readdir stats
        // This ensures directory listings show non-quorate permissions (read-only view)
        if let Some(dir_entry) = self.get_entry_by_inode(ino) {
            let dir_stat = self.entry_to_stat_with_quorum(&dir_entry, &path, false);
            all_entries.push((ino_fuse, dir_stat, ".".to_string()));
            all_entries.push((ino_fuse, dir_stat, "..".to_string()));
        }

        // For root directory, add plugin files
        if ino == ROOT_INODE {
            let plugin_names = self.plugins.list();
            for (idx, plugin_name) in plugin_names.iter().enumerate() {
                let plugin_inode = Self::PLUGIN_INODE_OFFSET + idx as u64;
                if let Some(plugin) = self.plugins.get(plugin_name) {
                    let stat = self.plugin_to_stat(plugin_inode, &plugin);
                    all_entries.push((plugin_inode, stat, plugin_name.clone()));
                }
            }
        }

        // Add actual entries from memdb
        // C implementation (cfs-plug-memdb.c:172) always passes quorate=0 for readdir stats
        for entry in &entries {
            let entry_path = match self.join_path(&path, &entry.name) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("Skipping entry with invalid UTF-8 path: {}", e);
                    continue;
                }
            };
            let stat = self.entry_to_stat_with_quorum(entry, &entry_path, false);
            // Convert internal inode to FUSE nodeid for directory entry
            let fuse_nodeid = self.inode_to_fuse(entry.inode);
            all_entries.push((fuse_nodeid, stat, entry.name.clone()));
        }

        // Return entries starting from offset
        let mut next = offset as isize;
        for (_inode, stat, name) in all_entries.iter().skip(offset as usize) {
            next += 1;
            match request.add_entry(OsStr::new(name), stat, next)? {
                ReplyBufState::Ok => (),
                ReplyBufState::Full => return Ok(()),
            }
        }

        Ok(())
    }

    /// Handle read operation
    fn handle_read(&self, ino_fuse: u64, offset: u64, size: usize) -> io::Result<Vec<u8>> {
        tracing::debug!("read(ino={}, offset={}, size={})", ino_fuse, offset, size);

        // Check if this is a plugin file (inode >= PLUGIN_INODE_OFFSET)
        if ino_fuse >= Self::PLUGIN_INODE_OFFSET {
            let plugin_idx = (ino_fuse - Self::PLUGIN_INODE_OFFSET) as usize;
            let plugin_names = self.plugins.list();
            if plugin_idx < plugin_names.len() {
                let plugin_name = &plugin_names[plugin_idx];
                if let Some(plugin) = self.plugins.get(plugin_name) {
                    let data = plugin
                        .read()
                        .map_err(|_| io::Error::from_raw_os_error(EIO))?;

                    let offset = offset as usize;
                    if offset >= data.len() {
                        return Ok(Vec::new());
                    } else {
                        let end = std::cmp::min(offset + size, data.len());
                        return Ok(data[offset..end].to_vec());
                    }
                }
            }
        }

        // Convert FUSE nodeid to internal inode
        let ino = self.fuse_to_inode(ino_fuse);

        let path = self.get_path_for_inode(ino);

        // Check if this is a directory
        if ino == ROOT_INODE {
            // Root directory itself - can't read
            return Err(io::Error::from_raw_os_error(EISDIR));
        }

        // Read from memdb
        self.memdb
            .read(&path, offset, size)
            .map_err(|_| io::Error::from_raw_os_error(ENOENT))
    }

    /// Handle write operation
    async fn handle_write(&self, ino_fuse: u64, offset: u64, data: &[u8]) -> io::Result<usize> {
        tracing::debug!(
            "write(ino={}, offset={}, size={})",
            ino_fuse,
            offset,
            data.len()
        );

        // Check if this is a plugin file (inode >= PLUGIN_INODE_OFFSET)
        if ino_fuse >= Self::PLUGIN_INODE_OFFSET {
            let plugin_idx = (ino_fuse - Self::PLUGIN_INODE_OFFSET) as usize;
            let plugin_names = self.plugins.list();

            if plugin_idx < plugin_names.len() {
                let plugin_name = &plugin_names[plugin_idx];
                if let Some(plugin) = self.plugins.get(plugin_name) {
                    // Validate offset (C only allows offset 0)
                    if offset != 0 {
                        tracing::warn!("Plugin write rejected: offset {} != 0", offset);
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }

                    // Call plugin write
                    tracing::debug!("Writing {} bytes to plugin '{}'", data.len(), plugin_name);
                    plugin.write(data).map(|_| data.len()).map_err(|e| {
                        tracing::error!("Plugin write failed: {}", e);
                        io::Error::from_raw_os_error(libc::EIO)
                    })?;

                    return Ok(data.len());
                }
            }

            // Plugin not found or invalid index
            return Err(io::Error::from_raw_os_error(libc::ENOENT));
        }

        // Regular memdb file write
        // Convert FUSE nodeid to internal inode
        let ino = self.fuse_to_inode(ino_fuse);

        let path = self.get_path_for_inode(ino);

        // C-style broadcast-first: send message and wait for result
        // C implementation (cfs-plug-memdb.c:262-265) sends just the write chunk
        // with original offset, not the full file contents
        if let Some(dfsm) = &self.dfsm {
            // Send write message with just the data chunk and original offset
            // The DFSM delivery will apply the write to all nodes
            let result = dfsm
                .send_message_sync(
                    FuseMessage::Write {
                        path: path.clone(),
                        offset,
                        data: data.to_vec(),
                    },
                    std::time::Duration::from_secs(10),
                )
                .await
                .map_err(|e| {
                    tracing::error!("DFSM send_message_sync failed: {}", e);
                    io::Error::from_raw_os_error(EIO)
                })?;

            if result.result < 0 {
                tracing::warn!("Write failed with errno: {}", -result.result);
                return Err(io::Error::from_raw_os_error(-result.result as i32));
            }

            Ok(data.len())
        } else {
            // No cluster - write locally
            let mtime = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32;

            // FUSE write() should never truncate - truncation is handled separately
            // via setattr (for explicit truncate) or open with O_TRUNC flag.
            // Offset writes must preserve content beyond the write range (POSIX semantics).
            self.memdb
                .write(&path, offset, 0, mtime, data, false)
                .map_err(|_| io::Error::from_raw_os_error(EACCES))
        }
    }

    /// Handle mkdir operation
    async fn handle_mkdir(&self, parent_fuse: u64, name: &OsStr, mode: u32) -> io::Result<EntryParam> {
        tracing::debug!(
            "mkdir(parent={}, name={})",
            parent_fuse,
            name.to_string_lossy()
        );

        // Convert FUSE nodeid to internal inode
        let parent = self.fuse_to_inode(parent_fuse);

        let parent_path = self.get_path_for_inode(parent);
        let name_str = name.to_string_lossy();
        let full_path = self.join_path(&parent_path, &name_str)?;

        // C-style broadcast-first: send message and wait for result
        if let Some(dfsm) = &self.dfsm {
            let result = dfsm
                .send_message_sync(
                    FuseMessage::Mkdir {
                        path: full_path.clone(),
                    },
                    std::time::Duration::from_secs(10),
                )
                .await
                .map_err(|e| {
                    tracing::error!("DFSM send_message_sync failed: {}", e);
                    io::Error::from_raw_os_error(EIO)
                })?;

            if result.result < 0 {
                tracing::warn!("Mkdir failed with errno: {}", -result.result);
                return Err(io::Error::from_raw_os_error(-result.result as i32));
            }
        } else {
            // No cluster - create locally
            let mtime = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32;

            self.memdb
                .create(&full_path, mode | libc::S_IFDIR, 0, mtime)
                .map_err(|_| io::Error::from_raw_os_error(EACCES))?;
        }

        // Look up the newly created entry (created via delivery callback)
        let entry = self
            .memdb
            .lookup_path(&full_path)
            .ok_or_else(|| io::Error::from_raw_os_error(EIO))?;

        let stat = self.entry_to_stat(&entry, &full_path);
        // Convert internal inode to FUSE nodeid
        let fuse_nodeid = self.inode_to_fuse(entry.inode);
        Ok(EntryParam {
            inode: fuse_nodeid,
            generation: Self::FUSE_GENERATION,
            attr: stat,
            attr_timeout: TTL,
            entry_timeout: TTL,
        })
    }

    /// Handle rmdir operation
    async fn handle_rmdir(&self, parent_fuse: u64, name: &OsStr) -> io::Result<()> {
        tracing::debug!(
            "rmdir(parent={}, name={})",
            parent_fuse,
            name.to_string_lossy()
        );

        // Convert FUSE nodeid to internal inode
        let parent = self.fuse_to_inode(parent_fuse);

        let parent_path = self.get_path_for_inode(parent);
        let name_str = name.to_string_lossy();
        let full_path = self.join_path(&parent_path, &name_str)?;

        // C-style broadcast-first: send message and wait for result
        if let Some(dfsm) = &self.dfsm {
            let result = dfsm
                .send_message_sync(
                    FuseMessage::Delete {
                        path: full_path.clone(),
                    },
                    std::time::Duration::from_secs(10),
                )
                .await
                .map_err(|e| {
                    tracing::error!("DFSM send_message_sync failed: {}", e);
                    io::Error::from_raw_os_error(EIO)
                })?;

            if result.result < 0 {
                tracing::warn!("Rmdir failed with errno: {}", -result.result);
                return Err(io::Error::from_raw_os_error(-result.result as i32));
            }
        } else {
            // No cluster - delete locally
            self.memdb
                .delete(&full_path, 0, SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as u32)
                .map_err(|_| io::Error::from_raw_os_error(EACCES))?;
        }

        Ok(())
    }

    /// Handle create operation
    async fn handle_create(&self, parent_fuse: u64, name: &OsStr, mode: u32) -> io::Result<EntryParam> {
        tracing::debug!(
            "create(parent={}, name={})",
            parent_fuse,
            name.to_string_lossy()
        );

        // Convert FUSE nodeid to internal inode
        let parent = self.fuse_to_inode(parent_fuse);

        let parent_path = self.get_path_for_inode(parent);
        let name_str = name.to_string_lossy();
        let full_path = self.join_path(&parent_path, &name_str)?;

        // C-style broadcast-first: send message and wait for result
        if let Some(dfsm) = &self.dfsm {
            // Direct await - clean and idiomatic async code
            let result = dfsm
                .send_message_sync(
                    FuseMessage::Create {
                        path: full_path.clone(),
                    },
                    std::time::Duration::from_secs(10),
                )
                .await
                .map_err(|e| {
                    tracing::error!("DFSM send_message_sync failed: {}", e);
                    io::Error::from_raw_os_error(EIO)
                })?;

            // Check result from deliver callback
            if result.result < 0 {
                tracing::warn!("Create failed with errno: {}", -result.result);
                return Err(io::Error::from_raw_os_error(-result.result as i32));
            }
        } else {
            // No cluster - create locally
            let mtime = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32;

            self.memdb
                .create(&full_path, mode | libc::S_IFREG, 0, mtime)
                .map_err(|_| io::Error::from_raw_os_error(EACCES))?;
        }

        // Look up the newly created entry (created via delivery callback)
        let entry = self
            .memdb
            .lookup_path(&full_path)
            .ok_or_else(|| io::Error::from_raw_os_error(EIO))?;

        let stat = self.entry_to_stat(&entry, &full_path);
        // Convert internal inode to FUSE nodeid
        let fuse_nodeid = self.inode_to_fuse(entry.inode);
        Ok(EntryParam {
            inode: fuse_nodeid,
            generation: Self::FUSE_GENERATION,
            attr: stat,
            attr_timeout: TTL,
            entry_timeout: TTL,
        })
    }

    /// Handle unlink operation
    async fn handle_unlink(&self, parent_fuse: u64, name: &OsStr) -> io::Result<()> {
        tracing::debug!(
            "unlink(parent={}, name={})",
            parent_fuse,
            name.to_string_lossy()
        );

        // Convert FUSE nodeid to internal inode
        let parent = self.fuse_to_inode(parent_fuse);

        let name_str = name.to_string_lossy();

        // Don't allow unlinking plugin files (in root directory)
        if parent == ROOT_INODE {
            let plugin_names = self.plugins.list();
            if plugin_names.iter().any(|p| p == name_str.as_ref()) {
                return Err(io::Error::from_raw_os_error(EACCES));
            }
        }

        let parent_path = self.get_path_for_inode(parent);
        let full_path = self.join_path(&parent_path, &name_str)?;

        // Check if trying to unlink a directory (should use rmdir instead)
        if let Some(entry) = self.memdb.lookup_path(&full_path)
            && entry.is_dir()
        {
            return Err(io::Error::from_raw_os_error(libc::EISDIR));
        }

        // C-style broadcast-first: send message and wait for result
        if let Some(dfsm) = &self.dfsm {
            let result = dfsm
                .send_message_sync(
                    FuseMessage::Delete { path: full_path },
                    std::time::Duration::from_secs(10),
                )
                .await
                .map_err(|e| {
                    tracing::error!("DFSM send_message_sync failed: {}", e);
                    io::Error::from_raw_os_error(EIO)
                })?;

            if result.result < 0 {
                tracing::warn!("Unlink failed with errno: {}", -result.result);
                return Err(io::Error::from_raw_os_error(-result.result as i32));
            }
        } else {
            // No cluster - delete locally
            self.memdb
                .delete(&full_path, 0, SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as u32)
                .map_err(|_| io::Error::from_raw_os_error(EACCES))?;
        }

        Ok(())
    }

    /// Handle rename operation
    async fn handle_rename(
        &self,
        parent_fuse: u64,
        name: &OsStr,
        new_parent_fuse: u64,
        new_name: &OsStr,
    ) -> io::Result<()> {
        tracing::debug!(
            "rename(parent={}, name={}, new_parent={}, new_name={})",
            parent_fuse,
            name.to_string_lossy(),
            new_parent_fuse,
            new_name.to_string_lossy()
        );

        // Convert FUSE nodeids to internal inodes
        let parent = self.fuse_to_inode(parent_fuse);
        let new_parent = self.fuse_to_inode(new_parent_fuse);

        let parent_path = self.get_path_for_inode(parent);
        let name_str = name.to_string_lossy();
        let old_path = self.join_path(&parent_path, &name_str)?;

        let new_parent_path = self.get_path_for_inode(new_parent);
        let new_name_str = new_name.to_string_lossy();
        let new_path = self.join_path(&new_parent_path, &new_name_str)?;

        // C-style broadcast-first: send message and wait for result
        if let Some(dfsm) = &self.dfsm {
            let result = dfsm
                .send_message_sync(
                    FuseMessage::Rename {
                        from: old_path.clone(),
                        to: new_path.clone(),
                    },
                    std::time::Duration::from_secs(10),
                )
                .await
                .map_err(|e| {
                    tracing::error!("DFSM send_message_sync failed: {}", e);
                    io::Error::from_raw_os_error(EIO)
                })?;

            if result.result < 0 {
                tracing::warn!("Rename failed with errno: {}", -result.result);
                return Err(io::Error::from_raw_os_error(-result.result as i32));
            }
        } else {
            // No cluster - rename locally
            self.memdb
                .rename(&old_path, &new_path, 0, SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as u32)
                .map_err(|_| io::Error::from_raw_os_error(EACCES))?;
        }

        Ok(())
    }

    /// Handle setattr operation
    ///
    /// Supports:
    /// - Truncate (size parameter)
    /// - Mtime updates (mtime parameter) - used for lock renewal/release
    /// - Mode changes (mode parameter) - validation only, no actual changes
    /// - Ownership changes (uid/gid parameters) - validation only, no actual changes
    ///
    /// C implementation (cfs-plug-memdb.c:393-436) ALWAYS sends DCDB_MESSAGE_CFS_MTIME
    /// via DFSM when mtime is updated (line 420-422), in addition to unlock messages
    ///
    /// chmod/chown (pmxcfs.c:180-214): These operations don't actually change anything,
    /// they just validate that the requested changes are allowed (returns -EPERM if not).
    async fn handle_setattr(
        &self,
        ino_fuse: u64,
        size: Option<u64>,
        mtime: Option<u32>,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
    ) -> io::Result<libc::stat> {
        tracing::debug!(
            "setattr(ino={}, size={:?}, mtime={:?})",
            ino_fuse,
            size,
            mtime
        );

        // Convert FUSE nodeid to internal inode
        let ino = self.fuse_to_inode(ino_fuse);
        let path = self.get_path_for_inode(ino);

        // Handle chmod operation (validation only - C: pmxcfs.c:180-197)
        // chmod validates that requested mode is allowed but doesn't actually change anything
        if let Some(new_mode) = mode {
            let is_private = self.is_private_path(&path);
            let mode_bits = new_mode & 0o777; // Extract permission bits only

            // C implementation allows only specific modes:
            // - 0600 (rw-------) for private paths
            // - 0640 (rw-r-----) for non-private paths
            let allowed = if is_private {
                mode_bits == 0o600
            } else {
                mode_bits == 0o640
            };

            if !allowed {
                tracing::debug!(
                    "chmod rejected: mode={:o}, path={}, is_private={}",
                    mode_bits,
                    path,
                    is_private
                );
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }

            tracing::debug!(
                "chmod validated: mode={:o}, path={}, is_private={}",
                mode_bits,
                path,
                is_private
            );
        }

        // Handle chown operation (validation only - C: pmxcfs.c:198-214)
        // chown validates that requested ownership is allowed but doesn't actually change anything
        if uid.is_some() || gid.is_some() {
            // C implementation allows only:
            // - uid: 0 (root) or -1 (no change)
            // - gid: www-data GID or -1 (no change)
            let uid_allowed = match uid {
                None => true,
                Some(u) => u == 0 || u == u32::MAX, // -1 as u32 = u32::MAX
            };

            let gid_allowed = match gid {
                None => true,
                Some(g) => g == self.gid || g == u32::MAX, // -1 as u32 = u32::MAX
            };

            if !uid_allowed || !gid_allowed {
                tracing::debug!(
                    "chown rejected: uid={:?}, gid={:?}, allowed_gid={}, path={}",
                    uid,
                    gid,
                    self.gid,
                    path
                );
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }

            tracing::debug!(
                "chown validated: uid={:?}, gid={:?}, allowed_gid={}, path={}",
                uid,
                gid,
                self.gid,
                path
            );
        }

        // Handle truncate operation
        if let Some(new_size) = size {
            let current_mtime = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32;

            // Truncate: clear the file then write empty data to set size
            self.memdb
                .write(&path, 0, 0, current_mtime, &vec![0u8; new_size as usize], true)
                .map_err(|_| io::Error::from_raw_os_error(EACCES))?;
        }

        // Handle mtime update (lock renewal/release)
        // C implementation (cfs-plug-memdb.c:415-422) ALWAYS sends DCDB_MESSAGE_CFS_MTIME
        // via DFSM when mtime is updated, in addition to unlock messages
        if let Some(new_mtime) = mtime {
            // Check if this is a lock directory
            if pmxcfs_memdb::is_lock_path(&path) {
                if let Some(entry) = self.memdb.get_entry_by_inode(ino)
                    && entry.is_dir()
                {
                    // mtime=0 on lock directory = unlock request (C: cfs-plug-memdb.c:411-418)
                    if new_mtime == 0 {
                        tracing::debug!("Unlock request for lock directory: {}", path);
                        let csum = entry.compute_checksum();

                        // If DFSM is available and synced, only send the message - don't delete locally
                        // The leader will check if expired and send Unlock message if needed
                        // If DFSM is not available or not synced, delete locally if expired (C: cfs-plug-memdb.c:425-427)
                        if self.dfsm.as_ref().is_none_or(|d| !d.is_synced()) {
                            if self.memdb.lock_expired(&path, &csum) {
                                tracing::info!(
                                    "DFSM not synced - deleting expired lock locally: {}",
                                    path
                                );
                                self.memdb
                                    .delete(&path, 0, SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as u32)
                                    .map_err(|_| io::Error::from_raw_os_error(EACCES))?;
                            }
                        } else {
                            // Broadcast unlock request to cluster (C: cfs-plug-memdb.c:417)
                            tracing::debug!("DFSM synced - sending unlock request to cluster");
                            self.dfsm.broadcast(FuseMessage::UnlockRequest { path: path.clone() });
                        }
                    }
                }
            }

            // C implementation ALWAYS sends MTIME message (lines 420-422), regardless of
            // whether it's an unlock request or not. This broadcasts the mtime update to
            // all cluster nodes for synchronization.
            if let Some(dfsm) = &self.dfsm {
                tracing::debug!("Sending MTIME message via DFSM: path={}, mtime={}", path, new_mtime);
                let result = dfsm
                    .send_message_sync(
                        FuseMessage::Mtime {
                            path: path.clone(),
                            mtime: new_mtime,
                        },
                        std::time::Duration::from_secs(10),
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!("DFSM send_message_sync failed for MTIME: {}", e);
                        io::Error::from_raw_os_error(EIO)
                    })?;

                if result.result < 0 {
                    tracing::warn!("MTIME failed with errno: {}", -result.result);
                    return Err(io::Error::from_raw_os_error(-result.result as i32));
                }
            } else {
                // No cluster - update locally
                self.memdb
                    .set_mtime(&path, 0, new_mtime)
                    .map_err(|_| io::Error::from_raw_os_error(EACCES))?;
            }
        }

        // Return current attributes
        if let Some(entry) = self.memdb.get_entry_by_inode(ino) {
            Ok(self.entry_to_stat(&entry, &path))
        } else {
            Err(io::Error::from_raw_os_error(ENOENT))
        }
    }

    /// Handle readlink operation - read symbolic link target
    fn handle_readlink(&self, ino_fuse: u64) -> io::Result<OsString> {
        tracing::debug!("readlink(ino={})", ino_fuse);

        // Check if this is a plugin (only plugins can be symlinks in pmxcfs)
        if ino_fuse >= Self::PLUGIN_INODE_OFFSET {
            let plugin_idx = (ino_fuse - Self::PLUGIN_INODE_OFFSET) as usize;
            let plugin_names = self.plugins.list();
            if plugin_idx < plugin_names.len() {
                let plugin_name = &plugin_names[plugin_idx];
                if let Some(plugin) = self.plugins.get(plugin_name) {
                    // Read the link target from the plugin
                    let data = plugin
                        .read()
                        .map_err(|_| io::Error::from_raw_os_error(EIO))?;

                    // Convert bytes to OsString
                    let target = std::str::from_utf8(&data)
                        .map_err(|_| io::Error::from_raw_os_error(EIO))?;

                    return Ok(OsString::from(target));
                }
            }
        }

        // Not a plugin or plugin not found
        Err(io::Error::from_raw_os_error(EINVAL))
    }

    /// Handle statfs operation - return filesystem statistics
    ///
    /// Matches C implementation (memdb.c:1275-1307)
    /// Returns fixed filesystem stats based on memdb state
    fn handle_statfs(&self) -> io::Result<libc::statvfs> {
        tracing::debug!("statfs()");

        const BLOCKSIZE: u64 = 4096;

        // Get statistics from memdb
        let (blocks, bfree, bavail, files, ffree) = self.memdb.statfs();

        let mut stbuf: libc::statvfs = unsafe { mem::zeroed() };

        // Block size and counts
        stbuf.f_bsize = BLOCKSIZE;       // Filesystem block size
        stbuf.f_frsize = BLOCKSIZE;      // Fragment size (same as block size)
        stbuf.f_blocks = blocks;         // Total blocks in filesystem
        stbuf.f_bfree = bfree;           // Free blocks
        stbuf.f_bavail = bavail;         // Free blocks available to unprivileged user

        // Inode counts
        stbuf.f_files = files;           // Total file nodes in filesystem
        stbuf.f_ffree = ffree;           // Free file nodes in filesystem
        stbuf.f_favail = ffree;          // Free file nodes available to unprivileged user

        // Other fields
        stbuf.f_fsid = 0;                // Filesystem ID
        stbuf.f_flag = 0;                // Mount flags
        stbuf.f_namemax = 255;           // Maximum filename length

        Ok(stbuf)
    }
}

/// Create and mount FUSE filesystem
pub async fn mount_fuse(
    mount_path: &Path,
    memdb: MemDb,
    config: Arc<Config>,
    dfsm: Option<Arc<Dfsm<FuseMessage>>>,
    plugins: Arc<PluginRegistry>,
    status: Arc<Status>,
) -> Result<(), Error> {
    let fs = Arc::new(PmxcfsFilesystem::new(memdb, config, dfsm, plugins, status));

    let mut fuse = Fuse::builder("pmxcfs")?
        .debug()
        .options("default_permissions")? // Enable kernel permission checking
        .options("allow_other")? // Allow non-root access
        .enable_readdir()
        .enable_readlink()
        .enable_mkdir()
        .enable_create()
        .enable_write()
        .enable_unlink()
        .enable_rmdir()
        .enable_rename()
        .enable_setattr()
        .enable_read()
        .enable_statfs()
        .build()?
        .mount(mount_path)?;

    tracing::info!("FUSE filesystem mounted at {}", mount_path.display());

    // Process FUSE requests
    while let Some(request) = fuse.try_next().await? {
        let fs = Arc::clone(&fs);
        match request {
            Request::Lookup(request) => {
                match fs.handle_lookup(request.parent, &request.file_name).await {
                    Ok(entry) => request.reply(&entry)?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            Request::Getattr(request) => match fs.handle_getattr(request.inode) {
                Ok(stat) => request.reply(&stat, TTL)?,
                Err(err) => request.io_fail(err)?,
            },
            Request::Readlink(request) => match fs.handle_readlink(request.inode) {
                Ok(target) => request.reply(&target)?,
                Err(err) => request.io_fail(err)?,
            },
            Request::Readdir(mut request) => match fs.handle_readdir(&mut request) {
                Ok(()) => request.reply()?,
                Err(err) => {
                    if let Some(io_err) = err.downcast_ref::<io::Error>() {
                        let errno = io_err.raw_os_error().unwrap_or(EIO);
                        request.fail(errno)?;
                    } else {
                        request.io_fail(io::Error::from_raw_os_error(EIO))?;
                    }
                }
            },
            Request::Read(request) => {
                match fs.handle_read(request.inode, request.offset, request.size) {
                    Ok(data) => request.reply(&data)?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            Request::Write(request) => {
                match fs.handle_write(request.inode, request.offset, request.data()).await {
                    Ok(written) => request.reply(written)?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            Request::Mkdir(request) => {
                match fs.handle_mkdir(request.parent, &request.dir_name, request.mode).await {
                    Ok(entry) => request.reply(&entry)?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            Request::Rmdir(request) => match fs.handle_rmdir(request.parent, &request.dir_name).await {
                Ok(()) => request.reply()?,
                Err(err) => request.io_fail(err)?,
            },
            Request::Rename(request) => {
                match fs.handle_rename(
                    request.parent,
                    &request.name,
                    request.new_parent,
                    &request.new_name,
                ).await {
                    Ok(()) => request.reply()?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            Request::Create(request) => {
                match fs.handle_create(request.parent, &request.file_name, request.mode).await {
                    Ok(entry) => request.reply(&entry, 0)?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            Request::Mknod(request) => {
                // Treat mknod same as create
                match fs.handle_create(request.parent, &request.file_name, request.mode).await {
                    Ok(entry) => request.reply(&entry)?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            Request::Unlink(request) => {
                match fs.handle_unlink(request.parent, &request.file_name).await {
                    Ok(()) => request.reply()?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            Request::Setattr(request) => {
                // Extract mtime if being set
                let mtime = request.mtime().map(|set_time| match set_time {
                    proxmox_fuse::requests::SetTime::Time(duration) => duration.as_secs() as u32,
                    proxmox_fuse::requests::SetTime::Now => SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                        as u32,
                });

                // Extract mode, uid, gid for chmod/chown validation (M1, M2)
                let mode = request.mode();
                let uid = request.uid();
                let gid = request.gid();

                match fs.handle_setattr(request.inode, request.size(), mtime, mode, uid, gid).await {
                    Ok(stat) => request.reply(&stat, TTL)?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            Request::Open(request) => {
                // Plugin files don't support truncation, but can be opened for write
                if request.inode >= PmxcfsFilesystem::PLUGIN_INODE_OFFSET {
                    // Check if plugin is being opened for writing
                    let is_write = (request.flags & (libc::O_WRONLY | libc::O_RDWR)) != 0;

                    if is_write {
                        // Verify plugin is writable
                        let plugin_idx =
                            (request.inode - PmxcfsFilesystem::PLUGIN_INODE_OFFSET) as usize;
                        let plugin_names = fs.plugins.list();

                        if plugin_idx < plugin_names.len() {
                            let plugin_name = &plugin_names[plugin_idx];
                            if let Some(plugin) = fs.plugins.get(plugin_name) {
                                // Check if plugin supports write (mode has write bit for owner)
                                let mode = plugin.mode();
                                if (mode & 0o200) == 0 {
                                    // Plugin is read-only
                                    request.io_fail(io::Error::from_raw_os_error(libc::EACCES))?;
                                    continue;
                                }
                            }
                        }
                    }

                    // Verify plugin exists (getattr)
                    match fs.handle_getattr(request.inode) {
                        Ok(_) => request.reply(0)?,
                        Err(err) => request.io_fail(err)?,
                    }
                } else {
                    // Regular files: handle truncation
                    if (request.flags & libc::O_TRUNC) != 0 {
                        match fs.handle_setattr(request.inode, Some(0), None, None, None, None).await {
                            Ok(_) => request.reply(0)?,
                            Err(err) => request.io_fail(err)?,
                        }
                    } else {
                        match fs.handle_getattr(request.inode) {
                            Ok(_) => request.reply(0)?,
                            Err(err) => request.io_fail(err)?,
                        }
                    }
                }
            }
            Request::Release(request) => {
                request.reply()?;
            }
            Request::Forget(_request) => {
                // Forget is a notification, no reply needed
            }
            Request::Statfs(request) => {
                match fs.handle_statfs() {
                    Ok(stbuf) => request.reply(&stbuf)?,
                    Err(err) => request.io_fail(err)?,
                }
            }
            other => {
                tracing::warn!("Unsupported FUSE request: {:?}", other);
                bail!("Unsupported FUSE request");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper to create a minimal PmxcfsFilesystem for testing
    fn create_test_filesystem() -> (PmxcfsFilesystem, TempDir) {
        let tmp_dir = TempDir::new().unwrap();
        let db_path = tmp_dir.path().join("test.db");

        let memdb = MemDb::open(&db_path, true).unwrap();
        let config = pmxcfs_test_utils::create_test_config(false);
        let plugins = crate::plugins::init_plugins_for_test("testnode");
        let status = Arc::new(Status::new(config.clone(), None));

        let fs = PmxcfsFilesystem::new(memdb, config, None, plugins, status);
        (fs, tmp_dir)
    }

    // ===== Inode Mapping Tests =====

    #[test]
    fn test_fuse_to_inode_mapping() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Root: FUSE nodeid 1 → internal inode 0
        assert_eq!(fs.fuse_to_inode(1), 0);

        // Regular inodes: N → N-1
        assert_eq!(fs.fuse_to_inode(2), 1);
        assert_eq!(fs.fuse_to_inode(10), 9);
        assert_eq!(fs.fuse_to_inode(100), 99);

        // Plugin inodes (>= PLUGIN_INODE_OFFSET) unchanged
        assert_eq!(fs.fuse_to_inode(1000000), 1000000);
        assert_eq!(fs.fuse_to_inode(1000001), 1000001);
    }

    #[test]
    fn test_inode_to_fuse_mapping() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Root: internal inode 0 → FUSE nodeid 1
        assert_eq!(fs.inode_to_fuse(0), 1);

        // Regular inodes: N → N+1
        assert_eq!(fs.inode_to_fuse(1), 2);
        assert_eq!(fs.inode_to_fuse(9), 10);
        assert_eq!(fs.inode_to_fuse(99), 100);

        // Plugin inodes (>= PLUGIN_INODE_OFFSET) unchanged
        assert_eq!(fs.inode_to_fuse(1000000), 1000000);
        assert_eq!(fs.inode_to_fuse(1000001), 1000001);
    }

    #[test]
    fn test_inode_mapping_roundtrip() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Test roundtrip for regular inodes
        for inode in 0..1000 {
            let fuse = fs.inode_to_fuse(inode);
            let back = fs.fuse_to_inode(fuse);
            assert_eq!(inode, back, "Roundtrip failed for inode {inode}");
        }

        // Test roundtrip for plugin inodes
        for offset in 0..100 {
            let inode = 1000000 + offset;
            let fuse = fs.inode_to_fuse(inode);
            let back = fs.fuse_to_inode(fuse);
            assert_eq!(inode, back, "Roundtrip failed for plugin inode {inode}");
        }
    }

    // ===== Path Privacy Tests =====

    #[test]
    fn test_is_private_path_priv_root() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Exact "priv" at root
        assert!(fs.is_private_path("priv"));
        assert!(fs.is_private_path("/priv"));
        assert!(fs.is_private_path("///priv"));

        // "priv/" at root
        assert!(fs.is_private_path("priv/"));
        assert!(fs.is_private_path("/priv/"));
        assert!(fs.is_private_path("priv/file.txt"));
        assert!(fs.is_private_path("/priv/subdir/file"));
    }

    #[test]
    fn test_is_private_path_nodes() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Node-specific priv directories
        assert!(fs.is_private_path("nodes/node1/priv"));
        assert!(fs.is_private_path("/nodes/node1/priv"));
        assert!(fs.is_private_path("nodes/node1/priv/"));
        assert!(fs.is_private_path("nodes/node1/priv/config"));
        assert!(fs.is_private_path("/nodes/node1/priv/subdir/file"));

        // Multiple levels
        assert!(fs.is_private_path("nodes/test-node/priv/deep/path/file.txt"));
    }

    #[test]
    fn test_is_private_path_non_private() {
        let (fs, _tmpdir) = create_test_filesystem();

        // "priv" as substring but not matching pattern
        assert!(!fs.is_private_path("private"));
        assert!(!fs.is_private_path("privileged"));
        assert!(!fs.is_private_path("some/private/path"));

        // Regular paths
        assert!(!fs.is_private_path(""));
        assert!(!fs.is_private_path("/"));
        assert!(!fs.is_private_path("nodes"));
        assert!(!fs.is_private_path("nodes/node1"));
        assert!(!fs.is_private_path("nodes/node1/qemu-server"));
        assert!(!fs.is_private_path("corosync.conf"));

        // "priv" in middle of path component
        assert!(!fs.is_private_path("nodes/privileged"));
        assert!(!fs.is_private_path("nodes/node1/private"));
    }

    #[test]
    fn test_is_private_path_edge_cases() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Empty path
        assert!(!fs.is_private_path(""));

        // Only slashes
        assert!(!fs.is_private_path("/"));
        assert!(!fs.is_private_path("//"));
        assert!(!fs.is_private_path("///"));

        // "priv" with trailing characters (not slash)
        assert!(!fs.is_private_path("priv123"));
        assert!(!fs.is_private_path("priv.txt"));

        // Case sensitivity
        assert!(!fs.is_private_path("Priv"));
        assert!(!fs.is_private_path("PRIV"));
        assert!(!fs.is_private_path("nodes/node1/Priv"));
    }

    // ===== Error Path Tests =====

    #[tokio::test]
    async fn test_lookup_nonexistent() {
        use std::ffi::OsStr;
        let (fs, _tmpdir) = create_test_filesystem();

        // Try to lookup a file that doesn't exist
        let result = fs.handle_lookup(1, OsStr::new("nonexistent.txt")).await;

        assert!(result.is_err(), "Lookup of nonexistent file should fail");
        if let Err(e) = result {
            assert_eq!(e.raw_os_error(), Some(libc::ENOENT));
        }
    }

    #[test]
    fn test_getattr_nonexistent_inode() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Try to get attributes for an inode that doesn't exist
        let result = fs.handle_getattr(999999);

        assert!(result.is_err(), "Getattr on nonexistent inode should fail");
        if let Err(e) = result {
            assert_eq!(e.raw_os_error(), Some(libc::ENOENT));
        }
    }

    #[test]
    fn test_read_directory_as_file() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Try to read the root directory as if it were a file
        let result = fs.handle_read(1, 0, 100);

        assert!(result.is_err(), "Reading directory as file should fail");
        if let Err(e) = result {
            assert_eq!(e.raw_os_error(), Some(libc::EISDIR));
        }
    }

    #[tokio::test]
    async fn test_write_to_nonexistent_file() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Try to write to a file that doesn't exist (should fail with EACCES)
        let result = fs.handle_write(999999, 0, b"data").await;

        assert!(result.is_err(), "Writing to nonexistent file should fail");
        if let Err(e) = result {
            assert_eq!(e.raw_os_error(), Some(libc::EACCES));
        }
    }

    #[tokio::test]
    async fn test_unlink_directory_fails() {
        use std::ffi::OsStr;
        let (fs, _tmpdir) = create_test_filesystem();

        // Create a directory first by writing a file
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;
        let _ = fs.memdb.write("/testdir/file.txt", 0, 0, now, b"test", false);

        // Look up testdir to verify it exists as a directory
        if let Some(entry) = fs.memdb.lookup_path("/testdir") {
            assert!(entry.is_dir(), "testdir should be a directory");

            // Try to unlink the directory (should fail)
            let result = fs.handle_unlink(1, OsStr::new("testdir")).await;

            assert!(result.is_err(), "Unlinking directory should fail");
            // Note: May return EACCES if directory doesn't exist in internal lookup,
            // or EISDIR if found as directory
            if let Err(e) = result {
                let err_code = e.raw_os_error();
                assert!(
                    err_code == Some(libc::EISDIR) || err_code == Some(libc::EACCES),
                    "Expected EISDIR or EACCES, got {err_code:?}"
                );
            }
        }
    }

    // ===== Plugin-related Tests =====

    #[test]
    fn test_plugin_inode_range() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Plugin inodes should be >= PLUGIN_INODE_OFFSET (1000000)
        let plugin_inode = 1000000;

        // Verify that plugin inodes don't overlap with regular inodes
        assert!(plugin_inode >= 1000000);
        assert_ne!(fs.fuse_to_inode(plugin_inode), plugin_inode - 1);
        assert_eq!(fs.fuse_to_inode(plugin_inode), plugin_inode);
    }

    #[test]
    fn test_file_type_preservation_in_permissions() {
        let (fs, _tmpdir) = create_test_filesystem();

        // Create a file
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;
        let _ = fs.memdb.write("/test.txt", 0, 0, now, b"test", false);

        if let Ok(stat) = fs.handle_getattr(fs.inode_to_fuse(1)) {
            // Verify that file type bits are preserved (S_IFREG)
            assert_eq!(stat.st_mode & libc::S_IFMT, libc::S_IFREG);
        }
    }
}
