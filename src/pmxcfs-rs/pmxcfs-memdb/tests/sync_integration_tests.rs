/// Integration tests for MemDb synchronization operations
///
/// Tests the apply_tree_entry and encode_index functionality used during
/// cluster state synchronization.
use anyhow::Result;
use pmxcfs_memdb::{MemDb, ROOT_INODE, TreeEntry};
use tempfile::TempDir;

fn create_test_db() -> Result<(MemDb, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let memdb = MemDb::open(&db_path, true)?;
    Ok((memdb, temp_dir))
}

#[test]
fn test_encode_index_empty_db() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Encode index from empty database (only root entry)
    let index = memdb.encode_index()?;

    // Should have version and one entry (root)
    assert_eq!(index.version, 1); // Root created with version 1
    assert_eq!(index.size, 1);
    assert_eq!(index.entries.len(), 1);
    // Root is converted to inode 0 for C wire format compatibility
    assert_eq!(index.entries[0].inode, 0); // Root in C format (was 1 in Rust)

    Ok(())
}

#[test]
fn test_encode_index_with_entries() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Create some entries
    memdb.create("/file1.txt", 0, 0, 1000)?;
    memdb.create("/dir1", libc::S_IFDIR, 0, 1001)?;
    memdb.create("/dir1/file2.txt", 0, 0, 1002)?;

    // Encode index
    let index = memdb.encode_index()?;

    // Should have 4 entries: root, file1.txt, dir1, dir1/file2.txt
    assert_eq!(index.size, 4);
    assert_eq!(index.entries.len(), 4);

    // Entries should be sorted by inode
    for i in 1..index.entries.len() {
        assert!(
            index.entries[i].inode > index.entries[i - 1].inode,
            "Entries not sorted"
        );
    }

    // Version should be incremented
    assert!(index.version >= 4); // At least 4 operations

    Ok(())
}

#[test]
fn test_apply_tree_entry_new() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Create a new TreeEntry
    let entry = TreeEntry {
        inode: 10,
        parent: ROOT_INODE,
        version: 100,
        writer: 2,
        mtime: 5000,
        size: 13,
        entry_type: 8, // DT_REG
        name: "applied.txt".to_string(),
        data: b"applied data!".to_vec(),
    };

    // Apply it
    memdb.apply_tree_entry(entry.clone())?;

    // Verify it was added
    let retrieved = memdb.lookup_path("/applied.txt");
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();

    assert_eq!(retrieved.inode, 10);
    assert_eq!(retrieved.name, "applied.txt");
    assert_eq!(retrieved.version, 100);
    assert_eq!(retrieved.writer, 2);
    assert_eq!(retrieved.mtime, 5000);
    assert_eq!(retrieved.data, b"applied data!");

    // Verify database version was updated
    assert!(memdb.get_version() >= 100);

    Ok(())
}

#[test]
fn test_apply_tree_entry_update() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Create an initial entry
    memdb.create("/update.txt", 0, 0, 1000)?;
    memdb.write("/update.txt", 0, 0, 1001, b"original", false)?;

    let initial = memdb.lookup_path("/update.txt").unwrap();
    let initial_inode = initial.inode;

    // Apply an updated version
    let updated = TreeEntry {
        inode: initial_inode,
        parent: ROOT_INODE,
        version: 200,
        writer: 3,
        mtime: 2000,
        size: 7,
        entry_type: 8,
        name: "update.txt".to_string(),
        data: b"updated".to_vec(),
    };

    memdb.apply_tree_entry(updated)?;

    // Verify it was updated
    let retrieved = memdb.lookup_path("/update.txt").unwrap();
    assert_eq!(retrieved.inode, initial_inode); // Same inode
    assert_eq!(retrieved.version, 200); // Updated version
    assert_eq!(retrieved.writer, 3); // Updated writer
    assert_eq!(retrieved.mtime, 2000); // Updated mtime
    assert_eq!(retrieved.data, b"updated"); // Updated data

    Ok(())
}

#[test]
fn test_apply_tree_entry_directory() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Apply a directory entry
    let dir_entry = TreeEntry {
        inode: 20,
        parent: ROOT_INODE,
        version: 50,
        writer: 1,
        mtime: 3000,
        size: 0,
        entry_type: 4, // DT_DIR
        name: "newdir".to_string(),
        data: Vec::new(),
    };

    memdb.apply_tree_entry(dir_entry)?;

    // Verify directory was created
    let retrieved = memdb.lookup_path("/newdir").unwrap();
    assert_eq!(retrieved.inode, 20);
    assert!(retrieved.is_dir());
    assert_eq!(retrieved.name, "newdir");

    Ok(())
}

#[test]
fn test_apply_tree_entry_move() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Create initial structure
    memdb.create("/olddir", libc::S_IFDIR, 0, 1000)?;
    memdb.create("/newdir", libc::S_IFDIR, 0, 1001)?;
    memdb.create("/olddir/file.txt", 0, 0, 1002)?;

    let file = memdb.lookup_path("/olddir/file.txt").unwrap();
    let file_inode = file.inode;
    let newdir = memdb.lookup_path("/newdir").unwrap();

    // Apply entry that moves file to newdir
    let moved = TreeEntry {
        inode: file_inode,
        parent: newdir.inode, // New parent
        version: 100,
        writer: 2,
        mtime: 2000,
        size: 0,
        entry_type: 8,
        name: "file.txt".to_string(),
        data: Vec::new(),
    };

    memdb.apply_tree_entry(moved)?;

    // Verify file moved
    assert!(memdb.lookup_path("/olddir/file.txt").is_none());
    assert!(memdb.lookup_path("/newdir/file.txt").is_some());
    let retrieved = memdb.lookup_path("/newdir/file.txt").unwrap();
    assert_eq!(retrieved.inode, file_inode);

    Ok(())
}

#[test]
fn test_apply_multiple_entries() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Apply multiple entries simulating a sync
    let entries = vec![
        TreeEntry {
            inode: 10,
            parent: ROOT_INODE,
            version: 100,
            writer: 2,
            mtime: 5000,
            size: 0,
            entry_type: 4, // Dir
            name: "configs".to_string(),
            data: Vec::new(),
        },
        TreeEntry {
            inode: 11,
            parent: 10,
            version: 101,
            writer: 2,
            mtime: 5001,
            size: 12,
            entry_type: 8, // File
            name: "config1.txt".to_string(),
            data: b"config data1".to_vec(),
        },
        TreeEntry {
            inode: 12,
            parent: 10,
            version: 102,
            writer: 2,
            mtime: 5002,
            size: 12,
            entry_type: 8,
            name: "config2.txt".to_string(),
            data: b"config data2".to_vec(),
        },
    ];

    // Apply all entries
    for entry in entries {
        memdb.apply_tree_entry(entry)?;
    }

    // Verify all were applied correctly
    assert!(memdb.lookup_path("/configs").is_some());
    assert!(memdb.lookup_path("/configs/config1.txt").is_some());
    assert!(memdb.lookup_path("/configs/config2.txt").is_some());

    let config1 = memdb.lookup_path("/configs/config1.txt").unwrap();
    assert_eq!(config1.data, b"config data1");

    let config2 = memdb.lookup_path("/configs/config2.txt").unwrap();
    assert_eq!(config2.data, b"config data2");

    // Verify database version
    assert_eq!(memdb.get_version(), 102);

    Ok(())
}

#[test]
fn test_encode_decode_round_trip() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Create some entries
    memdb.create("/file1.txt", 0, 0, 1000)?;
    memdb.write("/file1.txt", 0, 0, 1001, b"data1", false)?;
    memdb.create("/dir1", libc::S_IFDIR, 0, 1002)?;
    memdb.create("/dir1/file2.txt", 0, 0, 1003)?;
    memdb.write("/dir1/file2.txt", 0, 0, 1004, b"data2", false)?;

    // Encode index
    let index = memdb.encode_index()?;
    let serialized = index.serialize();

    // Deserialize
    let deserialized = pmxcfs_memdb::MemDbIndex::deserialize(&serialized)?;

    // Verify roundtrip
    assert_eq!(deserialized.version, index.version);
    assert_eq!(deserialized.last_inode, index.last_inode);
    assert_eq!(deserialized.writer, index.writer);
    assert_eq!(deserialized.mtime, index.mtime);
    assert_eq!(deserialized.size, index.size);
    assert_eq!(deserialized.entries.len(), index.entries.len());

    for (orig, deser) in index.entries.iter().zip(deserialized.entries.iter()) {
        assert_eq!(deser.inode, orig.inode);
        assert_eq!(deser.digest, orig.digest);
    }

    Ok(())
}

#[test]
fn test_apply_tree_entry_persistence() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("persist.db");

    // Create database and apply entry
    {
        let memdb = MemDb::open(&db_path, true)?;
        let entry = TreeEntry {
            inode: 15,
            parent: ROOT_INODE,
            version: 75,
            writer: 3,
            mtime: 7000,
            size: 9,
            entry_type: 8,
            name: "persist.txt".to_string(),
            data: b"persisted".to_vec(),
        };
        memdb.apply_tree_entry(entry)?;
    }

    // Reopen database and verify entry persisted
    {
        let memdb = MemDb::open(&db_path, false)?;
        let retrieved = memdb.lookup_path("/persist.txt");
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.inode, 15);
        assert_eq!(retrieved.version, 75);
        assert_eq!(retrieved.data, b"persisted");
    }

    Ok(())
}

#[test]
fn test_index_digest_stability() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Create entry
    memdb.create("/stable.txt", 0, 0, 1000)?;
    memdb.write("/stable.txt", 0, 0, 1001, b"stable data", false)?;

    // Encode index twice
    let index1 = memdb.encode_index()?;
    let index2 = memdb.encode_index()?;

    // Digests should be identical
    assert_eq!(index1.entries.len(), index2.entries.len());
    for (e1, e2) in index1.entries.iter().zip(index2.entries.iter()) {
        assert_eq!(e1.inode, e2.inode);
        assert_eq!(e1.digest, e2.digest, "Digests should be stable");
    }

    Ok(())
}

#[test]
fn test_index_digest_changes_on_modification() -> Result<()> {
    let (memdb, _temp_dir) = create_test_db()?;

    // Create entry
    memdb.create("/change.txt", 0, 0, 1000)?;
    memdb.write("/change.txt", 0, 0, 1001, b"original", false)?;

    // Get initial digest
    let index1 = memdb.encode_index()?;
    let original_digest = index1
        .entries
        .iter()
        .find(|e| e.inode != 1) // Not root
        .unwrap()
        .digest;

    // Modify the file
    memdb.write("/change.txt", 0, 0, 1002, b"modified", false)?;

    // Get new digest
    let index2 = memdb.encode_index()?;
    let modified_digest = index2
        .entries
        .iter()
        .find(|e| e.inode != 1) // Not root
        .unwrap()
        .digest;

    // Digest should change
    assert_ne!(
        original_digest, modified_digest,
        "Digest should change after modification"
    );

    Ok(())
}
