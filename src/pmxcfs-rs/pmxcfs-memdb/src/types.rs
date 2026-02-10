/// Type definitions for memdb module
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub(super) const MEMDB_MAX_FILE_SIZE: usize = 1024 * 1024; // 1 MiB (matches C version)
pub(super) const MEMDB_MAX_FSSIZE: usize = 128 * 1024 * 1024; // 128 MiB (matches C version)
pub(super) const MEMDB_MAX_INODES: usize = 256 * 1024; // 256k inodes (matches C version)
pub(super) const LOCK_TIMEOUT: u64 = 120; // Lock timeout in seconds
pub(super) const DT_DIR: u8 = 4; // Directory type
pub(super) const DT_REG: u8 = 8; // Regular file type

/// Root inode number (matches C implementation's memdb root inode)
/// IMPORTANT: This is the MEMDB root inode, which is 0 in both C and Rust.
/// The FUSE layer exposes this as inode 1 to the filesystem (FUSE_ROOT_ID).
/// See pmxcfs/src/fuse.rs for the inode mapping logic between memdb and FUSE.
pub const ROOT_INODE: u64 = 0;

/// Version file name (matches C VERSIONFILENAME)
/// Used to store root metadata as inode ROOT_INODE in the database
pub const VERSION_FILENAME: &str = "__version__";

/// Lock directory path (where cluster resource locks are stored)
/// Locks are implemented as directory entries stored at `priv/lock/<lockname>`
pub const LOCK_DIR_PATH: &str = "priv/lock";

/// Lock information for resource locking
///
/// In the C version (memdb.h:71-74), the lock info struct includes a `path` field
/// that serves as the hash table key. In Rust, we use `HashMap<String, LockInfo>`
/// where the path is stored as the HashMap key, so we don't duplicate it here.
#[derive(Clone, Debug)]
pub(crate) struct LockInfo {
    /// Lock timestamp (seconds since UNIX epoch)
    pub(crate) ltime: u64,

    /// Checksum of the locked resource (used to detect changes)
    pub(crate) csum: [u8; 32],
}

/// Tree entry representing a file or directory
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TreeEntry {
    pub inode: u64,
    pub parent: u64,
    pub version: u64,
    pub writer: u32,
    pub mtime: u32,
    pub size: usize,
    pub entry_type: u8, // DT_DIR or DT_REG
    pub name: String,
    pub data: Vec<u8>, // File data (empty for directories)
}

impl TreeEntry {
    pub fn is_dir(&self) -> bool {
        self.entry_type == DT_DIR
    }

    pub fn is_file(&self) -> bool {
        self.entry_type == DT_REG
    }

    /// Serialize TreeEntry to C-compatible wire format for Update messages
    ///
    /// Wire format (matches dcdb_send_update_inode):
    /// ```c
    /// [parent: u64][inode: u64][version: u64][writer: u32][mtime: u32]
    /// [size: u32][namelen: u32][type: u8][name: namelen bytes][data: size bytes]
    /// ```
    pub fn serialize_for_update(&self) -> Vec<u8> {
        let namelen = (self.name.len() + 1) as u32; // Include null terminator
        let header_size = 8 + 8 + 8 + 4 + 4 + 4 + 4 + 1; // 41 bytes
        let total_size = header_size + namelen as usize + self.data.len();

        let mut buf = Vec::with_capacity(total_size);

        // Header fields
        buf.extend_from_slice(&self.parent.to_le_bytes());
        buf.extend_from_slice(&self.inode.to_le_bytes());
        buf.extend_from_slice(&self.version.to_le_bytes());
        buf.extend_from_slice(&self.writer.to_le_bytes());
        buf.extend_from_slice(&self.mtime.to_le_bytes());
        buf.extend_from_slice(&(self.size as u32).to_le_bytes());
        buf.extend_from_slice(&namelen.to_le_bytes());
        buf.push(self.entry_type);

        // Name (null-terminated)
        buf.extend_from_slice(self.name.as_bytes());
        buf.push(0); // null terminator

        // Data (only for files)
        if self.entry_type == DT_REG && !self.data.is_empty() {
            buf.extend_from_slice(&self.data);
        }

        buf
    }

    /// Deserialize TreeEntry from C-compatible wire format
    ///
    /// Matches dcdb_parse_update_inode
    pub fn deserialize_from_update(data: &[u8]) -> anyhow::Result<Self> {
        if data.len() < 41 {
            anyhow::bail!(
                "Update message too short: {} bytes (need at least 41)",
                data.len()
            );
        }

        let mut offset = 0;

        // Parse header
        let parent = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
        offset += 8;
        let inode = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
        offset += 8;
        let version = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
        offset += 8;
        let writer = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let mtime = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let size = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let namelen = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let entry_type = data[offset];
        offset += 1;

        // Validate type
        if entry_type != DT_REG && entry_type != DT_DIR {
            anyhow::bail!("Invalid entry type: {entry_type}");
        }

        // Validate lengths
        if data.len() < offset + namelen + size {
            anyhow::bail!(
                "Update message too short: {} bytes (need {})",
                data.len(),
                offset + namelen + size
            );
        }

        // Parse name (null-terminated)
        let name_bytes = &data[offset..offset + namelen];
        if name_bytes.is_empty() || name_bytes[namelen - 1] != 0 {
            anyhow::bail!("Name not null-terminated");
        }
        let name = std::str::from_utf8(&name_bytes[..namelen - 1])
            .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in name: {e}"))?
            .to_string();
        offset += namelen;

        // Parse data
        let data_vec = if entry_type == DT_REG && size > 0 {
            data[offset..offset + size].to_vec()
        } else {
            Vec::new()
        };

        Ok(TreeEntry {
            inode,
            parent,
            version,
            writer,
            mtime,
            size,
            entry_type,
            name,
            data: data_vec,
        })
    }

    /// Compute SHA-256 checksum of this tree entry
    ///
    /// This checksum is used by the lock system to detect changes to lock directory entries.
    /// Matches C version's memdb_tree_entry_csum() function (memdb.c:1389).
    ///
    /// The checksum includes all entry metadata (inode, version, writer, mtime, size,
    /// entry_type, parent, name) and data (for files). This ensures any modification to a lock
    /// directory entry is detected, triggering lock timeout reset.
    ///
    /// CRITICAL: Field order and byte representation must match C exactly:
    /// 1. inode (u64, native endian)
    /// 2. version (u64, native endian)
    /// 3. writer (u32, native endian)
    /// 4. mtime (u32, native endian)
    /// 5. size (u32, native endian - C uses guint32)
    /// 6. entry_type (u8)
    /// 7. parent (u64, native endian)
    /// 8. name (bytes)
    /// 9. data (if present)
    pub fn compute_checksum(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Hash entry metadata in C's exact order (memdb.c:1389-1397)
        hasher.update(self.inode.to_ne_bytes());      // 1. inode
        hasher.update(self.version.to_ne_bytes());    // 2. version
        hasher.update(self.writer.to_ne_bytes());     // 3. writer
        hasher.update(self.mtime.to_ne_bytes());      // 4. mtime
        hasher.update((self.size as u32).to_ne_bytes()); // 5. size (C uses guint32)
        hasher.update([self.entry_type]);             // 6. type
        hasher.update(self.parent.to_ne_bytes());     // 7. parent
        hasher.update(self.name.as_bytes());          // 8. name

        // Hash data if present (memdb.c:1399-1400)
        if !self.data.is_empty() {
            hasher.update(&self.data);
        }

        hasher.finalize().into()
    }
}

/// Return type for load_from_db: (index, tree, root_inode, max_version)
pub(super) type LoadDbResult = (
    HashMap<u64, TreeEntry>,
    HashMap<u64, HashMap<String, u64>>,
    u64,
    u64,
);

#[cfg(test)]
mod tests {
    use super::*;

    // ===== TreeEntry Serialization Tests =====

    #[test]
    fn test_tree_entry_serialize_file_with_data() {
        let data = b"test file content".to_vec();
        let entry = TreeEntry {
            inode: 42,
            parent: 0,
            version: 1,
            writer: 100,
            name: "testfile.txt".to_string(),
            mtime: 1234567890,
            size: data.len(),
            entry_type: DT_REG,
            data: data.clone(),
        };

        let serialized = entry.serialize_for_update();

        // Should have: 41 bytes header + name + null + data
        let expected_size = 41 + entry.name.len() + 1 + data.len();
        assert_eq!(serialized.len(), expected_size);

        // Verify roundtrip
        let deserialized = TreeEntry::deserialize_from_update(&serialized).unwrap();
        assert_eq!(deserialized.inode, entry.inode);
        assert_eq!(deserialized.name, entry.name);
        assert_eq!(deserialized.size, entry.size);
        assert_eq!(deserialized.data, entry.data);
    }

    #[test]
    fn test_tree_entry_serialize_directory() {
        let entry = TreeEntry {
            inode: 10,
            parent: 0,
            version: 1,
            writer: 50,
            name: "mydir".to_string(),
            mtime: 1234567890,
            size: 0,
            entry_type: DT_DIR,
            data: Vec::new(),
        };

        let serialized = entry.serialize_for_update();

        // Should have: 41 bytes header + name + null (no data for directories)
        let expected_size = 41 + entry.name.len() + 1;
        assert_eq!(serialized.len(), expected_size);

        // Verify roundtrip
        let deserialized = TreeEntry::deserialize_from_update(&serialized).unwrap();
        assert_eq!(deserialized.inode, entry.inode);
        assert_eq!(deserialized.name, entry.name);
        assert_eq!(deserialized.entry_type, DT_DIR);
        assert!(
            deserialized.data.is_empty(),
            "Directories should have no data"
        );
    }

    #[test]
    fn test_tree_entry_deserialize_truncated_header() {
        // Only 40 bytes instead of required 41
        let data = vec![0u8; 40];

        let result = TreeEntry::deserialize_from_update(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too short"));
    }

    #[test]
    fn test_tree_entry_deserialize_invalid_type() {
        let mut data = vec![0u8; 100];
        // Set entry type to invalid value (not DT_REG or DT_DIR)
        data[40] = 99; // Invalid type

        let result = TreeEntry::deserialize_from_update(&data);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid entry type")
        );
    }

    #[test]
    fn test_tree_entry_deserialize_missing_name_terminator() {
        let mut data = vec![0u8; 100];

        // Set valid header fields
        data[40] = DT_REG; // entry_type at offset 40

        // Set namelen = 5 (at offset 32-35)
        data[32..36].copy_from_slice(&5u32.to_le_bytes());

        // Put name bytes WITHOUT null terminator
        data[41..46].copy_from_slice(b"test!");
        // Note: data[45] should be 0 for null terminator but we set it to '!'

        let result = TreeEntry::deserialize_from_update(&data);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not null-terminated")
        );
    }
}
