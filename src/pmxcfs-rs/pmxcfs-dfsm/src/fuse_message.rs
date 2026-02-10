/// FUSE message types for cluster synchronization
///
/// These are the high-level operations that get broadcast through the cluster
/// via the main database DFSM (pmxcfs_v1 CPG group).
use anyhow::{Context, Result};

use crate::message::Message;
use crate::wire_format::{CFuseMessage, CMessageType};

#[derive(Debug, Clone, PartialEq)]
pub enum FuseMessage {
    /// Create a regular file
    Create { path: String },
    /// Create a directory
    Mkdir { path: String },
    /// Write data to a file
    Write {
        path: String,
        offset: u64,
        data: Vec<u8>,
    },
    /// Delete a file or directory
    Delete { path: String },
    /// Rename/move a file or directory
    Rename { from: String, to: String },
    /// Update modification time
    /// Note: mtime is sent via offset field in CFuseMessage (C: dcdb.c:900)
    Mtime { path: String, mtime: u32 },
    /// Request unlock (not yet implemented)
    UnlockRequest { path: String },
    /// Unlock (not yet implemented)
    Unlock { path: String },
}

impl Message for FuseMessage {
    fn message_type(&self) -> u16 {
        match self {
            FuseMessage::Create { .. } => CMessageType::Create as u16,
            FuseMessage::Mkdir { .. } => CMessageType::Mkdir as u16,
            FuseMessage::Write { .. } => CMessageType::Write as u16,
            FuseMessage::Delete { .. } => CMessageType::Delete as u16,
            FuseMessage::Rename { .. } => CMessageType::Rename as u16,
            FuseMessage::Mtime { .. } => CMessageType::Mtime as u16,
            FuseMessage::UnlockRequest { .. } => CMessageType::UnlockRequest as u16,
            FuseMessage::Unlock { .. } => CMessageType::Unlock as u16,
        }
    }

    fn serialize(&self) -> Vec<u8> {
        let c_msg = match self {
            FuseMessage::Create { path } => CFuseMessage {
                size: 0,
                offset: 0,
                flags: 0,
                path: path.clone(),
                to: None,
                data: Vec::new(),
            },
            FuseMessage::Mkdir { path } => CFuseMessage {
                size: 0,
                offset: 0,
                flags: 0,
                path: path.clone(),
                to: None,
                data: Vec::new(),
            },
            FuseMessage::Write { path, offset, data } => CFuseMessage {
                size: data.len() as u32,
                offset: *offset as u32,
                flags: 0,
                path: path.clone(),
                to: None,
                data: data.clone(),
            },
            FuseMessage::Delete { path } => CFuseMessage {
                size: 0,
                offset: 0,
                flags: 0,
                path: path.clone(),
                to: None,
                data: Vec::new(),
            },
            FuseMessage::Rename { from, to } => CFuseMessage {
                size: 0,
                offset: 0,
                flags: 0,
                path: from.clone(),
                to: Some(to.clone()),
                data: Vec::new(),
            },
            FuseMessage::Mtime { path, mtime } => CFuseMessage {
                size: 0,
                offset: *mtime as u32,  // mtime is sent via offset field (C: dcdb.c:900)
                flags: 0,
                path: path.clone(),
                to: None,
                data: Vec::new(),
            },
            FuseMessage::UnlockRequest { path } => CFuseMessage {
                size: 0,
                offset: 0,
                flags: 0,
                path: path.clone(),
                to: None,
                data: Vec::new(),
            },
            FuseMessage::Unlock { path } => CFuseMessage {
                size: 0,
                offset: 0,
                flags: 0,
                path: path.clone(),
                to: None,
                data: Vec::new(),
            },
        };

        c_msg.serialize()
    }

    fn deserialize(message_type: u16, data: &[u8]) -> Result<Self> {
        let c_msg = CFuseMessage::parse(data).context("Failed to parse C FUSE message")?;
        let msg_type = CMessageType::try_from(message_type).context("Invalid C message type")?;

        Ok(match msg_type {
            CMessageType::Create => FuseMessage::Create { path: c_msg.path },
            CMessageType::Mkdir => FuseMessage::Mkdir { path: c_msg.path },
            CMessageType::Write => FuseMessage::Write {
                path: c_msg.path,
                offset: c_msg.offset as u64,
                data: c_msg.data,
            },
            CMessageType::Delete => FuseMessage::Delete { path: c_msg.path },
            CMessageType::Rename => FuseMessage::Rename {
                from: c_msg.path,
                to: c_msg.to.unwrap_or_default(),
            },
            CMessageType::Mtime => FuseMessage::Mtime {
                path: c_msg.path,
                mtime: c_msg.offset as u32,  // mtime is sent via offset field (C: dcdb.c:900)
            },
            CMessageType::UnlockRequest => FuseMessage::UnlockRequest { path: c_msg.path },
            CMessageType::Unlock => FuseMessage::Unlock { path: c_msg.path },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuse_message_create() {
        let msg = FuseMessage::Create {
            path: "/test/file".to_string(),
        };
        assert_eq!(msg.message_type(), CMessageType::Create as u16);

        let serialized = msg.serialize();
        let deserialized = FuseMessage::deserialize(msg.message_type(), &serialized).unwrap();
        assert_eq!(msg, deserialized);
    }

    #[test]
    fn test_fuse_message_write() {
        let msg = FuseMessage::Write {
            path: "/test/file".to_string(),
            offset: 100,
            data: vec![1, 2, 3, 4, 5],
        };
        assert_eq!(msg.message_type(), CMessageType::Write as u16);

        let serialized = msg.serialize();
        let deserialized = FuseMessage::deserialize(msg.message_type(), &serialized).unwrap();
        assert_eq!(msg, deserialized);
    }

    #[test]
    fn test_fuse_message_rename() {
        let msg = FuseMessage::Rename {
            from: "/old/path".to_string(),
            to: "/new/path".to_string(),
        };
        assert_eq!(msg.message_type(), CMessageType::Rename as u16);

        let serialized = msg.serialize();
        let deserialized = FuseMessage::deserialize(msg.message_type(), &serialized).unwrap();
        assert_eq!(msg, deserialized);
    }
}
