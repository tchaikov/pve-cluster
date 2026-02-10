//! libqb wire protocol structures and constants
//!
//! This module contains the low-level protocol definitions for libqb IPC communication.
//! All structures must match the C counterparts exactly for binary compatibility.

/// Message ID for authentication requests (matches libqb's QB_IPC_MSG_AUTHENTICATE)
pub(super) const MSG_AUTHENTICATE: i32 = -1;

/// Connection type for shared memory transport (matches libqb's QB_IPC_SHM)
pub(super) const CONNECTION_TYPE_SHM: u32 = 1;

/// Maximum path length - used in connection response
pub(super) const PATH_MAX: usize = 4096;

/// Wrapper for i32 that aligns to 8-byte boundary with explicit padding
///
/// Simulates C's `__attribute__ ((aligned(8)))` on individual i32 fields.
/// This is used to match libqb's per-field alignment behavior.
///
/// Memory layout:
/// - Bytes 0-3: i32 value
/// - Bytes 4-7: zero padding
/// - Total: 8 bytes
#[repr(C, align(8))]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Align8 {
    pub value: i32,
    _pad: u32, // 4 bytes padding for i32 -> 8 bytes total
}

impl Align8 {
    #[inline]
    pub const fn new(value: i32) -> Self {
        Align8 { value, _pad: 0 }
    }
}

impl std::ops::Deref for Align8 {
    type Target = i32;

    #[inline]
    fn deref(&self) -> &i32 {
        &self.value
    }
}

impl std::ops::DerefMut for Align8 {
    #[inline]
    fn deref_mut(&mut self) -> &mut i32 {
        &mut self.value
    }
}

impl From<i32> for Align8 {
    #[inline]
    fn from(value: i32) -> Self {
        Align8::new(value)
    }
}

impl Default for Align8 {
    #[inline]
    fn default() -> Self {
        Align8::new(0)
    }
}

/// Request header (matches libqb's qb_ipc_request_header)
///
/// Each field is 8-byte aligned to match C's __attribute__ ((aligned(8)))
#[repr(C, align(8))]
#[derive(Debug, Copy, Clone)]
pub struct RequestHeader {
    pub id: Align8,
    pub size: Align8,
}

/// Response header (matches libqb's qb_ipc_response_header)
#[repr(C, align(8))]
#[derive(Debug, Copy, Clone)]
pub struct ResponseHeader {
    pub id: Align8,
    pub size: Align8,
    pub error: Align8,
}

/// Connection request sent by client during handshake (matches libqb's qb_ipc_connection_request)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub(super) struct ConnectionRequest {
    pub hdr: RequestHeader,
    pub max_msg_size: u32,
}

/// Connection response sent by server during handshake (matches libqb's qb_ipc_connection_response)
#[repr(C, align(8))]
#[derive(Debug)]
pub(super) struct ConnectionResponse {
    pub hdr: ResponseHeader,
    pub connection_type: u32,
    pub max_msg_size: u32,
    pub connection: usize,
    pub request: [u8; PATH_MAX],
    pub response: [u8; PATH_MAX],
    pub event: [u8; PATH_MAX],
}

/// Request passed to handlers
///
/// Contains all information about an IPC request including the message ID,
/// payload data, and connection context (uid, gid, pid, permissions).
#[derive(Debug, Clone)]
pub struct Request {
    /// Message ID identifying the operation (application-defined)
    pub msg_id: i32,

    /// Request payload data
    pub data: Vec<u8>,

    /// Whether this connection has read-only access
    pub is_read_only: bool,

    /// Connection ID (for logging/debugging)
    pub conn_id: u64,

    /// Client user ID (from SO_PEERCRED)
    pub uid: u32,

    /// Client group ID (from SO_PEERCRED)
    pub gid: u32,

    /// Client process ID (from SO_PEERCRED)
    pub pid: u32,
}

/// Response from handlers
///
/// Contains the error code and response data to send back to the client.
#[derive(Debug, Clone)]
pub struct Response {
    /// Error code (0 = success, negative = errno)
    pub error_code: i32,

    /// Response payload data
    pub data: Vec<u8>,
}

impl Response {
    /// Create a successful response with data
    pub fn ok(data: Vec<u8>) -> Self {
        Self {
            error_code: 0,
            data,
        }
    }

    /// Create an error response with errno
    pub fn err(error_code: i32) -> Self {
        Self {
            error_code,
            data: Vec::new(),
        }
    }

    /// Create an error response with errno and optional data
    pub fn with_error(error_code: i32, data: Vec<u8>) -> Self {
        Self { error_code, data }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_sizes() {
        assert_eq!(std::mem::size_of::<RequestHeader>(), 16);
        assert_eq!(std::mem::align_of::<RequestHeader>(), 8);
        assert_eq!(std::mem::size_of::<ResponseHeader>(), 24);
        assert_eq!(std::mem::align_of::<ResponseHeader>(), 8);
        assert_eq!(std::mem::size_of::<ConnectionRequest>(), 24); // 16 (header) + 4 (max_msg_size) + 4 (padding)

        println!(
            "ConnectionResponse size: {}",
            std::mem::size_of::<ConnectionResponse>()
        );
        println!(
            "ConnectionResponse align: {}",
            std::mem::align_of::<ConnectionResponse>()
        );
        println!("PATH_MAX: {PATH_MAX}");

        // C expects: 24 (header) + 4 (connection_type) + 4 (max_msg_size) + 8 (connection pointer) + 3*4096 (paths) = 12328
        assert_eq!(std::mem::size_of::<ConnectionResponse>(), 12328);
    }

    // ===== Align8 Tests =====

    #[test]
    fn test_align8_size_and_alignment() {
        // Verify Align8 is exactly 8 bytes
        assert_eq!(std::mem::size_of::<Align8>(), 8);
        assert_eq!(std::mem::align_of::<Align8>(), 8);
    }

    #[test]
    fn test_align8_creation_and_value_access() {
        let a = Align8::new(42);
        assert_eq!(a.value, 42);
        assert_eq!(*a, 42); // Test Deref
    }

    #[test]
    fn test_align8_from_i32() {
        let a: Align8 = (-100).into();
        assert_eq!(a.value, -100);
    }

    #[test]
    fn test_align8_default() {
        let a = Align8::default();
        assert_eq!(a.value, 0);
    }

    #[test]
    fn test_align8_deref_mut() {
        let mut a = Align8::new(10);
        *a = 20; // Test DerefMut
        assert_eq!(a.value, 20);
    }

    #[test]
    fn test_align8_padding_is_zero() {
        let a = Align8::new(123);
        // Padding should always be 0
        assert_eq!(a._pad, 0);
    }

    // ===== Response Tests =====

    #[test]
    fn test_response_ok_creation() {
        let data = b"test data".to_vec();
        let resp = Response::ok(data.clone());

        assert_eq!(resp.error_code, 0);
        assert_eq!(resp.data, data);
    }

    #[test]
    fn test_response_err_creation() {
        let resp = Response::err(-5); // ERRNO like EIO

        assert_eq!(resp.error_code, -5);
        assert!(resp.data.is_empty());
    }

    #[test]
    fn test_response_with_error_and_data() {
        let data = b"error details".to_vec();
        let resp = Response::with_error(-22, data.clone()); // EINVAL

        assert_eq!(resp.error_code, -22);
        assert_eq!(resp.data, data);
    }

    #[test]
    fn test_response_error_codes() {
        // Test various errno values
        let test_cases = vec![
            (0, "success"),
            (-1, "EPERM"),
            (-2, "ENOENT"),
            (-13, "EACCES"),
            (-22, "EINVAL"),
        ];

        for (code, _name) in test_cases {
            let resp = Response::err(code);
            assert_eq!(resp.error_code, code);
        }
    }

    // ===== Request Tests =====

    #[test]
    fn test_request_creation() {
        let req = Request {
            msg_id: 100,
            data: b"payload".to_vec(),
            is_read_only: false,
            conn_id: 12345,
            uid: 0,
            gid: 0,
            pid: 999,
        };

        assert_eq!(req.msg_id, 100);
        assert_eq!(req.data, b"payload");
        assert!(!req.is_read_only);
        assert_eq!(req.conn_id, 12345);
        assert_eq!(req.uid, 0);
        assert_eq!(req.gid, 0);
        assert_eq!(req.pid, 999);
    }

    #[test]
    fn test_request_read_only_flag() {
        let req_ro = Request {
            msg_id: 1,
            data: vec![],
            is_read_only: true,
            conn_id: 1,
            uid: 33,
            gid: 33,
            pid: 1000,
        };

        let req_rw = Request {
            msg_id: 1,
            data: vec![],
            is_read_only: false,
            conn_id: 2,
            uid: 0,
            gid: 0,
            pid: 1001,
        };

        assert!(req_ro.is_read_only);
        assert!(!req_rw.is_read_only);
    }
}
