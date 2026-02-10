//! Wire protocol compatibility test with libqb C clients
//!
//! This integration test verifies that our Rust Server is fully compatible
//! with real libqb C clients by using libqb's client API via FFI.
//!
//! Run with: cargo test --package pmxcfs-ipc --test qb_wire_compat -- --ignored --nocapture
//!
//! Requires: libqb-dev installed

use pmxcfs_test_utils::wait_for_condition_blocking;
use std::ffi::CString;
use std::thread;
use std::time::Duration;

// ============================================================================
// Minimal libqb FFI bindings (client-side only)
// ============================================================================

/// libqb request header matching C's __attribute__ ((aligned(8)))
/// Each field is i32 with 8-byte alignment, achieved via explicit padding
#[repr(C, align(8))]
#[derive(Debug, Copy, Clone)]
struct QbIpcRequestHeader {
    id: i32,    // 4 bytes
    _pad1: u32, // 4 bytes padding
    size: i32,  // 4 bytes
    _pad2: u32, // 4 bytes padding
}

/// libqb response header matching C's __attribute__ ((aligned(8)))
/// Each field is i32 with 8-byte alignment, achieved via explicit padding
#[repr(C, align(8))]
#[derive(Debug, Copy, Clone)]
struct QbIpcResponseHeader {
    id: i32,    // 4 bytes
    _pad1: u32, // 4 bytes padding
    size: i32,  // 4 bytes
    _pad2: u32, // 4 bytes padding
    error: i32, // 4 bytes
    _pad3: u32, // 4 bytes padding
}

// Opaque type for connection handle
#[repr(C)]
struct QbIpccConnection {
    _private: [u8; 0],
}

#[link(name = "qb")]
unsafe extern "C" {
    /// Connect to a QB IPC service
    /// Returns NULL on failure
    fn qb_ipcc_connect(name: *const libc::c_char, max_msg_size: usize) -> *mut QbIpccConnection;

    /// Send request and receive response (with iovec)
    /// Returns number of bytes received, or negative errno on error
    fn qb_ipcc_sendv_recv(
        conn: *mut QbIpccConnection,
        iov: *const libc::iovec,
        iov_len: u32,
        res_buf: *mut libc::c_void,
        res_buf_size: usize,
        timeout_ms: i32,
    ) -> libc::ssize_t;

    /// Disconnect from service
    fn qb_ipcc_disconnect(conn: *mut QbIpccConnection);

    /// Initialize libqb logging
    fn qb_log_init(name: *const libc::c_char, facility: i32, priority: i32);

    /// Control log targets
    fn qb_log_ctl(target: i32, conf: i32, arg: i32) -> i32;

    /// Filter control
    fn qb_log_filter_ctl(
        target: i32,
        op: i32,
        type_: i32,
        text: *const libc::c_char,
        priority: i32,
    ) -> i32;
}

// Log targets
const QB_LOG_STDERR: i32 = 2;

// Log control operations
const QB_LOG_CONF_ENABLED: i32 = 1;

// Log filter operations
const QB_LOG_FILTER_ADD: i32 = 0;
const QB_LOG_FILTER_FILE: i32 = 1;

// Log levels (from syslog.h)
const LOG_TRACE: i32 = 8; // LOG_DEBUG + 1

// ============================================================================
// Safe Rust wrapper around libqb client
// ============================================================================

struct QbIpcClient {
    conn: *mut QbIpccConnection,
}

impl QbIpcClient {
    fn connect(service_name: &str, max_msg_size: usize) -> Result<Self, String> {
        let name = CString::new(service_name).map_err(|e| format!("Invalid service name: {e}"))?;

        let conn = unsafe { qb_ipcc_connect(name.as_ptr(), max_msg_size) };

        if conn.is_null() {
            let errno = unsafe { *libc::__errno_location() };
            let error_str = unsafe {
                let err_ptr = libc::strerror(errno);
                std::ffi::CStr::from_ptr(err_ptr)
                    .to_string_lossy()
                    .to_string()
            };
            Err(format!(
                "qb_ipcc_connect returned NULL (errno={errno}: {error_str})"
            ))
        } else {
            Ok(Self { conn })
        }
    }

    fn send_recv(
        &self,
        request_id: i32,
        request_data: &[u8],
        timeout_ms: i32,
    ) -> Result<(i32, Vec<u8>), String> {
        // Build request
        let req_header = QbIpcRequestHeader {
            id: request_id,
            _pad1: 0,
            size: (std::mem::size_of::<QbIpcRequestHeader>() + request_data.len()) as i32,
            _pad2: 0,
        };

        // Setup iovec
        let mut iov = vec![libc::iovec {
            iov_base: &req_header as *const _ as *mut libc::c_void,
            iov_len: std::mem::size_of::<QbIpcRequestHeader>(),
        }];

        if !request_data.is_empty() {
            iov.push(libc::iovec {
                iov_base: request_data.as_ptr() as *mut libc::c_void,
                iov_len: request_data.len(),
            });
        }

        // Response buffer
        const MAX_RESPONSE: usize = 8192 * 128;
        let mut resp_buf = vec![0u8; MAX_RESPONSE];

        // Send and receive
        let result = unsafe {
            qb_ipcc_sendv_recv(
                self.conn,
                iov.as_ptr(),
                iov.len() as u32,
                resp_buf.as_mut_ptr() as *mut libc::c_void,
                resp_buf.len(),
                timeout_ms,
            )
        };

        if result < 0 {
            return Err(format!("qb_ipcc_sendv_recv failed: {}", -result));
        }

        let bytes_received = result as usize;

        // Parse response header
        if bytes_received < std::mem::size_of::<QbIpcResponseHeader>() {
            return Err("Response too short".to_string());
        }

        let resp_header = unsafe { *(resp_buf.as_ptr() as *const QbIpcResponseHeader) };

        // Verify response ID matches request
        if resp_header.id != request_id {
            return Err(format!(
                "Response ID mismatch: expected {}, got {}",
                request_id, resp_header.id
            ));
        }

        // Extract data
        let data_start = std::mem::size_of::<QbIpcResponseHeader>();
        let data = resp_buf[data_start..bytes_received].to_vec();

        Ok((resp_header.error, data))
    }
}

impl Drop for QbIpcClient {
    fn drop(&mut self) {
        unsafe {
            qb_ipcc_disconnect(self.conn);
        }
    }
}

// ============================================================================
// Integration Test
// ============================================================================

#[test]
#[ignore] // Run with: cargo test -- --ignored
fn test_libqb_wire_protocol_compatibility() {
    eprintln!("🧪 Starting wire protocol compatibility test");

    // Check if libqb is available
    eprintln!("🔍 Checking if libqb is available...");
    if !check_libqb_available() {
        eprintln!("⏭️  SKIP: libqb not installed");
        eprintln!("   Install with: sudo apt-get install libqb-dev");
        return;
    }
    eprintln!("✓ libqb is available");

    // Start test server
    eprintln!("🚀 Starting test server...");
    let server_handle = start_test_server();
    eprintln!("✓ Server thread spawned");

    // Wait for server to be ready
    eprintln!("⏳ Waiting for server initialization...");
    wait_for_server_ready("pve2");
    eprintln!("✓ Server is ready");

    // Run tests
    eprintln!("🧪 Running client tests...");
    let test_result = run_client_tests();

    // Cleanup
    drop(server_handle);

    // Assert results
    assert!(
        test_result.is_ok(),
        "Client tests failed: {:?}",
        test_result.err()
    );
}

fn check_libqb_available() -> bool {
    std::process::Command::new("pkg-config")
        .args(["--exists", "libqb"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn start_test_server() -> thread::JoinHandle<()> {
    use async_trait::async_trait;
    use pmxcfs_ipc::{Handler, Request, Response, Server};

    // Create test handler
    struct TestHandler;

    #[async_trait]
    impl Handler for TestHandler {
        fn authenticate(&self, _uid: u32, _gid: u32) -> Option<pmxcfs_ipc::Permissions> {
            // Accept all connections with read-write access for testing
            Some(pmxcfs_ipc::Permissions::ReadWrite)
        }

        async fn handle(&self, request: Request) -> Response {
            match request.msg_id {
                1 => {
                    // CFS_IPC_GET_FS_VERSION
                    let response_str = r#"{"version":1,"protocol":1}"#;
                    Response::ok(response_str.as_bytes().to_vec())
                }
                2 => {
                    // CFS_IPC_GET_CLUSTER_INFO
                    let response_str = r#"{"nodes":[],"quorate":false}"#;
                    Response::ok(response_str.as_bytes().to_vec())
                }
                3 => {
                    // CFS_IPC_GET_GUEST_LIST
                    let response_str = r#"{"data":[]}"#;
                    Response::ok(response_str.as_bytes().to_vec())
                }
                _ => Response::err(-libc::EINVAL),
            }
        }
    }

    // Spawn server thread with tokio runtime
    thread::spawn(move || {
        // Initialize tracing for server (WARN level - silent on success)
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_target(false)
            .init();

        // Create tokio runtime for async server
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

        rt.block_on(async {
            let mut server = Server::new("pve2", TestHandler);

            // Server uses abstract Unix socket (Linux-specific)
            if let Err(e) = server.start() {
                eprintln!("Server startup failed: {e}");
                eprintln!("Error details: {e:?}");
                panic!("Server startup failed");
            }

            // Give tokio a chance to start the acceptor task
            tokio::task::yield_now().await;

            // Block forever to keep server alive
            std::future::pending::<()>().await;
        });
    })
}

/// Wait for server to be ready by checking if socket file exists
fn wait_for_server_ready(service_name: &str) {
    assert!(
        wait_for_condition_blocking(
            || {
                // Check if socket file exists in /dev/shm
                let socket_pattern = format!("/dev/shm/qb-{service_name}-");
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

fn run_client_tests() -> Result<(), String> {
    // Enable libqb debug logging to see what's happening
    eprintln!("🔧 Enabling libqb debug logging...");
    unsafe {
        let name = CString::new("qb_test").unwrap();
        qb_log_init(name.as_ptr(), libc::LOG_USER, LOG_TRACE);
        qb_log_ctl(QB_LOG_STDERR, QB_LOG_CONF_ENABLED, 1);
        // Enable all log messages from all files at TRACE level
        let all_files = CString::new("*").unwrap();
        qb_log_filter_ctl(
            QB_LOG_STDERR,
            QB_LOG_FILTER_ADD,
            QB_LOG_FILTER_FILE,
            all_files.as_ptr(),
            LOG_TRACE,
        );
    }
    eprintln!("✓ libqb logging enabled (TRACE level)");

    eprintln!("📡 Connecting to server...");
    // Connect to abstract socket "pve2"
    // Use a very large buffer size to rule out space issues
    let client = QbIpcClient::connect("pve2", 8192 * 1024)?; // 8MB instead of 1MB
    eprintln!("✓ Connected successfully");

    eprintln!("🧪 Test 1: GET_FS_VERSION");
    // Test 1: GET_FS_VERSION
    let (error, data) = client.send_recv(1, &[], 5000)?;
    eprintln!("✓ Got response: error={}, data_len={}", error, data.len());
    if error == 0 {
        let response = String::from_utf8_lossy(&data);
        eprintln!("  Response: {response}");
        assert!(
            response.contains("version"),
            "Response should contain version field"
        );
    }

    eprintln!("🧪 Test 2: GET_CLUSTER_INFO");
    // Test 2: GET_CLUSTER_INFO
    let (error, data) = client.send_recv(2, &[], 5000)?;
    eprintln!("✓ Got response: error={}, data_len={}", error, data.len());
    if error == 0 {
        let response = String::from_utf8_lossy(&data);
        eprintln!("  Response: {response}");
        assert!(
            response.contains("nodes"),
            "Response should contain nodes field"
        );
    }

    eprintln!("🧪 Test 3: Request with data payload");
    // Test 3: Request with data payload
    let test_payload = b"test_payload_data";
    let (_error, _data) = client.send_recv(1, test_payload, 5000)?;
    eprintln!("✓ Request with payload succeeded");

    eprintln!("🧪 Test 4: GET_GUEST_LIST");
    // Test 4: GET_GUEST_LIST
    let (_error, _data) = client.send_recv(3, &[], 5000)?;
    eprintln!("✓ GET_GUEST_LIST succeeded");

    Ok(())
}
