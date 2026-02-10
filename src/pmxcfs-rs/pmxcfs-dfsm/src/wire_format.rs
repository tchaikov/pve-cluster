/// C-compatible wire format for cluster communication
///
/// This module implements the exact wire protocol used by the C version of pmxcfs
/// to ensure compatibility with C-based cluster nodes.
///
/// The C version uses a simple format with iovec arrays containing raw C types.
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use std::ffi::CStr;

/// C message types (must match dcdb.h)
#[derive(Debug, Clone, Copy, PartialEq, Eq, num_enum::TryFromPrimitive)]
#[repr(u16)]
pub enum CMessageType {
    Write = 1,
    Mkdir = 2,
    Delete = 3,
    Rename = 4,
    Create = 5,
    Mtime = 6,
    UnlockRequest = 7,
    Unlock = 8,
}

/// C-compatible FUSE message header
/// Layout matches the iovec array from C: [size][offset][pathlen][tolen][flags]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct CFuseMessageHeader {
    size: u32,
    offset: u32,
    pathlen: u32,
    tolen: u32,
    flags: u32,
}

/// Parsed C FUSE message
#[derive(Debug, Clone)]
pub struct CFuseMessage {
    pub size: u32,
    pub offset: u32,
    pub flags: u32,
    pub path: String,
    pub to: Option<String>,
    pub data: Vec<u8>,
}

impl CFuseMessage {
    /// Parse a C FUSE message from raw bytes
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < std::mem::size_of::<CFuseMessageHeader>() {
            return Err(anyhow::anyhow!(
                "Message too short: {} < {}",
                data.len(),
                std::mem::size_of::<CFuseMessageHeader>()
            ));
        }

        // Parse header manually to avoid alignment issues
        let header = CFuseMessageHeader {
            size: u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            offset: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            pathlen: u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            tolen: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
            flags: u32::from_le_bytes([data[16], data[17], data[18], data[19]]),
        };

        // Check for integer overflow in total size calculation
        let total_size = header
            .pathlen
            .checked_add(header.tolen)
            .and_then(|s| s.checked_add(header.size))
            .ok_or_else(|| anyhow::anyhow!("Integer overflow in message size calculation"))?;

        // Validate total size is reasonable (prevent DoS)
        const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024; // 16MB
        if total_size > MAX_MESSAGE_SIZE {
            return Err(anyhow::anyhow!(
                "Message size {total_size} exceeds maximum {MAX_MESSAGE_SIZE}"
            ));
        }

        let mut offset = std::mem::size_of::<CFuseMessageHeader>();

        // Parse path with overflow-checked arithmetic
        let path = if header.pathlen > 0 {
            let end_offset = offset
                .checked_add(header.pathlen as usize)
                .ok_or_else(|| anyhow::anyhow!("Integer overflow in path offset"))?;

            if end_offset > data.len() {
                return Err(anyhow::anyhow!(
                    "Invalid path length: {} bytes at offset {} exceeds message size {}",
                    header.pathlen,
                    offset,
                    data.len()
                ));
            }
            let path_bytes = &data[offset..end_offset];
            offset = end_offset;

            // C strings are null-terminated
            CStr::from_bytes_until_nul(path_bytes)
                .context("Invalid path string")?
                .to_str()
                .context("Path not valid UTF-8")?
                .to_string()
        } else {
            String::new()
        };

        // Parse 'to' (for rename operations) with overflow-checked arithmetic
        let to = if header.tolen > 0 {
            let end_offset = offset
                .checked_add(header.tolen as usize)
                .ok_or_else(|| anyhow::anyhow!("Integer overflow in 'to' offset"))?;

            if end_offset > data.len() {
                return Err(anyhow::anyhow!(
                    "Invalid 'to' length: {} bytes at offset {} exceeds message size {}",
                    header.tolen,
                    offset,
                    data.len()
                ));
            }
            let to_bytes = &data[offset..end_offset];
            offset = end_offset;

            Some(
                CStr::from_bytes_until_nul(to_bytes)
                    .context("Invalid to string")?
                    .to_str()
                    .context("To path not valid UTF-8")?
                    .to_string(),
            )
        } else {
            None
        };

        // Parse data buffer with overflow-checked arithmetic
        let buf_data = if header.size > 0 {
            let end_offset = offset
                .checked_add(header.size as usize)
                .ok_or_else(|| anyhow::anyhow!("Integer overflow in data offset"))?;

            if end_offset > data.len() {
                return Err(anyhow::anyhow!(
                    "Invalid data size: {} bytes at offset {} exceeds message size {}",
                    header.size,
                    offset,
                    data.len()
                ));
            }
            data[offset..end_offset].to_vec()
        } else {
            Vec::new()
        };

        Ok(CFuseMessage {
            size: header.size,
            offset: header.offset,
            flags: header.flags,
            path,
            to,
            data: buf_data,
        })
    }

    /// Serialize to C wire format
    pub fn serialize(&self) -> Vec<u8> {
        let path_bytes = self.path.as_bytes();
        let pathlen = if path_bytes.is_empty() {
            0
        } else {
            (path_bytes.len() + 1) as u32 // +1 for null terminator
        };

        let to_bytes = self.to.as_ref().map(|s| s.as_bytes()).unwrap_or(&[]);
        let tolen = if to_bytes.is_empty() {
            0
        } else {
            (to_bytes.len() + 1) as u32
        };

        let header = CFuseMessageHeader {
            size: self.size,
            offset: self.offset,
            pathlen,
            tolen,
            flags: self.flags,
        };

        let mut result = Vec::new();

        // Serialize header
        result.extend_from_slice(bytemuck::bytes_of(&header));

        // Serialize path (with null terminator)
        if pathlen > 0 {
            result.extend_from_slice(path_bytes);
            result.push(0); // null terminator
        }

        // Serialize 'to' (with null terminator)
        if tolen > 0 {
            result.extend_from_slice(to_bytes);
            result.push(0); // null terminator
        }

        // Serialize data
        if self.size > 0 {
            result.extend_from_slice(&self.data);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize_write() {
        let msg = CFuseMessage {
            size: 13,
            offset: 0,
            flags: 0,
            path: "/test.txt".to_string(),
            to: None,
            data: b"Hello, World!".to_vec(),
        };

        let serialized = msg.serialize();
        let parsed = CFuseMessage::parse(&serialized).unwrap();

        assert_eq!(parsed.size, msg.size);
        assert_eq!(parsed.offset, msg.offset);
        assert_eq!(parsed.flags, msg.flags);
        assert_eq!(parsed.path, msg.path);
        assert_eq!(parsed.to, msg.to);
        assert_eq!(parsed.data, msg.data);
    }

    #[test]
    fn test_serialize_deserialize_rename() {
        let msg = CFuseMessage {
            size: 0,
            offset: 0,
            flags: 0,
            path: "/old.txt".to_string(),
            to: Some("/new.txt".to_string()),
            data: Vec::new(),
        };

        let serialized = msg.serialize();
        let parsed = CFuseMessage::parse(&serialized).unwrap();

        assert_eq!(parsed.path, msg.path);
        assert_eq!(parsed.to, msg.to);
    }
}
