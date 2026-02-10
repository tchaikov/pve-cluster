//! Authentication tests for pmxcfs-ipc
//!
//! These tests verify that the Handler::authenticate() mechanism works correctly
//! for different authentication policies.
//!
//! Note: These tests use real Unix sockets, so they test authentication behavior
//! from the server's perspective. The UID/GID will be the test process's credentials,
//! so we test the Handler logic rather than OS-level credential checking.
use async_trait::async_trait;
use pmxcfs_ipc::{Handler, Permissions, Request, Response, Server};
use pmxcfs_test_utils::wait_for_condition_blocking;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

/// Helper to create a unique service name for each test
fn unique_service_name() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!("auth-test-{}", COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Helper to connect using the qb_wire_compat FFI client
/// Returns true if connection succeeded, false if rejected
fn try_connect(service_name: &str) -> bool {
    use std::ffi::CString;

    #[repr(C)]
    struct QbIpccConnection {
        _private: [u8; 0],
    }

    #[link(name = "qb")]
    unsafe extern "C" {
        fn qb_ipcc_connect(name: *const libc::c_char, max_msg_size: usize)
        -> *mut QbIpccConnection;
        fn qb_ipcc_disconnect(conn: *mut QbIpccConnection);
    }

    let name = CString::new(service_name).expect("Invalid service name");
    let conn = unsafe { qb_ipcc_connect(name.as_ptr(), 8192) };

    let success = !conn.is_null();

    if success {
        unsafe { qb_ipcc_disconnect(conn) };
    }

    success
}

// ============================================================================
// Test Handlers with Different Authentication Policies
// ============================================================================

/// Handler that accepts all connections with read-write access
struct AcceptAllHandler;

#[async_trait]
impl Handler for AcceptAllHandler {
    fn authenticate(&self, _uid: u32, _gid: u32) -> Option<Permissions> {
        Some(Permissions::ReadWrite)
    }

    async fn handle(&self, _request: Request) -> Response {
        Response::ok(b"test".to_vec())
    }
}

/// Handler that rejects all connections
struct RejectAllHandler;

#[async_trait]
impl Handler for RejectAllHandler {
    fn authenticate(&self, _uid: u32, _gid: u32) -> Option<Permissions> {
        None
    }

    async fn handle(&self, _request: Request) -> Response {
        Response::ok(b"test".to_vec())
    }
}

/// Handler that only accepts root (uid=0)
struct RootOnlyHandler;

#[async_trait]
impl Handler for RootOnlyHandler {
    fn authenticate(&self, uid: u32, _gid: u32) -> Option<Permissions> {
        if uid == 0 {
            Some(Permissions::ReadWrite)
        } else {
            None
        }
    }

    async fn handle(&self, _request: Request) -> Response {
        Response::ok(b"test".to_vec())
    }
}

/// Handler that tracks authentication calls
struct TrackingHandler {
    call_count: Arc<AtomicU32>,
    last_uid: Arc<AtomicU32>,
    last_gid: Arc<AtomicU32>,
}

impl TrackingHandler {
    fn new() -> (Self, Arc<AtomicU32>, Arc<AtomicU32>, Arc<AtomicU32>) {
        let call_count = Arc::new(AtomicU32::new(0));
        let last_uid = Arc::new(AtomicU32::new(0));
        let last_gid = Arc::new(AtomicU32::new(0));

        (
            Self {
                call_count: call_count.clone(),
                last_uid: last_uid.clone(),
                last_gid: last_gid.clone(),
            },
            call_count,
            last_uid,
            last_gid,
        )
    }
}

#[async_trait]
impl Handler for TrackingHandler {
    fn authenticate(&self, uid: u32, gid: u32) -> Option<Permissions> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.last_uid.store(uid, Ordering::SeqCst);
        self.last_gid.store(gid, Ordering::SeqCst);
        Some(Permissions::ReadWrite)
    }

    async fn handle(&self, _request: Request) -> Response {
        Response::ok(b"test".to_vec())
    }
}

/// Handler that grants read-only access to non-root
struct ReadOnlyForNonRootHandler;

#[async_trait]
impl Handler for ReadOnlyForNonRootHandler {
    fn authenticate(&self, uid: u32, _gid: u32) -> Option<Permissions> {
        if uid == 0 {
            Some(Permissions::ReadWrite)
        } else {
            Some(Permissions::ReadOnly)
        }
    }

    async fn handle(&self, request: Request) -> Response {
        // read_only field is visible to the handler via the connection
        // For testing purposes, just accept requests
        Response::ok(format!("handled msg_id {}", request.msg_id).into_bytes())
    }
}

// ============================================================================
// Helper to start server in background thread
// ============================================================================

fn start_server<H: Handler + 'static>(service_name: String, handler: H) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            let mut server = Server::new(&service_name, handler);
            server.start().expect("Server startup failed");
            std::future::pending::<()>().await;
        });
    })
}

/// Wait for server to be ready by checking if socket file exists
fn wait_for_server_ready(service_name: &str) {
    // The socket is created in /dev/shm/qb-{service_name}-*
    // We'll just try to connect repeatedly until successful or timeout
    assert!(
        wait_for_condition_blocking(
            || {
                // Try a quick connection attempt
                // For servers that accept connections, this will succeed
                // For servers that reject, the socket will at least exist

                let socket_pattern = format!("/dev/shm/qb-{service_name}-");
                // Check if any socket file matching the pattern exists
                if let Ok(entries) = std::fs::read_dir("/dev/shm") {
                    for entry in entries.flatten() {
                        if let Ok(name) = entry.file_name().into_string()
                            && name.starts_with(&socket_pattern)
                        {
                            return true;
                        }
                    }
                }
                false
            },
            Duration::from_secs(5),
            Duration::from_millis(10),
        ),
        "Server should be ready within 5 seconds"
    );
}

// ============================================================================
// Tests
// ============================================================================

#[test]
#[ignore] // Requires libqb-dev
fn test_accept_all_handler() {
    let service_name = unique_service_name();
    let _server = start_server(service_name.clone(), AcceptAllHandler);

    wait_for_server_ready(&service_name);

    assert!(
        try_connect(&service_name),
        "AcceptAllHandler should accept connection"
    );
}

#[test]
#[ignore] // Requires libqb-dev
fn test_reject_all_handler() {
    let service_name = unique_service_name();
    let _server = start_server(service_name.clone(), RejectAllHandler);

    wait_for_server_ready(&service_name);

    assert!(
        !try_connect(&service_name),
        "RejectAllHandler should reject connection"
    );
}

#[test]
#[ignore] // Requires libqb-dev
fn test_root_only_handler() {
    let service_name = unique_service_name();
    let _server = start_server(service_name.clone(), RootOnlyHandler);

    wait_for_server_ready(&service_name);

    let connected = try_connect(&service_name);

    // Get current uid
    let current_uid = unsafe { libc::getuid() };

    if current_uid == 0 {
        assert!(
            connected,
            "RootOnlyHandler should accept connection when running as root"
        );
    } else {
        assert!(
            !connected,
            "RootOnlyHandler should reject connection when not running as root (uid={current_uid})"
        );
    }
}

#[test]
#[ignore] // Requires libqb-dev
fn test_authentication_called_with_credentials() {
    let service_name = unique_service_name();
    let (handler, call_count, last_uid, last_gid) = TrackingHandler::new();
    let _server = start_server(service_name.clone(), handler);

    wait_for_server_ready(&service_name);

    let current_uid = unsafe { libc::getuid() };
    let current_gid = unsafe { libc::getgid() };

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        0,
        "Should not be called yet"
    );

    let connected = try_connect(&service_name);

    assert!(connected, "TrackingHandler should accept connection");
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "authenticate() should be called once"
    );
    assert_eq!(
        last_uid.load(Ordering::SeqCst),
        current_uid,
        "authenticate() should receive correct uid"
    );
    assert_eq!(
        last_gid.load(Ordering::SeqCst),
        current_gid,
        "authenticate() should receive correct gid"
    );
}

#[test]
#[ignore] // Requires libqb-dev
fn test_multiple_connections_call_authenticate_each_time() {
    let service_name = unique_service_name();
    let (handler, call_count, _, _) = TrackingHandler::new();
    let _server = start_server(service_name.clone(), handler);

    wait_for_server_ready(&service_name);

    // First connection
    assert!(try_connect(&service_name));
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Second connection
    assert!(try_connect(&service_name));
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    // Third connection
    assert!(try_connect(&service_name));
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
}

#[test]
#[ignore] // Requires libqb-dev
fn test_read_only_permissions_accepted() {
    let service_name = unique_service_name();
    let _server = start_server(service_name.clone(), ReadOnlyForNonRootHandler);

    wait_for_server_ready(&service_name);

    // Connection should succeed regardless of whether we get ReadOnly or ReadWrite
    // (both are accepted, just with different permissions)
    assert!(
        try_connect(&service_name),
        "ReadOnlyForNonRootHandler should accept connections with appropriate permissions"
    );
}

/// Test that demonstrates the authentication policy is enforced at connection time
#[test]
#[ignore] // Requires libqb-dev
fn test_authentication_enforced_at_connection_time() {
    // This test verifies that authentication happens during connection setup,
    // not during request handling
    let service_name = unique_service_name();
    let _server = start_server(service_name.clone(), RejectAllHandler);

    wait_for_server_ready(&service_name);

    // Connection should fail immediately, before any request is sent
    let start = std::time::Instant::now();
    let connected = try_connect(&service_name);
    let duration = start.elapsed();

    assert!(!connected, "Connection should be rejected");
    assert!(
        duration < Duration::from_millis(100),
        "Rejection should happen quickly during handshake, not during request processing"
    );
}

#[cfg(test)]
mod policy_examples {
    use super::*;

    /// Example: Handler that mimics Proxmox VE authentication policy
    /// - Root (uid=0) gets read-write
    /// - www-data (uid=33) gets read-only (for web UI)
    /// - Others are rejected
    struct ProxmoxStyleHandler;

    #[async_trait]
    impl Handler for ProxmoxStyleHandler {
        fn authenticate(&self, uid: u32, _gid: u32) -> Option<Permissions> {
            match uid {
                0 => Some(Permissions::ReadWrite), // root
                33 => Some(Permissions::ReadOnly), // www-data
                _ => None,                         // reject others
            }
        }

        async fn handle(&self, request: Request) -> Response {
            // In real implementation, would check request.read_only
            // to enforce read-only restrictions
            Response::ok(format!("msg_id {}", request.msg_id).into_bytes())
        }
    }

    #[test]
    #[ignore] // Requires libqb-dev
    fn test_proxmox_style_policy() {
        let service_name = unique_service_name();
        let _server = start_server(service_name.clone(), ProxmoxStyleHandler);

        wait_for_server_ready(&service_name);

        let current_uid = unsafe { libc::getuid() };
        let connected = try_connect(&service_name);

        match current_uid {
            0 => assert!(connected, "Root should be accepted"),
            33 => assert!(connected, "www-data should be accepted"),
            _ => assert!(!connected, "Other users should be rejected"),
        }
    }

    /// Example: Handler that uses group-based authentication
    struct GroupBasedHandler {
        allowed_gid: u32,
    }

    impl GroupBasedHandler {
        fn new(allowed_gid: u32) -> Self {
            Self { allowed_gid }
        }
    }

    #[async_trait]
    impl Handler for GroupBasedHandler {
        fn authenticate(&self, _uid: u32, gid: u32) -> Option<Permissions> {
            if gid == self.allowed_gid {
                Some(Permissions::ReadWrite)
            } else {
                None
            }
        }

        async fn handle(&self, _request: Request) -> Response {
            Response::ok(b"ok".to_vec())
        }
    }

    #[test]
    #[ignore] // Requires libqb-dev
    fn test_group_based_authentication() {
        let service_name = unique_service_name();
        let current_gid = unsafe { libc::getgid() };
        let _server = start_server(service_name.clone(), GroupBasedHandler::new(current_gid));

        wait_for_server_ready(&service_name);

        assert!(
            try_connect(&service_name),
            "Should accept connection from same group"
        );
    }
}
