/// Multi-node integration tests for DFSM cluster synchronization
///
/// These tests simulate multi-node clusters to verify the complete synchronization
/// protocol works correctly with multiple Rust nodes exchanging state.
use anyhow::Result;
use pmxcfs_dfsm::{Callbacks, FuseMessage, NodeSyncInfo};
use pmxcfs_memdb::{MemDb, MemDbIndex, ROOT_INODE, TreeEntry};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

/// Mock callbacks for testing DFSM without full pmxcfs integration
struct MockCallbacks {
    memdb: MemDb,
    states_received: Arc<Mutex<Vec<NodeSyncInfo>>>,
    updates_received: Arc<Mutex<Vec<TreeEntry>>>,
    synced_count: Arc<Mutex<usize>>,
}

impl MockCallbacks {
    fn new(memdb: MemDb) -> Self {
        Self {
            memdb,
            states_received: Arc::new(Mutex::new(Vec::new())),
            updates_received: Arc::new(Mutex::new(Vec::new())),
            synced_count: Arc::new(Mutex::new(0)),
        }
    }

    #[allow(dead_code)]
    fn get_states(&self) -> Vec<NodeSyncInfo> {
        self.states_received.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    fn get_updates(&self) -> Vec<TreeEntry> {
        self.updates_received.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    fn get_synced_count(&self) -> usize {
        *self.synced_count.lock().unwrap()
    }
}

impl Callbacks for MockCallbacks {
    type Message = FuseMessage;

    fn deliver_message(
        &self,
        _node_id: u32,
        _pid: u32,
        _message: FuseMessage,
        _timestamp: u64,
    ) -> Result<(i32, bool)> {
        Ok((0, true))
    }

    fn compute_checksum(&self, output: &mut [u8; 32]) -> Result<()> {
        let checksum = self.memdb.compute_database_checksum()?;
        output.copy_from_slice(&checksum);
        Ok(())
    }

    fn get_state(&self) -> Result<Vec<u8>> {
        let index = self.memdb.encode_index()?;
        Ok(index.serialize())
    }

    fn process_state_update(&self, states: &[NodeSyncInfo]) -> Result<bool> {
        // Store received states for verification
        *self.states_received.lock().unwrap() = states.to_vec();

        // Parse indices from states
        let mut indices: Vec<(u32, u32, MemDbIndex)> = Vec::new();
        for node in states {
            if let Some(state_data) = &node.state {
                match MemDbIndex::deserialize(state_data) {
                    Ok(index) => indices.push((node.node_id, node.pid, index)),
                    Err(_) => continue,
                }
            }
        }

        if indices.is_empty() {
            return Ok(true);
        }

        // Find leader (highest version, or if tie, highest mtime)
        let mut leader_idx = 0;
        for i in 1..indices.len() {
            let (_, _, current_index) = &indices[i];
            let (_, _, leader_index) = &indices[leader_idx];
            if current_index > leader_index {
                leader_idx = i;
            }
        }

        let (_leader_nodeid, _leader_pid, leader_index) = &indices[leader_idx];

        // Check if WE are synced with leader
        let our_index = self.memdb.encode_index()?;
        let we_are_synced = our_index.version == leader_index.version
            && our_index.mtime == leader_index.mtime
            && our_index.size == leader_index.size
            && our_index.entries.len() == leader_index.entries.len()
            && our_index
                .entries
                .iter()
                .zip(leader_index.entries.iter())
                .all(|(a, b)| a.inode == b.inode && a.digest == b.digest);

        Ok(we_are_synced)
    }

    fn process_update(&self, _node_id: u32, _pid: u32, data: &[u8]) -> Result<()> {
        // Deserialize and store update
        let tree_entry = TreeEntry::deserialize_from_update(data)?;
        self.updates_received
            .lock()
            .unwrap()
            .push(tree_entry.clone());

        // Apply to database
        self.memdb.apply_tree_entry(tree_entry)?;
        Ok(())
    }

    fn commit_state(&self) -> Result<()> {
        Ok(())
    }

    fn on_synced(&self) {
        *self.synced_count.lock().unwrap() += 1;
    }
}

fn create_test_node(node_id: u32) -> Result<(MemDb, TempDir, Arc<MockCallbacks>)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join(format!("node{node_id}.db"));
    let memdb = MemDb::open(&db_path, true)?;
    // Note: Local operations always use writer=0 (matching C implementation)
    // Remote DFSM updates use the writer field from the incoming TreeEntry

    let callbacks = Arc::new(MockCallbacks::new(memdb.clone()));
    Ok((memdb, temp_dir, callbacks))
}

#[test]
fn test_two_node_empty_sync() -> Result<()> {
    // Create two nodes with empty databases
    let (_memdb1, _temp1, callbacks1) = create_test_node(1)?;
    let (_memdb2, _temp2, callbacks2) = create_test_node(2)?;

    // Generate states from both nodes
    let state1 = callbacks1.get_state()?;
    let state2 = callbacks2.get_state()?;

    // Simulate state exchange
    let states = vec![
        NodeSyncInfo {
            node_id: 1,
            pid: 1000,
            state: Some(state1),
            synced: false,
        },
        NodeSyncInfo {
            node_id: 2,
            pid: 2000,
            state: Some(state2),
            synced: false,
        },
    ];

    // Both nodes process states
    let synced1 = callbacks1.process_state_update(&states)?;
    let synced2 = callbacks2.process_state_update(&states)?;

    // Both should be synced (empty databases are identical)
    assert!(synced1, "Node 1 should be synced");
    assert!(synced2, "Node 2 should be synced");

    Ok(())
}

#[test]
fn test_two_node_leader_election() -> Result<()> {
    // Create two nodes
    let (memdb1, _temp1, callbacks1) = create_test_node(1)?;
    let (_memdb2, _temp2, callbacks2) = create_test_node(2)?;

    // Node 1 has more data (higher version)
    memdb1.create("/file1.txt", 0, 0, 1000)?;
    memdb1.write("/file1.txt", 0, 0, 1001, b"data from node 1", false)?;

    // Generate states
    let state1 = callbacks1.get_state()?;
    let state2 = callbacks2.get_state()?;

    // Parse to check versions
    let index1 = MemDbIndex::deserialize(&state1)?;
    let index2 = MemDbIndex::deserialize(&state2)?;

    // Node 1 should have higher version
    assert!(
        index1.version > index2.version,
        "Node 1 version {} should be > Node 2 version {}",
        index1.version,
        index2.version
    );

    // Simulate state exchange
    let states = vec![
        NodeSyncInfo {
            node_id: 1,
            pid: 1000,
            state: Some(state1),
            synced: false,
        },
        NodeSyncInfo {
            node_id: 2,
            pid: 2000,
            state: Some(state2),
            synced: false,
        },
    ];

    // Process states
    let synced1 = callbacks1.process_state_update(&states)?;
    let synced2 = callbacks2.process_state_update(&states)?;

    // Node 1 (leader) should be synced, Node 2 (follower) should not
    assert!(synced1, "Node 1 (leader) should be synced");
    assert!(!synced2, "Node 2 (follower) should not be synced");

    Ok(())
}

#[test]
fn test_incremental_update_transfer() -> Result<()> {
    // Create leader and follower
    let (leader_db, _temp_leader, _) = create_test_node(1)?;
    let (follower_db, _temp_follower, follower_callbacks) = create_test_node(2)?;

    // Leader has data
    leader_db.create("/config", libc::S_IFDIR, 0, 1000)?;
    leader_db.create("/config/node.conf", 0, 0, 1001)?;
    leader_db.write("/config/node.conf", 0, 0, 1002, b"hostname=pve1", false)?;

    // Get entries from leader
    let leader_entries = leader_db.get_all_entries()?;

    // Simulate sending updates to follower
    for entry in leader_entries {
        if entry.inode == ROOT_INODE {
            continue; // Skip root (both have it)
        }

        // Serialize as update message
        let update_msg = entry.serialize_for_update();

        // Follower receives and processes update
        follower_callbacks.process_update(1, 1000, &update_msg)?;
    }

    // Verify follower has the data
    let config_dir = follower_db.lookup_path("/config");
    assert!(
        config_dir.is_some(),
        "Follower should have /config directory"
    );
    assert!(config_dir.unwrap().is_dir());

    let config_file = follower_db.lookup_path("/config/node.conf");
    assert!(
        config_file.is_some(),
        "Follower should have /config/node.conf"
    );

    let config_data = follower_db.read("/config/node.conf", 0, 1024)?;
    assert_eq!(
        config_data, b"hostname=pve1",
        "Follower should have correct data"
    );

    Ok(())
}

#[test]
fn test_three_node_sync() -> Result<()> {
    // Create three nodes
    let (memdb1, _temp1, callbacks1) = create_test_node(1)?;
    let (memdb2, _temp2, callbacks2) = create_test_node(2)?;
    let (_memdb3, _temp3, callbacks3) = create_test_node(3)?;

    // Node 1 has the most recent data
    memdb1.create("/cluster.conf", 0, 0, 5000)?;
    memdb1.write("/cluster.conf", 0, 0, 5001, b"version=3", false)?;

    // Node 2 has older data
    memdb2.create("/cluster.conf", 0, 0, 4000)?;
    memdb2.write("/cluster.conf", 0, 0, 4001, b"version=2", false)?;

    // Node 3 is empty (new node joining)

    // Generate states
    let state1 = callbacks1.get_state()?;
    let state2 = callbacks2.get_state()?;
    let state3 = callbacks3.get_state()?;

    let states = vec![
        NodeSyncInfo {
            node_id: 1,
            pid: 1000,
            state: Some(state1.clone()),
            synced: false,
        },
        NodeSyncInfo {
            node_id: 2,
            pid: 2000,
            state: Some(state2.clone()),
            synced: false,
        },
        NodeSyncInfo {
            node_id: 3,
            pid: 3000,
            state: Some(state3.clone()),
            synced: false,
        },
    ];

    // All nodes process states
    let synced1 = callbacks1.process_state_update(&states)?;
    let synced2 = callbacks2.process_state_update(&states)?;
    let synced3 = callbacks3.process_state_update(&states)?;

    // Node 1 (leader) should be synced
    assert!(synced1, "Node 1 (leader) should be synced");

    // Nodes 2 and 3 need updates
    assert!(!synced2, "Node 2 should need updates");
    assert!(!synced3, "Node 3 should need updates");

    // Verify leader has highest version
    let index1 = MemDbIndex::deserialize(&state1)?;
    let index2 = MemDbIndex::deserialize(&state2)?;
    let index3 = MemDbIndex::deserialize(&state3)?;

    assert!(index1.version >= index2.version);
    assert!(index1.version >= index3.version);

    Ok(())
}

#[test]
fn test_update_message_wire_format_compatibility() -> Result<()> {
    // Verify our wire format matches C implementation exactly
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

    let serialized = entry.serialize_for_update();

    // Verify header size (41 bytes)
    // parent(8) + inode(8) + version(8) + writer(4) + mtime(4) + size(4) + namelen(4) + type(1)
    let expected_header_size = 8 + 8 + 8 + 4 + 4 + 4 + 4 + 1;
    assert_eq!(expected_header_size, 41);

    // Verify total size
    let namelen = "test.conf".len() + 1; // Include null terminator
    let expected_total = expected_header_size + namelen + 11;
    assert_eq!(serialized.len(), expected_total);

    // Verify we can deserialize it back
    let deserialized = TreeEntry::deserialize_from_update(&serialized)?;
    assert_eq!(deserialized.inode, entry.inode);
    assert_eq!(deserialized.parent, entry.parent);
    assert_eq!(deserialized.version, entry.version);
    assert_eq!(deserialized.writer, entry.writer);
    assert_eq!(deserialized.mtime, entry.mtime);
    assert_eq!(deserialized.size, entry.size);
    assert_eq!(deserialized.entry_type, entry.entry_type);
    assert_eq!(deserialized.name, entry.name);
    assert_eq!(deserialized.data, entry.data);

    Ok(())
}

#[test]
fn test_index_wire_format_compatibility() -> Result<()> {
    // Verify memdb_index_t wire format matches C implementation
    use pmxcfs_memdb::IndexEntry;

    let entries = vec![
        IndexEntry {
            inode: 1,
            digest: [0u8; 32],
        },
        IndexEntry {
            inode: 2,
            digest: [1u8; 32],
        },
    ];

    let index = MemDbIndex::new(
        100,   // version
        2,     // last_inode
        1,     // writer
        12345, // mtime
        entries,
    );

    let serialized = index.serialize();

    // Verify header size (32 bytes)
    // version(8) + last_inode(8) + writer(4) + mtime(4) + size(4) + bytes(4)
    let expected_header_size = 8 + 8 + 4 + 4 + 4 + 4;
    assert_eq!(expected_header_size, 32);

    // Verify entry size (40 bytes each)
    // inode(8) + digest(32)
    let expected_entry_size = 8 + 32;
    assert_eq!(expected_entry_size, 40);

    // Verify total size
    let expected_total = expected_header_size + 2 * expected_entry_size;
    assert_eq!(serialized.len(), expected_total);
    assert_eq!(serialized.len(), index.bytes as usize);

    // Verify deserialization
    let deserialized = MemDbIndex::deserialize(&serialized)?;
    assert_eq!(deserialized.version, index.version);
    assert_eq!(deserialized.last_inode, index.last_inode);
    assert_eq!(deserialized.writer, index.writer);
    assert_eq!(deserialized.mtime, index.mtime);
    assert_eq!(deserialized.size, index.size);
    assert_eq!(deserialized.bytes, index.bytes);
    assert_eq!(deserialized.entries.len(), 2);

    Ok(())
}

#[test]
fn test_sync_with_conflicts() -> Result<()> {
    // Test scenario: two nodes modified different files
    let (memdb1, _temp1, _callbacks1) = create_test_node(1)?;
    let (memdb2, _temp2, _callbacks2) = create_test_node(2)?;

    // Both start with same base
    memdb1.create("/base.conf", 0, 0, 1000)?;
    memdb1.write("/base.conf", 0, 0, 1001, b"shared", false)?;

    memdb2.create("/base.conf", 0, 0, 1000)?;
    memdb2.write("/base.conf", 0, 0, 1001, b"shared", false)?;

    // Node 1 adds file1
    memdb1.create("/file1.txt", 0, 0, 2000)?;
    memdb1.write("/file1.txt", 0, 0, 2001, b"from node 1", false)?;

    // Node 2 adds file2
    memdb2.create("/file2.txt", 0, 0, 2000)?;
    memdb2.write("/file2.txt", 0, 0, 2001, b"from node 2", false)?;

    // Generate indices
    let index1 = memdb1.encode_index()?;
    let index2 = memdb2.encode_index()?;

    // Find differences
    let diffs_1_vs_2 = index1.find_differences(&index2);
    let diffs_2_vs_1 = index2.find_differences(&index1);

    // Node 1 has file1 that node 2 doesn't have
    assert!(
        !diffs_1_vs_2.is_empty(),
        "Node 1 should have entries node 2 doesn't have"
    );

    // Node 2 has file2 that node 1 doesn't have
    assert!(
        !diffs_2_vs_1.is_empty(),
        "Node 2 should have entries node 1 doesn't have"
    );

    // Higher version wins - in this case they're both v3 (base + create + write)
    // so mtime would be tiebreaker

    Ok(())
}

#[test]
fn test_large_file_update() -> Result<()> {
    // Test updating a file with significant data
    let (leader_db, _temp_leader, _) = create_test_node(1)?;
    let (follower_db, _temp_follower, follower_callbacks) = create_test_node(2)?;

    // Create a file with 10KB of data
    let large_data: Vec<u8> = (0..10240).map(|i| (i % 256) as u8).collect();

    leader_db.create("/large.bin", 0, 0, 1000)?;
    leader_db.write("/large.bin", 0, 0, 1001, &large_data, false)?;

    // Get the entry
    let entry = leader_db.lookup_path("/large.bin").unwrap();

    // Serialize and send
    let update_msg = entry.serialize_for_update();

    // Follower receives
    follower_callbacks.process_update(1, 1000, &update_msg)?;

    // Verify
    let follower_entry = follower_db.lookup_path("/large.bin").unwrap();
    assert_eq!(follower_entry.size, large_data.len());
    assert_eq!(follower_entry.data, large_data);

    Ok(())
}

#[test]
fn test_directory_hierarchy_sync() -> Result<()> {
    // Test syncing nested directory structure
    let (leader_db, _temp_leader, _) = create_test_node(1)?;
    let (follower_db, _temp_follower, follower_callbacks) = create_test_node(2)?;

    // Create directory hierarchy on leader
    leader_db.create("/etc", libc::S_IFDIR, 0, 1000)?;
    leader_db.create("/etc/pve", libc::S_IFDIR, 0, 1001)?;
    leader_db.create("/etc/pve/nodes", libc::S_IFDIR, 0, 1002)?;
    leader_db.create("/etc/pve/nodes/pve1", libc::S_IFDIR, 0, 1003)?;
    leader_db.create("/etc/pve/nodes/pve1/config", 0, 0, 1004)?;
    leader_db.write(
        "/etc/pve/nodes/pve1/config", 0, 0, 1005, b"cpu: 2\nmem: 4096", false,
    )?;

    // Send all entries to follower
    let entries = leader_db.get_all_entries()?;
    for entry in entries {
        if entry.inode == ROOT_INODE {
            continue; // Skip root
        }
        let update_msg = entry.serialize_for_update();
        follower_callbacks.process_update(1, 1000, &update_msg)?;
    }

    // Verify entire hierarchy
    assert!(follower_db.lookup_path("/etc").is_some());
    assert!(follower_db.lookup_path("/etc/pve").is_some());
    assert!(follower_db.lookup_path("/etc/pve/nodes").is_some());
    assert!(follower_db.lookup_path("/etc/pve/nodes/pve1").is_some());

    let config = follower_db.lookup_path("/etc/pve/nodes/pve1/config");
    assert!(config.is_some());
    assert_eq!(config.unwrap().data, b"cpu: 2\nmem: 4096");

    Ok(())
}
