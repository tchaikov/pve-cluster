/// DFSM Protocol Message Types
///
/// This module defines the DfsmMessage enum which encapsulates all DFSM protocol messages
/// with their associated data, providing type-safe serialization and deserialization.
///
/// Wire format matches C implementation's dfsm_message_*_header_t structures for compatibility.
use anyhow::Result;
use pmxcfs_memdb::TreeEntry;

use super::message::Message;
use super::types::{DfsmMessageType, SyncEpoch};

/// DFSM protocol message with typed variants
///
/// Each variant corresponds to a message type in the DFSM protocol and carries
/// the appropriate payload data. The wire format matches the C implementation:
///
/// For Normal messages: dfsm_message_normal_header_t (24 bytes) + fuse_data
/// ```text
/// [type: u16][subtype: u16][protocol: u32][time: u32][reserved: u32][count: u64][fuse_data...]
/// ```
///
/// The generic parameter `M` specifies the application message type and must implement
/// the `Message` trait for serialization/deserialization:
/// - `DfsmMessage<FuseMessage>` for database operations
/// - `DfsmMessage<KvStoreMessage>` for status synchronization
#[derive(Debug, Clone)]
pub enum DfsmMessage<M: Message> {
    /// Regular application message
    ///
    /// Contains a typed application message (FuseMessage or KvStoreMessage).
    /// C wire format: dfsm_message_normal_header_t + application_message data
    Normal {
        msg_count: u64,
        timestamp: u32,        // Unix timestamp (matches C's u32)
        protocol_version: u32, // Protocol version
        message: M,            // Typed message (FuseMessage or KvStoreMessage)
    },

    /// Start synchronization signal from leader (no payload)
    /// C wire format: dfsm_message_state_header_t (32 bytes: 16 base + 16 epoch)
    SyncStart { sync_epoch: SyncEpoch },

    /// State data from another node during sync
    ///
    /// Wire format: dfsm_message_state_header_t (32 bytes) + [state_data: raw bytes]
    State {
        sync_epoch: SyncEpoch,
        data: Vec<u8>,
    },

    /// State update from leader
    ///
    /// C wire format: dfsm_message_state_header_t (32 bytes: 16 base + 16 epoch) + TreeEntry fields
    /// This is sent by the leader during synchronization to update followers
    /// with individual database entries that differ from their state.
    Update {
        sync_epoch: SyncEpoch,
        tree_entry: TreeEntry,
    },

    /// Update complete signal from leader (no payload)
    /// C wire format: dfsm_message_state_header_t (32 bytes: 16 base + 16 epoch)
    UpdateComplete { sync_epoch: SyncEpoch },

    /// Verification request from leader
    ///
    /// Wire format: dfsm_message_state_header_t (32 bytes) + [csum_id: u64]
    VerifyRequest { sync_epoch: SyncEpoch, csum_id: u64 },

    /// Verification response with checksum
    ///
    /// Wire format: dfsm_message_state_header_t (32 bytes) + [csum_id: u64][checksum: [u8; 32]]
    Verify {
        sync_epoch: SyncEpoch,
        csum_id: u64,
        checksum: [u8; 32],
    },
}

impl<M: Message> DfsmMessage<M> {
    /// Protocol version (should match cluster-wide)
    pub const DEFAULT_PROTOCOL_VERSION: u32 = 1;

    /// Get the message type discriminant
    pub fn message_type(&self) -> DfsmMessageType {
        match self {
            DfsmMessage::Normal { .. } => DfsmMessageType::Normal,
            DfsmMessage::SyncStart { .. } => DfsmMessageType::SyncStart,
            DfsmMessage::State { .. } => DfsmMessageType::State,
            DfsmMessage::Update { .. } => DfsmMessageType::Update,
            DfsmMessage::UpdateComplete { .. } => DfsmMessageType::UpdateComplete,
            DfsmMessage::VerifyRequest { .. } => DfsmMessageType::VerifyRequest,
            DfsmMessage::Verify { .. } => DfsmMessageType::Verify,
        }
    }

    /// Serialize message to C-compatible wire format
    ///
    /// For Normal/Update: dfsm_message_normal_header_t (24 bytes) + application_data
    /// Format: [type: u16][subtype: u16][protocol: u32][time: u32][reserved: u32][count: u64][data...]
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            DfsmMessage::Normal {
                msg_count,
                timestamp,
                protocol_version,
                message,
            } => self.serialize_normal_message(*msg_count, *timestamp, *protocol_version, message),
            _ => self.serialize_state_message(),
        }
    }

    /// Serialize a Normal message with C-compatible header
    fn serialize_normal_message(
        &self,
        msg_count: u64,
        timestamp: u32,
        protocol_version: u32,
        message: &M,
    ) -> Vec<u8> {
        let msg_type = self.message_type() as u16;
        let subtype = message.message_type();
        let app_data = message.serialize();

        // C header: type (u16) + subtype (u16) + protocol (u32) + time (u32) + reserved (u32) + count (u64) = 24 bytes
        let mut message = Vec::with_capacity(24 + app_data.len());

        // dfsm_message_header_t fields
        message.extend_from_slice(&msg_type.to_le_bytes());
        message.extend_from_slice(&subtype.to_le_bytes());
        message.extend_from_slice(&protocol_version.to_le_bytes());
        message.extend_from_slice(&timestamp.to_le_bytes());
        message.extend_from_slice(&0u32.to_le_bytes()); // reserved

        // count field
        message.extend_from_slice(&msg_count.to_le_bytes());

        // application message data
        message.extend_from_slice(&app_data);

        message
    }

    /// Serialize state messages (non-Normal) with C-compatible header
    /// C wire format: dfsm_message_state_header_t (32 bytes) + payload
    /// Header breakdown: base (16 bytes) + epoch (16 bytes)
    fn serialize_state_message(&self) -> Vec<u8> {
        let msg_type = self.message_type() as u16;
        let (sync_epoch, payload) = self.extract_epoch_and_payload();

        // For state messages: dfsm_message_state_header_t (32 bytes: 16 base + 16 epoch) + payload
        let mut message = Vec::with_capacity(32 + payload.len());

        // Base header (16 bytes): type, subtype, protocol, time, reserved
        message.extend_from_slice(&msg_type.to_le_bytes());
        message.extend_from_slice(&0u16.to_le_bytes()); // subtype (unused)
        message.extend_from_slice(&Self::DEFAULT_PROTOCOL_VERSION.to_le_bytes());

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;
        message.extend_from_slice(&timestamp.to_le_bytes());
        message.extend_from_slice(&0u32.to_le_bytes()); // reserved

        // Epoch header (16 bytes): epoch, time, nodeid, pid
        message.extend_from_slice(&sync_epoch.serialize());

        // Payload
        message.extend_from_slice(&payload);

        message
    }

    /// Extract sync_epoch and payload from state messages
    fn extract_epoch_and_payload(&self) -> (SyncEpoch, Vec<u8>) {
        match self {
            DfsmMessage::Normal { .. } => {
                unreachable!("Normal messages use serialize_normal_message")
            }
            DfsmMessage::SyncStart { sync_epoch } => (*sync_epoch, Vec::new()),
            DfsmMessage::State { sync_epoch, data } => (*sync_epoch, data.clone()),
            DfsmMessage::Update {
                sync_epoch,
                tree_entry,
            } => (*sync_epoch, tree_entry.serialize_for_update()),
            DfsmMessage::UpdateComplete { sync_epoch } => (*sync_epoch, Vec::new()),
            DfsmMessage::VerifyRequest {
                sync_epoch,
                csum_id,
            } => (*sync_epoch, csum_id.to_le_bytes().to_vec()),
            DfsmMessage::Verify {
                sync_epoch,
                csum_id,
                checksum,
            } => {
                let mut data = Vec::with_capacity(8 + 32);
                data.extend_from_slice(&csum_id.to_le_bytes());
                data.extend_from_slice(checksum);
                (*sync_epoch, data)
            }
        }
    }

    /// Deserialize message from C-compatible wire format
    ///
    /// Normal messages: [base header: 16 bytes][count: u64][app data]
    /// State messages:  [base header: 16 bytes][epoch: 16 bytes][payload]
    ///
    /// # Arguments
    /// * `data` - Raw message bytes from CPG
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        if data.len() < 16 {
            anyhow::bail!(
                "Message too short: {} bytes (need at least 16 for header)",
                data.len()
            );
        }

        // Parse dfsm_message_header_t (16 bytes)
        let msg_type = u16::from_le_bytes([data[0], data[1]]);
        let subtype = u16::from_le_bytes([data[2], data[3]]);
        let protocol_version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let timestamp = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        let _reserved = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);

        let dfsm_type = DfsmMessageType::try_from(msg_type)?;

        // Normal messages have different structure than state messages
        if dfsm_type == DfsmMessageType::Normal {
            // Normal: [base: 16][count: 8][app_data: ...]
            let payload = &data[16..];
            Self::deserialize_normal_message(subtype, protocol_version, timestamp, payload)
        } else {
            // State messages: [base: 16][epoch: 16][payload: ...]
            if data.len() < 32 {
                anyhow::bail!(
                    "State message too short: {} bytes (need at least 32 for state header)",
                    data.len()
                );
            }
            let sync_epoch = SyncEpoch::deserialize(&data[16..32])
                .map_err(|e| anyhow::anyhow!("Failed to deserialize sync epoch: {e}"))?;
            let payload = &data[32..];
            Self::deserialize_state_message(dfsm_type, sync_epoch, payload)
        }
    }

    /// Deserialize a Normal message
    fn deserialize_normal_message(
        subtype: u16,
        protocol_version: u32,
        timestamp: u32,
        payload: &[u8],
    ) -> Result<Self> {
        // Normal messages have count field (u64) after base header
        if payload.len() < 8 {
            anyhow::bail!("Normal message too short: need count field");
        }
        let msg_count = u64::from_le_bytes(payload[0..8].try_into().unwrap());
        let app_data = &payload[8..];

        // Deserialize using the Message trait
        let message = M::deserialize(subtype, app_data)?;

        Ok(DfsmMessage::Normal {
            msg_count,
            timestamp,
            protocol_version,
            message,
        })
    }

    /// Deserialize a state message (with epoch)
    fn deserialize_state_message(
        dfsm_type: DfsmMessageType,
        sync_epoch: SyncEpoch,
        payload: &[u8],
    ) -> Result<Self> {
        match dfsm_type {
            DfsmMessageType::Normal => {
                unreachable!("Normal messages use deserialize_normal_message")
            }
            DfsmMessageType::Update => {
                let tree_entry = TreeEntry::deserialize_from_update(payload)?;
                Ok(DfsmMessage::Update {
                    sync_epoch,
                    tree_entry,
                })
            }
            DfsmMessageType::SyncStart => Ok(DfsmMessage::SyncStart { sync_epoch }),
            DfsmMessageType::State => Ok(DfsmMessage::State {
                sync_epoch,
                data: payload.to_vec(),
            }),
            DfsmMessageType::UpdateComplete => Ok(DfsmMessage::UpdateComplete { sync_epoch }),
            DfsmMessageType::VerifyRequest => {
                if payload.len() < 8 {
                    anyhow::bail!("VerifyRequest message too short");
                }
                let csum_id = u64::from_le_bytes(payload[0..8].try_into().unwrap());
                Ok(DfsmMessage::VerifyRequest {
                    sync_epoch,
                    csum_id,
                })
            }
            DfsmMessageType::Verify => {
                if payload.len() < 40 {
                    anyhow::bail!("Verify message too short");
                }
                let csum_id = u64::from_le_bytes(payload[0..8].try_into().unwrap());
                let mut checksum = [0u8; 32];
                checksum.copy_from_slice(&payload[8..40]);
                Ok(DfsmMessage::Verify {
                    sync_epoch,
                    csum_id,
                    checksum,
                })
            }
        }
    }

    /// Helper to create a Normal message from an application message
    pub fn from_message(msg_count: u64, message: M, protocol_version: u32) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;

        DfsmMessage::Normal {
            msg_count,
            timestamp,
            protocol_version,
            message,
        }
    }

    /// Helper to create an Update message from a TreeEntry
    ///
    /// Used by the leader during synchronization to send individual database entries
    /// to nodes that need to catch up. Matches C's dcdb_send_update_inode().
    pub fn from_tree_entry(tree_entry: TreeEntry, sync_epoch: SyncEpoch) -> Self {
        DfsmMessage::Update {
            sync_epoch,
            tree_entry,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FuseMessage;

    #[test]
    fn test_sync_start_roundtrip() {
        let sync_epoch = SyncEpoch {
            epoch: 1,
            time: 1234567890,
            nodeid: 1,
            pid: 1000,
        };
        let msg: DfsmMessage<FuseMessage> = DfsmMessage::SyncStart { sync_epoch };
        let serialized = msg.serialize();
        let deserialized = DfsmMessage::<FuseMessage>::deserialize(&serialized).unwrap();

        assert!(
            matches!(deserialized, DfsmMessage::SyncStart { sync_epoch: e } if e == sync_epoch)
        );
    }

    #[test]
    fn test_normal_roundtrip() {
        let fuse_msg = FuseMessage::Create {
            path: "/test/file".to_string(),
        };

        let msg: DfsmMessage<FuseMessage> = DfsmMessage::Normal {
            msg_count: 42,
            timestamp: 1234567890,
            protocol_version: DfsmMessage::<FuseMessage>::DEFAULT_PROTOCOL_VERSION,
            message: fuse_msg.clone(),
        };

        let serialized = msg.serialize();
        let deserialized = DfsmMessage::<FuseMessage>::deserialize(&serialized).unwrap();

        match deserialized {
            DfsmMessage::Normal {
                msg_count,
                timestamp,
                protocol_version,
                message,
            } => {
                assert_eq!(msg_count, 42);
                assert_eq!(timestamp, 1234567890);
                assert_eq!(
                    protocol_version,
                    DfsmMessage::<FuseMessage>::DEFAULT_PROTOCOL_VERSION
                );
                assert_eq!(message, fuse_msg);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_verify_request_roundtrip() {
        let sync_epoch = SyncEpoch {
            epoch: 2,
            time: 1234567891,
            nodeid: 2,
            pid: 2000,
        };
        let msg: DfsmMessage<FuseMessage> = DfsmMessage::VerifyRequest {
            sync_epoch,
            csum_id: 0x123456789ABCDEF0,
        };
        let serialized = msg.serialize();
        let deserialized = DfsmMessage::<FuseMessage>::deserialize(&serialized).unwrap();

        match deserialized {
            DfsmMessage::VerifyRequest {
                sync_epoch: e,
                csum_id,
            } => {
                assert_eq!(e, sync_epoch);
                assert_eq!(csum_id, 0x123456789ABCDEF0);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_verify_roundtrip() {
        let sync_epoch = SyncEpoch {
            epoch: 3,
            time: 1234567892,
            nodeid: 3,
            pid: 3000,
        };
        let checksum = [42u8; 32];
        let msg: DfsmMessage<FuseMessage> = DfsmMessage::Verify {
            sync_epoch,
            csum_id: 0x1122334455667788,
            checksum,
        };
        let serialized = msg.serialize();
        let deserialized = DfsmMessage::<FuseMessage>::deserialize(&serialized).unwrap();

        match deserialized {
            DfsmMessage::Verify {
                sync_epoch: e,
                csum_id,
                checksum: recv_checksum,
            } => {
                assert_eq!(e, sync_epoch);
                assert_eq!(csum_id, 0x1122334455667788);
                assert_eq!(recv_checksum, checksum);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_invalid_magic() {
        let data = vec![0xAA, 0x00, 0x01, 0x02];
        assert!(DfsmMessage::<FuseMessage>::deserialize(&data).is_err());
    }

    #[test]
    fn test_too_short() {
        let data = vec![0xFF];
        assert!(DfsmMessage::<FuseMessage>::deserialize(&data).is_err());
    }

    // ===== Edge Case Tests =====

    #[test]
    fn test_state_message_too_short() {
        // State messages need at least 32 bytes (16 base + 16 epoch)
        let mut data = vec![0u8; 31]; // One byte short
        // Set message type to State (2)
        data[0..2].copy_from_slice(&2u16.to_le_bytes());

        let result = DfsmMessage::<FuseMessage>::deserialize(&data);
        assert!(result.is_err(), "State message with 31 bytes should fail");
        assert!(result.unwrap_err().to_string().contains("too short"));
    }

    #[test]
    fn test_normal_message_missing_count() {
        // Normal messages need count field (u64) after 16-byte header
        let mut data = vec![0u8; 20]; // Header + 4 bytes (not enough for u64 count)
        // Set message type to Normal (0)
        data[0..2].copy_from_slice(&0u16.to_le_bytes());

        let result = DfsmMessage::<FuseMessage>::deserialize(&data);
        assert!(
            result.is_err(),
            "Normal message without full count field should fail"
        );
    }

    #[test]
    fn test_verify_message_truncated_checksum() {
        // Verify messages need csum_id (8 bytes) + checksum (32 bytes) = 40 bytes payload
        let sync_epoch = SyncEpoch {
            epoch: 1,
            time: 123,
            nodeid: 1,
            pid: 100,
        };
        let mut data = Vec::new();

        // Base header (16 bytes)
        data.extend_from_slice(&6u16.to_le_bytes()); // Verify message type
        data.extend_from_slice(&0u16.to_le_bytes()); // subtype
        data.extend_from_slice(&1u32.to_le_bytes()); // protocol
        data.extend_from_slice(&123u32.to_le_bytes()); // time
        data.extend_from_slice(&0u32.to_le_bytes()); // reserved

        // Epoch (16 bytes)
        data.extend_from_slice(&sync_epoch.serialize());

        // Truncated payload (only 39 bytes instead of 40)
        data.extend_from_slice(&0x12345678u64.to_le_bytes());
        data.extend_from_slice(&[0u8; 31]); // Only 31 bytes of checksum

        let result = DfsmMessage::<FuseMessage>::deserialize(&data);
        assert!(
            result.is_err(),
            "Verify message with truncated checksum should fail"
        );
    }

    #[test]
    fn test_update_message_with_tree_entry() {
        use pmxcfs_memdb::TreeEntry;

        // Create a valid tree entry with matching size
        let data = vec![1, 2, 3, 4, 5];
        let tree_entry = TreeEntry {
            inode: 42,
            parent: 0,
            version: 1,
            writer: 0,
            name: "testfile".to_string(),
            mtime: 1234567890,
            size: data.len(), // size must match data.len()
            entry_type: 8,    // DT_REG (regular file)
            data,
        };

        let sync_epoch = SyncEpoch {
            epoch: 5,
            time: 999,
            nodeid: 2,
            pid: 200,
        };
        let msg: DfsmMessage<FuseMessage> = DfsmMessage::Update {
            sync_epoch,
            tree_entry: tree_entry.clone(),
        };

        let serialized = msg.serialize();
        let deserialized = DfsmMessage::<FuseMessage>::deserialize(&serialized).unwrap();

        match deserialized {
            DfsmMessage::Update {
                sync_epoch: e,
                tree_entry: recv_entry,
            } => {
                assert_eq!(e, sync_epoch);
                assert_eq!(recv_entry.inode, tree_entry.inode);
                assert_eq!(recv_entry.name, tree_entry.name);
                assert_eq!(recv_entry.size, tree_entry.size);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_update_complete_roundtrip() {
        let sync_epoch = SyncEpoch {
            epoch: 10,
            time: 5555,
            nodeid: 3,
            pid: 300,
        };
        let msg: DfsmMessage<FuseMessage> = DfsmMessage::UpdateComplete { sync_epoch };

        let serialized = msg.serialize();
        assert_eq!(
            serialized.len(),
            32,
            "UpdateComplete should be exactly 32 bytes (header + epoch)"
        );

        let deserialized = DfsmMessage::<FuseMessage>::deserialize(&serialized).unwrap();

        assert!(
            matches!(deserialized, DfsmMessage::UpdateComplete { sync_epoch: e } if e == sync_epoch)
        );
    }

    #[test]
    fn test_state_message_with_large_payload() {
        let sync_epoch = SyncEpoch {
            epoch: 7,
            time: 7777,
            nodeid: 4,
            pid: 400,
        };
        // Create a large payload (1MB)
        let large_data = vec![0xAB; 1024 * 1024];

        let msg: DfsmMessage<FuseMessage> = DfsmMessage::State {
            sync_epoch,
            data: large_data.clone(),
        };

        let serialized = msg.serialize();
        // Should be 32 bytes header + 1MB data
        assert_eq!(serialized.len(), 32 + 1024 * 1024);

        let deserialized = DfsmMessage::<FuseMessage>::deserialize(&serialized).unwrap();

        match deserialized {
            DfsmMessage::State {
                sync_epoch: e,
                data,
            } => {
                assert_eq!(e, sync_epoch);
                assert_eq!(data.len(), large_data.len());
                assert_eq!(data, large_data);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_message_type_detection() {
        let sync_epoch = SyncEpoch {
            epoch: 1,
            time: 100,
            nodeid: 1,
            pid: 50,
        };

        let sync_start: DfsmMessage<FuseMessage> = DfsmMessage::SyncStart { sync_epoch };
        assert_eq!(sync_start.message_type(), DfsmMessageType::SyncStart);

        let state: DfsmMessage<FuseMessage> = DfsmMessage::State {
            sync_epoch,
            data: vec![1, 2, 3],
        };
        assert_eq!(state.message_type(), DfsmMessageType::State);

        let update_complete: DfsmMessage<FuseMessage> = DfsmMessage::UpdateComplete { sync_epoch };
        assert_eq!(
            update_complete.message_type(),
            DfsmMessageType::UpdateComplete
        );
    }

    #[test]
    fn test_from_message_helper() {
        let fuse_msg = FuseMessage::Mkdir {
            path: "/new/dir".to_string(),
        };
        let msg_count = 123;
        let protocol_version = DfsmMessage::<FuseMessage>::DEFAULT_PROTOCOL_VERSION;

        let dfsm_msg = DfsmMessage::from_message(msg_count, fuse_msg.clone(), protocol_version);

        match dfsm_msg {
            DfsmMessage::Normal {
                msg_count: count,
                timestamp: _,
                protocol_version: pv,
                message,
            } => {
                assert_eq!(count, msg_count);
                assert_eq!(pv, protocol_version);
                assert_eq!(message, fuse_msg);
            }
            _ => panic!("from_message should create Normal variant"),
        }
    }

    #[test]
    fn test_verify_request_with_max_csum_id() {
        let sync_epoch = SyncEpoch {
            epoch: 99,
            time: 9999,
            nodeid: 5,
            pid: 500,
        };
        let max_csum_id = u64::MAX; // Test with maximum value

        let msg: DfsmMessage<FuseMessage> = DfsmMessage::VerifyRequest {
            sync_epoch,
            csum_id: max_csum_id,
        };

        let serialized = msg.serialize();
        let deserialized = DfsmMessage::<FuseMessage>::deserialize(&serialized).unwrap();

        match deserialized {
            DfsmMessage::VerifyRequest {
                sync_epoch: e,
                csum_id,
            } => {
                assert_eq!(e, sync_epoch);
                assert_eq!(csum_id, max_csum_id);
            }
            _ => panic!("Wrong message type"),
        }
    }
}
