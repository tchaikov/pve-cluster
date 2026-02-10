/// High-level message abstraction for DFSM
///
/// This module provides a Message trait for working with cluster messages
/// at a higher abstraction level than raw bytes.
use anyhow::Result;

/// Trait for messages that can be sent through DFSM
pub trait Message: Clone + std::fmt::Debug + Send + Sync + Sized + 'static {
    /// Get the message type identifier
    fn message_type(&self) -> u16;

    /// Serialize the message to bytes (application message payload only)
    ///
    /// This serializes only the application-level payload. The DFSM protocol
    /// headers (msg_count, timestamp, protocol_version, etc.) are added by
    /// DfsmMessage::serialize() when wrapping in DfsmMessage::Normal.
    fn serialize(&self) -> Vec<u8>;

    /// Deserialize from bytes given a message type
    fn deserialize(message_type: u16, data: &[u8]) -> Result<Self>;
}
