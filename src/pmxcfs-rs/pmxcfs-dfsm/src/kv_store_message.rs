/// KvStore message types for DFSM status synchronization
///
/// This module defines the KvStore message types that are delivered through
/// the status DFSM state machine (pve_kvstore_v1 CPG group).
use anyhow::Context;

use crate::message::Message;

/// KvStore message type IDs (matches C's kvstore_message_t enum)
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, num_enum::TryFromPrimitive, num_enum::IntoPrimitive,
)]
#[repr(u16)]
enum KvStoreMessageType {
    Update = 1,         // KVSTORE_MESSAGE_UPDATE
    UpdateComplete = 2, // KVSTORE_MESSAGE_UPDATE_COMPLETE
    Log = 3,            // KVSTORE_MESSAGE_LOG
}

/// KvStore message types for ephemeral status synchronization
///
/// These messages are used by the kvstore DFSM (pve_kvstore_v1 CPG group)
/// to synchronize ephemeral data like RRD metrics, node IPs, and cluster logs.
///
/// Matches C implementation's KVSTORE_MESSAGE_* types in status.c
#[derive(Debug, Clone, PartialEq)]
pub enum KvStoreMessage {
    /// Update key-value data from a node
    ///
    /// Wire format: key (256 bytes, null-terminated) + value (variable length)
    /// Matches C's KVSTORE_MESSAGE_UPDATE
    Update { key: String, value: Vec<u8> },

    /// Cluster log entry
    ///
    /// Wire format: clog_entry_t struct
    /// Matches C's KVSTORE_MESSAGE_LOG
    Log {
        time: u32,
        priority: u8,
        node: String,
        ident: String,
        tag: String,
        message: String,
    },

    /// Update complete signal (not currently used)
    ///
    /// Matches C's KVSTORE_MESSAGE_UPDATE_COMPLETE
    UpdateComplete,
}

impl KvStoreMessage {
    /// Get message type ID (matches C's kvstore_message_t enum)
    pub fn message_type(&self) -> u16 {
        let msg_type = match self {
            KvStoreMessage::Update { .. } => KvStoreMessageType::Update,
            KvStoreMessage::UpdateComplete => KvStoreMessageType::UpdateComplete,
            KvStoreMessage::Log { .. } => KvStoreMessageType::Log,
        };
        msg_type.into()
    }

    /// Serialize to C-compatible wire format
    ///
    /// Update format: key (256 bytes, null-terminated) + value (variable)
    /// Log format: clog_entry_t struct
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            KvStoreMessage::Update { key, value } => {
                // C format: char key[256] + data
                let mut buf = vec![0u8; 256];
                let key_bytes = key.as_bytes();
                let copy_len = key_bytes.len().min(255); // Leave room for null terminator
                buf[..copy_len].copy_from_slice(&key_bytes[..copy_len]);
                // buf is already zero-filled, so null terminator is automatic

                buf.extend_from_slice(value);
                buf
            }
            KvStoreMessage::Log {
                time,
                priority,
                node,
                ident,
                tag,
                message,
            } => {
                // C format: clog_entry_t
                // struct clog_entry_t {
                //     uint32_t time;
                //     uint8_t priority;
                //     uint8_t padding[3];
                //     uint32_t node_len, ident_len, tag_len, msg_len;
                //     char data[];  // node + ident + tag + message (all null-terminated)
                // }

                let node_bytes = node.as_bytes();
                let ident_bytes = ident.as_bytes();
                let tag_bytes = tag.as_bytes();
                let msg_bytes = message.as_bytes();

                let node_len = (node_bytes.len() + 1) as u32; // +1 for null
                let ident_len = (ident_bytes.len() + 1) as u32;
                let tag_len = (tag_bytes.len() + 1) as u32;
                let msg_len = (msg_bytes.len() + 1) as u32;

                let total_len = 4 + 1 + 3 + 16 + node_len + ident_len + tag_len + msg_len;
                let mut buf = Vec::with_capacity(total_len as usize);

                buf.extend_from_slice(&time.to_le_bytes());
                buf.push(*priority);
                buf.extend_from_slice(&[0u8; 3]); // padding
                buf.extend_from_slice(&node_len.to_le_bytes());
                buf.extend_from_slice(&ident_len.to_le_bytes());
                buf.extend_from_slice(&tag_len.to_le_bytes());
                buf.extend_from_slice(&msg_len.to_le_bytes());

                buf.extend_from_slice(node_bytes);
                buf.push(0); // null terminator
                buf.extend_from_slice(ident_bytes);
                buf.push(0);
                buf.extend_from_slice(tag_bytes);
                buf.push(0);
                buf.extend_from_slice(msg_bytes);
                buf.push(0);

                buf
            }
            KvStoreMessage::UpdateComplete => {
                // No payload
                Vec::new()
            }
        }
    }

    /// Deserialize from C-compatible wire format
    pub fn deserialize(msg_type: u16, data: &[u8]) -> anyhow::Result<Self> {
        use KvStoreMessageType::*;

        let msg_type = KvStoreMessageType::try_from(msg_type)
            .map_err(|_| anyhow::anyhow!("Unknown kvstore message type: {msg_type}"))?;

        match msg_type {
            Update => {
                if data.len() < 256 {
                    anyhow::bail!("UPDATE message too short: {} < 256", data.len());
                }

                // Find null terminator in first 256 bytes
                let key_end = data[..256]
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or_else(|| anyhow::anyhow!("UPDATE key not null-terminated"))?;

                let key = std::str::from_utf8(&data[..key_end])
                    .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in UPDATE key: {e}"))?
                    .to_string();

                let value = data[256..].to_vec();

                Ok(KvStoreMessage::Update { key, value })
            }
            UpdateComplete => Ok(KvStoreMessage::UpdateComplete),
            Log => {
                if data.len() < 20 {
                    // Minimum: 4+1+3+16 = 24 bytes header
                    anyhow::bail!("LOG message too short");
                }

                let time = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                let priority = data[4];
                // data[5..8] is padding

                let node_len = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
                let ident_len =
                    u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
                let tag_len = u32::from_le_bytes([data[16], data[17], data[18], data[19]]) as usize;
                let msg_len = u32::from_le_bytes([data[20], data[21], data[22], data[23]]) as usize;

                let expected_len = 24 + node_len + ident_len + tag_len + msg_len;
                if data.len() != expected_len {
                    anyhow::bail!(
                        "LOG message size mismatch: {} != {}",
                        data.len(),
                        expected_len
                    );
                }

                let mut offset = 24;

                let node = std::str::from_utf8(&data[offset..offset + node_len - 1])
                    .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in LOG node: {e}"))?
                    .to_string();
                offset += node_len;

                let ident = std::str::from_utf8(&data[offset..offset + ident_len - 1])
                    .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in LOG ident: {e}"))?
                    .to_string();
                offset += ident_len;

                let tag = std::str::from_utf8(&data[offset..offset + tag_len - 1])
                    .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in LOG tag: {e}"))?
                    .to_string();
                offset += tag_len;

                let message = std::str::from_utf8(&data[offset..offset + msg_len - 1])
                    .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in LOG message: {e}"))?
                    .to_string();

                Ok(KvStoreMessage::Log {
                    time,
                    priority,
                    node,
                    ident,
                    tag,
                    message,
                })
            }
        }
    }
}

impl Message for KvStoreMessage {
    fn message_type(&self) -> u16 {
        // Delegate to the existing method
        KvStoreMessage::message_type(self)
    }

    fn serialize(&self) -> Vec<u8> {
        // Delegate to the existing method
        KvStoreMessage::serialize(self)
    }

    fn deserialize(message_type: u16, data: &[u8]) -> anyhow::Result<Self> {
        // Delegate to the existing method
        KvStoreMessage::deserialize(message_type, data)
            .context("Failed to deserialize KvStoreMessage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kvstore_message_update_serialization() {
        let msg = KvStoreMessage::Update {
            key: "test_key".to_string(),
            value: vec![1, 2, 3, 4, 5],
        };

        let serialized = msg.serialize();
        assert_eq!(serialized.len(), 256 + 5);
        assert_eq!(&serialized[..8], b"test_key");
        assert_eq!(serialized[8], 0); // null terminator
        assert_eq!(&serialized[256..], &[1, 2, 3, 4, 5]);

        let deserialized = KvStoreMessage::deserialize(1, &serialized).unwrap();
        assert_eq!(msg, deserialized);
    }

    #[test]
    fn test_kvstore_message_log_serialization() {
        let msg = KvStoreMessage::Log {
            time: 1234567890,
            priority: 5,
            node: "node1".to_string(),
            ident: "pmxcfs".to_string(),
            tag: "info".to_string(),
            message: "test message".to_string(),
        };

        let serialized = msg.serialize();
        let deserialized = KvStoreMessage::deserialize(3, &serialized).unwrap();
        assert_eq!(msg, deserialized);
    }

    #[test]
    fn test_kvstore_message_type() {
        assert_eq!(
            KvStoreMessage::Update {
                key: "".into(),
                value: vec![]
            }
            .message_type(),
            1
        );
        assert_eq!(KvStoreMessage::UpdateComplete.message_type(), 2);
        assert_eq!(
            KvStoreMessage::Log {
                time: 0,
                priority: 0,
                node: "".into(),
                ident: "".into(),
                tag: "".into(),
                message: "".into()
            }
            .message_type(),
            3
        );
    }

    #[test]
    fn test_kvstore_message_type_roundtrip() {
        // Test that message_type() and deserialize() are consistent
        use super::KvStoreMessageType;

        assert_eq!(u16::from(KvStoreMessageType::Update), 1);
        assert_eq!(u16::from(KvStoreMessageType::UpdateComplete), 2);
        assert_eq!(u16::from(KvStoreMessageType::Log), 3);

        assert_eq!(
            KvStoreMessageType::try_from(1).unwrap(),
            KvStoreMessageType::Update
        );
        assert_eq!(
            KvStoreMessageType::try_from(2).unwrap(),
            KvStoreMessageType::UpdateComplete
        );
        assert_eq!(
            KvStoreMessageType::try_from(3).unwrap(),
            KvStoreMessageType::Log
        );

        assert!(KvStoreMessageType::try_from(0).is_err());
        assert!(KvStoreMessageType::try_from(4).is_err());
    }
}
