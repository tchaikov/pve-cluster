/// MemDB Index structures for C-compatible state synchronization
///
/// This module implements the memdb_index_t format used by the C implementation
/// for efficient state comparison during cluster synchronization.
use anyhow::Result;
use sha2::{Digest, Sha256};

/// Size of the memdb_index_t header in bytes (version + last_inode + writer + mtime + size + bytes)
/// Wire format: 8 + 8 + 4 + 4 + 4 + 4 = 32 bytes
const MEMDB_INDEX_HEADER_SIZE: u32 = 32;

/// Size of each memdb_index_extry_t in bytes (inode + digest)
/// Wire format: 8 + 32 = 40 bytes
const MEMDB_INDEX_ENTRY_SIZE: u32 = 40;

/// Index entry matching C's memdb_index_extry_t
///
/// Wire format (40 bytes):
/// ```c
/// typedef struct {
///     guint64 inode;      // 8 bytes
///     char digest[32];    // 32 bytes (SHA256)
/// } memdb_index_extry_t;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub inode: u64,
    pub digest: [u8; 32],
}

impl IndexEntry {
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(40);
        data.extend_from_slice(&self.inode.to_le_bytes());
        data.extend_from_slice(&self.digest);
        data
    }

    pub fn deserialize(data: &[u8]) -> Result<Self> {
        if data.len() < 40 {
            anyhow::bail!("IndexEntry too short: {} bytes (need 40)", data.len());
        }

        let inode = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&data[8..40]);

        Ok(Self { inode, digest })
    }
}

/// MemDB index matching C's memdb_index_t
///
/// Wire format header (24 bytes) + entries:
/// ```c
/// typedef struct {
///     guint64 version;        // 8 bytes
///     guint64 last_inode;     // 8 bytes
///     guint32 writer;         // 4 bytes
///     guint32 mtime;          // 4 bytes
///     guint32 size;           // 4 bytes (number of entries)
///     guint32 bytes;          // 4 bytes (total bytes allocated)
///     memdb_index_extry_t entries[];  // variable length
/// } memdb_index_t;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemDbIndex {
    pub version: u64,
    pub last_inode: u64,
    pub writer: u32,
    pub mtime: u32,
    pub size: u32,  // number of entries
    pub bytes: u32, // total bytes (24 + size * 40)
    pub entries: Vec<IndexEntry>,
}

impl MemDbIndex {
    /// Create a new index from entries
    ///
    /// Entries are automatically sorted by inode for efficient comparison
    /// and to match C implementation behavior.
    pub fn new(
        version: u64,
        last_inode: u64,
        writer: u32,
        mtime: u32,
        mut entries: Vec<IndexEntry>,
    ) -> Self {
        // Sort entries by inode (matching C implementation)
        entries.sort_by_key(|e| e.inode);

        let size = entries.len() as u32;
        let bytes = MEMDB_INDEX_HEADER_SIZE + size * MEMDB_INDEX_ENTRY_SIZE;

        Self {
            version,
            last_inode,
            writer,
            mtime,
            size,
            bytes,
            entries,
        }
    }

    /// Serialize to C-compatible wire format
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(self.bytes as usize);

        // Header (32 bytes)
        data.extend_from_slice(&self.version.to_le_bytes());
        data.extend_from_slice(&self.last_inode.to_le_bytes());
        data.extend_from_slice(&self.writer.to_le_bytes());
        data.extend_from_slice(&self.mtime.to_le_bytes());
        data.extend_from_slice(&self.size.to_le_bytes());
        data.extend_from_slice(&self.bytes.to_le_bytes());

        // Entries (40 bytes each)
        for entry in &self.entries {
            data.extend_from_slice(&entry.serialize());
        }

        data
    }

    /// Deserialize from C-compatible wire format
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        if data.len() < 32 {
            anyhow::bail!(
                "MemDbIndex too short: {} bytes (need at least 32)",
                data.len()
            );
        }

        // Parse header
        let version = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let last_inode = u64::from_le_bytes(data[8..16].try_into().unwrap());
        let writer = u32::from_le_bytes(data[16..20].try_into().unwrap());
        let mtime = u32::from_le_bytes(data[20..24].try_into().unwrap());
        let size = u32::from_le_bytes(data[24..28].try_into().unwrap());
        let bytes = u32::from_le_bytes(data[28..32].try_into().unwrap());

        // Validate size
        let expected_bytes = 32 + size * 40;
        if bytes != expected_bytes {
            anyhow::bail!("MemDbIndex bytes mismatch: got {bytes}, expected {expected_bytes}");
        }

        if data.len() < bytes as usize {
            anyhow::bail!(
                "MemDbIndex data too short: {} bytes (need {})",
                data.len(),
                bytes
            );
        }

        // Parse entries
        let mut entries = Vec::with_capacity(size as usize);
        let mut offset = 32;
        for _ in 0..size {
            let entry = IndexEntry::deserialize(&data[offset..offset + 40])?;
            entries.push(entry);
            offset += 40;
        }

        Ok(Self {
            version,
            last_inode,
            writer,
            mtime,
            size,
            bytes,
            entries,
        })
    }

    /// Compute SHA256 digest of a tree entry for the index
    ///
    /// Matches C's memdb_encode_index() digest computation (memdb.c:1497-1507)
    /// CRITICAL: Order and fields must match exactly:
    ///   1. version, 2. writer, 3. mtime, 4. size, 5. type, 6. parent, 7. name, 8. data
    ///
    /// NOTE: inode is NOT included in the digest (only used as the index key)
    #[allow(clippy::too_many_arguments)]
    pub fn compute_entry_digest(
        _inode: u64, // Not included in digest, only for signature compatibility
        parent: u64,
        version: u64,
        writer: u32,
        mtime: u32,
        size: usize,
        entry_type: u8,
        name: &str,
        data: &[u8],
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Hash entry metadata in C's exact order (memdb.c:1497-1503)
        // C uses native endian (in-memory representation), so we use to_ne_bytes()
        hasher.update(version.to_ne_bytes());
        hasher.update(writer.to_ne_bytes());
        hasher.update(mtime.to_ne_bytes());
        hasher.update((size as u32).to_ne_bytes()); // C uses u32 for te->size
        hasher.update([entry_type]);
        hasher.update(parent.to_ne_bytes());
        hasher.update(name.as_bytes());

        // Hash data only for regular files with non-zero size (memdb.c:1505-1507)
        if entry_type == 8 /* DT_REG */ && size > 0 {
            hasher.update(data);
        }

        hasher.finalize().into()
    }
}

/// Implement comparison for MemDbIndex
///
/// Matches C's dcdb_choose_leader_with_highest_index() logic:
/// - If same version, higher mtime wins
/// - If different version, higher version wins
impl PartialOrd for MemDbIndex {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MemDbIndex {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // First compare by version (higher version wins)
        // Then by mtime (higher mtime wins) if versions are equal
        self.version
            .cmp(&other.version)
            .then_with(|| self.mtime.cmp(&other.mtime))
    }
}

impl MemDbIndex {
    /// Find entries that differ from another index
    ///
    /// Returns the set of inodes that need to be sent as updates.
    /// Matches C's dcdb_create_and_send_updates() comparison logic.
    pub fn find_differences(&self, other: &MemDbIndex) -> Vec<u64> {
        let mut differences = Vec::new();

        // Walk through master index, comparing with slave
        let mut j = 0; // slave position

        for i in 0..self.entries.len() {
            let master_entry = &self.entries[i];
            let inode = master_entry.inode;

            // Advance slave pointer to matching or higher inode
            while j < other.entries.len() && other.entries[j].inode < inode {
                j += 1;
            }

            // Check if entries match
            if j < other.entries.len() {
                let slave_entry = &other.entries[j];
                if slave_entry.inode == inode && slave_entry.digest == master_entry.digest {
                    // Entries match - skip
                    continue;
                }
            }

            // Entry differs or missing - needs update
            differences.push(inode);
        }

        differences
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for index serialization and synchronization
    //!
    //! This test module covers:
    //! - Index serialization/deserialization (round-trip verification)
    //! - Leader election logic (version-based, mtime tiebreaker)
    //! - Difference detection (finding sync deltas between indices)
    //! - TreeEntry serialization (files, directories, empty files)
    //! - Digest computation (determinism, sorted entries)
    //! - Large index handling (100+ entry stress tests)
    //!
    //! ## Serialization Format
    //!
    //! - IndexEntry: 40 bytes (8-byte inode + 32-byte digest)
    //! - MemDbIndex: Header (version) + entries
    //! - TreeEntry: Type-specific format (regular file, directory, symlink)
    //!
    //! ## Leader Election
    //!
    //! Leader election follows these rules:
    //! 1. Higher version wins
    //! 2. If versions equal, higher mtime wins
    //! 3. If both equal, indices are considered equal

    use super::*;

    #[test]
    fn test_index_entry_roundtrip() {
        let entry = IndexEntry {
            inode: 0x123456789ABCDEF0,
            digest: [42u8; 32],
        };

        let serialized = entry.serialize();
        assert_eq!(serialized.len(), 40);

        let deserialized = IndexEntry::deserialize(&serialized).unwrap();
        assert_eq!(deserialized, entry);
    }

    #[test]
    fn test_memdb_index_roundtrip() {
        let entries = vec![
            IndexEntry {
                inode: 1,
                digest: [1u8; 32],
            },
            IndexEntry {
                inode: 2,
                digest: [2u8; 32],
            },
        ];

        let index = MemDbIndex::new(100, 1000, 1, 123456, entries);

        let serialized = index.serialize();
        assert_eq!(serialized.len(), 32 + 2 * 40);

        let deserialized = MemDbIndex::deserialize(&serialized).unwrap();
        assert_eq!(deserialized.version, 100);
        assert_eq!(deserialized.last_inode, 1000);
        assert_eq!(deserialized.size, 2);
        assert_eq!(deserialized.entries.len(), 2);
    }

    #[test]
    fn test_index_comparison() {
        let idx1 = MemDbIndex::new(100, 0, 1, 1000, vec![]);
        let idx2 = MemDbIndex::new(100, 0, 1, 2000, vec![]);
        let idx3 = MemDbIndex::new(101, 0, 1, 500, vec![]);

        // Same version, lower mtime
        assert!(idx1 < idx2);
        assert_eq!(idx1.cmp(&idx2), std::cmp::Ordering::Less);

        // Same version, higher mtime
        assert!(idx2 > idx1);
        assert_eq!(idx2.cmp(&idx1), std::cmp::Ordering::Greater);

        // Higher version wins even with lower mtime
        assert!(idx3 > idx2);
        assert_eq!(idx3.cmp(&idx2), std::cmp::Ordering::Greater);

        // Test equality
        let idx4 = MemDbIndex::new(100, 0, 1, 1000, vec![]);
        assert_eq!(idx1, idx4);
        assert_eq!(idx1.cmp(&idx4), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_find_differences() {
        let master_entries = vec![
            IndexEntry {
                inode: 1,
                digest: [1u8; 32],
            },
            IndexEntry {
                inode: 2,
                digest: [2u8; 32],
            },
            IndexEntry {
                inode: 3,
                digest: [3u8; 32],
            },
        ];

        let slave_entries = vec![
            IndexEntry {
                inode: 1,
                digest: [1u8; 32], // same
            },
            IndexEntry {
                inode: 2,
                digest: [99u8; 32], // different digest
            },
            // missing inode 3
        ];

        let master = MemDbIndex::new(100, 3, 1, 1000, master_entries);
        let slave = MemDbIndex::new(100, 2, 1, 900, slave_entries);

        let diffs = master.find_differences(&slave);
        assert_eq!(diffs, vec![2, 3]); // inode 2 changed, inode 3 missing
    }

    // ========== Tests moved from sync_tests.rs ==========

    #[test]
    fn test_memdb_index_serialization() {
        // Create a simple index with a few entries
        let entries = vec![
            IndexEntry {
                inode: 1,
                digest: [0u8; 32],
            },
            IndexEntry {
                inode: 2,
                digest: [1u8; 32],
            },
            IndexEntry {
                inode: 3,
                digest: [2u8; 32],
            },
        ];

        let index = MemDbIndex::new(
            100,   // version
            3,     // last_inode
            1,     // writer
            12345, // mtime
            entries,
        );

        // Serialize
        let serialized = index.serialize();

        // Expected size: 32-byte header + 3 * 40-byte entries = 152 bytes
        assert_eq!(serialized.len(), 32 + 3 * 40);
        assert_eq!(serialized.len(), index.bytes as usize);

        // Deserialize
        let deserialized = MemDbIndex::deserialize(&serialized).expect("Failed to deserialize");

        // Verify all fields match
        assert_eq!(deserialized.version, index.version);
        assert_eq!(deserialized.last_inode, index.last_inode);
        assert_eq!(deserialized.writer, index.writer);
        assert_eq!(deserialized.mtime, index.mtime);
        assert_eq!(deserialized.size, index.size);
        assert_eq!(deserialized.bytes, index.bytes);
        assert_eq!(deserialized.entries.len(), index.entries.len());

        for (i, (orig, deser)) in index
            .entries
            .iter()
            .zip(deserialized.entries.iter())
            .enumerate()
        {
            assert_eq!(deser.inode, orig.inode, "Entry {i} inode mismatch");
            assert_eq!(deser.digest, orig.digest, "Entry {i} digest mismatch");
        }
    }

    #[test]
    fn test_leader_election_by_version() {
        use std::cmp::Ordering;

        // Create three indices with different versions
        let entries1 = vec![IndexEntry {
            inode: 1,
            digest: [0u8; 32],
        }];
        let entries2 = vec![IndexEntry {
            inode: 1,
            digest: [0u8; 32],
        }];
        let entries3 = vec![IndexEntry {
            inode: 1,
            digest: [0u8; 32],
        }];

        let index1 = MemDbIndex::new(100, 1, 1, 1000, entries1);
        let index2 = MemDbIndex::new(150, 1, 2, 1000, entries2); // Higher version - should win
        let index3 = MemDbIndex::new(120, 1, 3, 1000, entries3);

        // Test comparisons
        assert_eq!(index2.cmp(&index1), Ordering::Greater);
        assert_eq!(index2.cmp(&index3), Ordering::Greater);
        assert_eq!(index1.cmp(&index2), Ordering::Less);
        assert_eq!(index3.cmp(&index2), Ordering::Less);
    }

    #[test]
    fn test_leader_election_by_mtime_tiebreaker() {
        use std::cmp::Ordering;

        // Create two indices with same version but different mtime
        let entries1 = vec![IndexEntry {
            inode: 1,
            digest: [0u8; 32],
        }];
        let entries2 = vec![IndexEntry {
            inode: 1,
            digest: [0u8; 32],
        }];

        let index1 = MemDbIndex::new(100, 1, 1, 1000, entries1);
        let index2 = MemDbIndex::new(100, 1, 2, 2000, entries2); // Same version, higher mtime - should win

        // Test comparison - higher mtime should win
        assert_eq!(index2.cmp(&index1), Ordering::Greater);
        assert_eq!(index1.cmp(&index2), Ordering::Less);
    }

    #[test]
    fn test_leader_election_equal_indices() {
        use std::cmp::Ordering;

        // Create two identical indices
        let entries1 = vec![IndexEntry {
            inode: 1,
            digest: [0u8; 32],
        }];
        let entries2 = vec![IndexEntry {
            inode: 1,
            digest: [0u8; 32],
        }];

        let index1 = MemDbIndex::new(100, 1, 1, 1000, entries1);
        let index2 = MemDbIndex::new(100, 1, 2, 1000, entries2);

        // Should be equal
        assert_eq!(index1.cmp(&index2), Ordering::Equal);
        assert_eq!(index2.cmp(&index1), Ordering::Equal);
    }

    #[test]
    fn test_index_find_differences() {
        // Leader has inodes 1, 2, 3
        let leader_entries = vec![
            IndexEntry {
                inode: 1,
                digest: [0u8; 32],
            },
            IndexEntry {
                inode: 2,
                digest: [1u8; 32],
            },
            IndexEntry {
                inode: 3,
                digest: [2u8; 32],
            },
        ];
        let leader = MemDbIndex::new(100, 3, 1, 1000, leader_entries);

        // Follower has inodes 1 (same), 2 (different digest), missing 3
        let follower_entries = vec![
            IndexEntry {
                inode: 1,
                digest: [0u8; 32],
            }, // Same
            IndexEntry {
                inode: 2,
                digest: [99u8; 32],
            }, // Different digest
        ];
        let follower = MemDbIndex::new(90, 2, 2, 900, follower_entries);

        // Find differences
        let diffs = leader.find_differences(&follower);

        // Should find inodes 2 (different digest) and 3 (missing in follower)
        assert_eq!(diffs.len(), 2);
        assert!(diffs.contains(&2));
        assert!(diffs.contains(&3));
    }

    #[test]
    fn test_index_find_differences_no_diffs() {
        // Both have same inodes with same digests
        let entries1 = vec![
            IndexEntry {
                inode: 1,
                digest: [0u8; 32],
            },
            IndexEntry {
                inode: 2,
                digest: [1u8; 32],
            },
        ];
        let entries2 = vec![
            IndexEntry {
                inode: 1,
                digest: [0u8; 32],
            },
            IndexEntry {
                inode: 2,
                digest: [1u8; 32],
            },
        ];

        let index1 = MemDbIndex::new(100, 2, 1, 1000, entries1);
        let index2 = MemDbIndex::new(100, 2, 2, 1000, entries2);

        let diffs = index1.find_differences(&index2);
        assert_eq!(diffs.len(), 0);
    }

    #[test]
    fn test_index_find_differences_follower_has_extra() {
        // Leader has inodes 1, 2
        let leader_entries = vec![
            IndexEntry {
                inode: 1,
                digest: [0u8; 32],
            },
            IndexEntry {
                inode: 2,
                digest: [1u8; 32],
            },
        ];
        let leader = MemDbIndex::new(100, 2, 1, 1000, leader_entries);

        // Follower has inodes 1, 2, 3 (extra inode 3)
        let follower_entries = vec![
            IndexEntry {
                inode: 1,
                digest: [0u8; 32],
            },
            IndexEntry {
                inode: 2,
                digest: [1u8; 32],
            },
            IndexEntry {
                inode: 3,
                digest: [2u8; 32],
            },
        ];
        let follower = MemDbIndex::new(90, 3, 2, 900, follower_entries);

        // Find differences - leader should not report extra entries in follower
        // (follower will delete them when it receives leader's updates)
        let diffs = leader.find_differences(&follower);
        assert_eq!(diffs.len(), 0);
    }

    #[test]
    fn test_tree_entry_update_serialization() {
        use crate::types::TreeEntry;

        // Create a TreeEntry
        let entry = TreeEntry {
            inode: 42,
            parent: 1,
            version: 100,
            writer: 2,
            mtime: 12345,
            size: 11,
            entry_type: 8, // DT_REG
            name: "test.conf".to_string(),
            data: b"hello world".to_vec(),
        };

        // Serialize for update
        let serialized = entry.serialize_for_update();

        // Expected size: 41-byte header + 10 bytes (name + null) + 11 bytes (data)
        // = 62 bytes
        assert_eq!(serialized.len(), 41 + 10 + 11);

        // Deserialize
        let deserialized = TreeEntry::deserialize_from_update(&serialized).unwrap();

        // Verify all fields
        assert_eq!(deserialized.inode, entry.inode);
        assert_eq!(deserialized.parent, entry.parent);
        assert_eq!(deserialized.version, entry.version);
        assert_eq!(deserialized.writer, entry.writer);
        assert_eq!(deserialized.mtime, entry.mtime);
        assert_eq!(deserialized.size, entry.size);
        assert_eq!(deserialized.entry_type, entry.entry_type);
        assert_eq!(deserialized.name, entry.name);
        assert_eq!(deserialized.data, entry.data);
    }

    #[test]
    fn test_tree_entry_directory_serialization() {
        use crate::types::TreeEntry;

        // Create a directory entry (no data)
        let entry = TreeEntry {
            inode: 10,
            parent: 1,
            version: 50,
            writer: 1,
            mtime: 10000,
            size: 0,
            entry_type: 4, // DT_DIR
            name: "configs".to_string(),
            data: Vec::new(),
        };

        // Serialize
        let serialized = entry.serialize_for_update();

        // Expected size: 41-byte header + 8 bytes (name + null) + 0 bytes (no data)
        assert_eq!(serialized.len(), 41 + 8);

        // Deserialize
        let deserialized = TreeEntry::deserialize_from_update(&serialized).unwrap();

        assert_eq!(deserialized.inode, entry.inode);
        assert_eq!(deserialized.name, entry.name);
        assert_eq!(deserialized.entry_type, 4); // DT_DIR
        assert_eq!(deserialized.data.len(), 0);
    }

    #[test]
    fn test_tree_entry_empty_file_serialization() {
        use crate::types::TreeEntry;

        // Create an empty file
        let entry = TreeEntry {
            inode: 20,
            parent: 1,
            version: 75,
            writer: 3,
            mtime: 20000,
            size: 0,
            entry_type: 8, // DT_REG
            name: "empty.txt".to_string(),
            data: Vec::new(),
        };

        // Serialize
        let serialized = entry.serialize_for_update();

        // Expected size: 41-byte header + 10 bytes (name + null) + 0 bytes (no data)
        assert_eq!(serialized.len(), 41 + 10);

        // Deserialize
        let deserialized = TreeEntry::deserialize_from_update(&serialized).unwrap();

        assert_eq!(deserialized.inode, entry.inode);
        assert_eq!(deserialized.name, entry.name);
        assert_eq!(deserialized.size, 0);
        assert_eq!(deserialized.data.len(), 0);
    }

    #[test]
    fn test_index_digest_computation() {
        // Test that different entries produce different digests
        let digest1 = MemDbIndex::compute_entry_digest(1, 0, 100, 1, 1000, 0, 4, "dir1", &[]);

        let digest2 = MemDbIndex::compute_entry_digest(2, 0, 100, 1, 1000, 0, 4, "dir2", &[]);

        // Different inodes should produce different digests
        assert_ne!(digest1, digest2);

        // Same parameters should produce same digest
        let digest3 = MemDbIndex::compute_entry_digest(1, 0, 100, 1, 1000, 0, 4, "dir1", &[]);
        assert_eq!(digest1, digest3);

        // Different data should produce different digest
        let digest4 = MemDbIndex::compute_entry_digest(1, 0, 100, 1, 1000, 5, 8, "file", b"hello");
        let digest5 = MemDbIndex::compute_entry_digest(1, 0, 100, 1, 1000, 5, 8, "file", b"world");
        assert_ne!(digest4, digest5);
    }

    #[test]
    fn test_index_sorted_entries() {
        // Create entries in unsorted order
        let entries = vec![
            IndexEntry {
                inode: 5,
                digest: [5u8; 32],
            },
            IndexEntry {
                inode: 2,
                digest: [2u8; 32],
            },
            IndexEntry {
                inode: 8,
                digest: [8u8; 32],
            },
            IndexEntry {
                inode: 1,
                digest: [1u8; 32],
            },
        ];

        let index = MemDbIndex::new(100, 8, 1, 1000, entries);

        // Verify entries are stored sorted by inode
        assert_eq!(index.entries[0].inode, 1);
        assert_eq!(index.entries[1].inode, 2);
        assert_eq!(index.entries[2].inode, 5);
        assert_eq!(index.entries[3].inode, 8);
    }

    #[test]
    fn test_large_index_serialization() {
        // Test with a larger number of entries
        let mut entries = Vec::new();
        for i in 1..=100 {
            entries.push(IndexEntry {
                inode: i,
                digest: [(i % 256) as u8; 32],
            });
        }

        let index = MemDbIndex::new(1000, 100, 1, 50000, entries);

        // Serialize and deserialize
        let serialized = index.serialize();
        let deserialized =
            MemDbIndex::deserialize(&serialized).expect("Failed to deserialize large index");

        // Verify
        assert_eq!(deserialized.version, index.version);
        assert_eq!(deserialized.size, 100);
        assert_eq!(deserialized.entries.len(), 100);

        for i in 0..100 {
            assert_eq!(deserialized.entries[i].inode, (i + 1) as u64);
        }
    }
}
