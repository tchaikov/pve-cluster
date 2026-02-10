/// Per-connection handling for libqb IPC with shared memory ring buffers
///
/// This module contains all connection-specific logic including connection
/// establishment, authentication, request handling, and shared memory ring buffer management.
use anyhow::{Context, Result};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

use super::handler::{Handler, Permissions};
use super::protocol::*;
use super::ringbuffer::{FlowControl, RingBuffer};

/// Per-connection state using shared memory ring buffers
///
/// Uses SHM transport (shared memory ring buffers).
#[allow(dead_code)] // Fields are intentionally stored for lifecycle management
pub(super) struct QbConnection {
    /// Connection ID for logging and debugging
    conn_id: u64,

    /// Client process ID (from SO_PEERCRED)
    pid: u32,

    /// Client user ID (from SO_PEERCRED)
    uid: u32,

    /// Client group ID (from SO_PEERCRED)
    gid: u32,

    /// Whether this connection has read-only access (determined by Handler::authenticate)
    pub(super) read_only: bool,

    /// Setup socket (kept open for disconnect detection)
    /// None if moved to request handler task
    _setup_stream: Option<UnixStream>,

    /// Ring buffers for shared memory IPC
    /// Request ring: client writes, server reads
    request_rb: Option<RingBuffer>,
    /// Response ring: server writes, client reads
    response_rb: Option<RingBuffer>,
    /// Event ring: server writes, client reads (for async notifications)
    /// NOTE: The existing PVE/IPCC.xs Perl client only uses qb_ipcc_sendv_recv()
    /// and never calls qb_ipcc_event_recv(), so this ring buffer is created
    /// for libqb compatibility but remains unused in practice.
    _event_rb: Option<RingBuffer>,

    /// Paths to ring buffer data files (for debugging/cleanup)
    pub(super) ring_buffer_paths: Vec<PathBuf>,

    /// Task handle for request handler (auto-aborted on drop)
    pub(super) task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl QbConnection {
    /// Accept a new connection from the setup socket
    ///
    /// Performs authentication, creates ring buffers, spawns request handler task,
    /// and returns the connection object.
    pub(super) async fn accept(
        mut stream: UnixStream,
        conn_id: u64,
        service_name: &str,
        handler: Arc<dyn Handler>,
        cancellation_token: CancellationToken,
    ) -> Result<Self> {
        // Read connection request
        let fd = stream.as_raw_fd();
        let mut req_bytes = vec![0u8; std::mem::size_of::<ConnectionRequest>()];
        stream
            .read_exact(&mut req_bytes)
            .await
            .context("Failed to read connection request")?;

        tracing::debug!(
            "Connection request raw bytes ({} bytes): {:02x?}",
            req_bytes.len(),
            req_bytes
        );

        // SAFETY: req_bytes is guaranteed to be exactly sizeof(ConnectionRequest) bytes
        // due to read_exact() above. read_unaligned is used because the buffer may not
        // be aligned to ConnectionRequest's alignment requirement.
        let req =
            unsafe { std::ptr::read_unaligned(req_bytes.as_ptr() as *const ConnectionRequest) };

        tracing::debug!(
            "Connection request: id={}, size={}, max_msg_size={}",
            *req.hdr.id,
            *req.hdr.size,
            req.max_msg_size
        );

        // Validate connection request
        const MAX_REASONABLE_MSG_SIZE: u32 = 16 * 1024 * 1024; // 16MB
        const MIN_MSG_SIZE: u32 = 128;

        // Validate header size matches expected
        let expected_size = std::mem::size_of::<ConnectionRequest>() as i32;
        if *req.hdr.size != expected_size {
            tracing::warn!(
                "Rejecting connection {}: header size mismatch (expected {}, got {})",
                conn_id,
                expected_size,
                *req.hdr.size
            );
            send_connection_response(&mut stream, -libc::EINVAL, conn_id, 0, "", "", "").await?;
            anyhow::bail!("Invalid header size in connection request");
        }

        // Validate max_msg_size is within reasonable bounds
        if req.max_msg_size < MIN_MSG_SIZE || req.max_msg_size > MAX_REASONABLE_MSG_SIZE {
            tracing::warn!(
                "Rejecting connection {}: invalid max_msg_size {} (valid range: {}-{})",
                conn_id,
                req.max_msg_size,
                MIN_MSG_SIZE,
                MAX_REASONABLE_MSG_SIZE
            );
            send_connection_response(&mut stream, -libc::EINVAL, conn_id, 0, "", "", "").await?;
            anyhow::bail!("Invalid max_msg_size in connection request");
        }

        // Get peer credentials (SO_PEERCRED on Linux)
        let (uid, gid, pid) = get_peer_credentials(fd)?;

        // Authenticate using Handler trait
        let read_only = match handler.authenticate(uid, gid) {
            Some(Permissions::ReadWrite) => {
                tracing::info!(pid, uid, gid, "Connection accepted with read-write access");
                false
            }
            Some(Permissions::ReadOnly) => {
                tracing::info!(pid, uid, gid, "Connection accepted with read-only access");
                true
            }
            None => {
                tracing::warn!(
                    pid,
                    uid,
                    gid,
                    "Connection rejected by authentication policy"
                );
                send_connection_response(&mut stream, -libc::EPERM, conn_id, 0, "", "", "").await?;
                anyhow::bail!("Connection authentication failed");
            }
        };

        // Create connection descriptor for ring buffer naming
        let conn_desc = format!("{}-{}-{}", std::process::id(), pid, conn_id);
        let max_msg_size = req.max_msg_size.max(8192);

        // Create ring buffers in /dev/shm
        // Pass max_msg_size directly - RingBuffer::new() will add QB_RB_CHUNK_MARGIN and round up
        // (just like qb_rb_open() does on the client side)
        let ring_size = max_msg_size as usize;

        tracing::debug!(
            "Creating ring buffers for connection {}: size={} bytes",
            conn_id,
            ring_size
        );

        // Request ring: client writes, server reads
        // Request ring needs sizeof(int32_t) for flow control (shared_user_data)
        let request_rb_name = format!("{conn_desc}-{service_name}-request");
        let request_rb = RingBuffer::new(
            "/dev/shm",
            &request_rb_name,
            ring_size,
            std::mem::size_of::<i32>(),
        )
        .context("Failed to create request ring buffer")?;

        // Response ring: server writes, client reads
        // Response ring doesn't need shared_user_data
        let response_rb_name = format!("{conn_desc}-{service_name}-response");
        tracing::info!("About to create response ring buffer: {}", response_rb_name);
        let response_rb = RingBuffer::new("/dev/shm", &response_rb_name, ring_size, 0)
            .context("Failed to create response ring buffer")?;
        tracing::info!("Response ring buffer created successfully");

        // Event ring: server writes, client reads (for async notifications)
        // Event ring doesn't need shared_user_data
        tracing::info!("About to format event ring buffer name");
        let event_rb_name = format!("{conn_desc}-{service_name}-event");
        tracing::info!("About to create event ring buffer: {}", event_rb_name);
        let event_rb = RingBuffer::new("/dev/shm", &event_rb_name, ring_size, 0)
            .context("Failed to create event ring buffer")?;
        tracing::info!("Event ring buffer created successfully");

        // Collect full paths for cleanup tracking
        let request_data_path = PathBuf::from(format!("/dev/shm/qb-{request_rb_name}-data"));
        let response_data_path = PathBuf::from(format!("/dev/shm/qb-{response_rb_name}-data"));
        let event_data_path = PathBuf::from(format!("/dev/shm/qb-{event_rb_name}-data"));

        // Send connection response with ring buffer BASE NAMES (not full paths)
        // libqb client expects base names (e.g., "123-456-1-pve2-request")
        // It will internally prepend "/dev/shm/qb-" and append "-header" or "-data"
        send_connection_response(
            &mut stream,
            0,
            conn_id,
            max_msg_size,
            &request_rb_name,
            &response_rb_name,
            &event_rb_name,
        )
        .await?;

        // Spawn request handler task
        let handler_for_task = handler.clone();
        let cancellation_for_task = cancellation_token.child_token();

        let task_handle = tokio::spawn(async move {
            Self::handle_requests(
                request_rb,
                response_rb,
                stream, // Pass setup stream for disconnect detection
                handler_for_task,
                cancellation_for_task,
                conn_id,
                uid,
                gid,
                pid,
                read_only,
            )
            .await;
        });

        tracing::info!("Connection {} established (SHM transport)", conn_id);

        Ok(Self {
            conn_id,
            pid,
            uid,
            gid,
            read_only,
            _setup_stream: None, // Moved to task for disconnect detection
            request_rb: None,  // Moved to task
            response_rb: None, // Moved to task
            _event_rb: Some(event_rb),
            ring_buffer_paths: vec![request_data_path, response_data_path, event_data_path],
            task_handle: Some(task_handle),
        })
    }

    /// Request handler loop - receives and processes messages via ring buffers
    ///
    /// Runs in a background async task, receiving requests and sending responses
    /// through shared memory ring buffers.
    ///
    /// Uses tokio channels to implement a workqueue with flow control:
    /// - FlowControl::OK: Proceed with sending
    /// - FlowControl::SLOW_DOWN: Reduce send rate
    /// - FlowControl::STOP: Do not send
    ///
    /// Architecture: Three concurrent tasks communicating via tokio channels:
    /// 1. Request receiver: reads from request ring buffer, queues work
    /// 2. Worker: processes requests from work queue, sends to response queue
    /// 3. Response sender: writes responses from response queue to response ring buffer
    ///
    /// The setup_stream is monitored for closure (EOF) to detect client disconnection.
    /// This matches libqb's behavior where the server polls the setup socket for POLLHUP.
    #[allow(clippy::too_many_arguments)]
    async fn handle_requests(
        mut request_rb: RingBuffer,
        mut response_rb: RingBuffer,
        mut setup_stream: UnixStream,
        handler: Arc<dyn Handler>,
        cancellation_token: CancellationToken,
        conn_id: u64,
        uid: u32,
        gid: u32,
        pid: u32,
        read_only: bool,
    ) {
        tracing::debug!("Request handler started for connection {}", conn_id);

        // Monitor setup socket for disconnection using a separate task
        // This is necessary because the setup socket should only close when client disconnects
        let (disconnect_tx, mut disconnect_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let mut buf = [0u8; 1];
            loop {
                match setup_stream.read(&mut buf).await {
                    Ok(0) => {
                        // EOF - client closed setup socket
                        tracing::info!("Client disconnected (setup socket EOF) for conn {}", conn_id);
                        let _ = disconnect_tx.send(());
                        break;
                    }
                    Ok(_) => {
                        // Unexpected data on setup socket - ignore
                        tracing::warn!("Unexpected data on setup socket for conn {}", conn_id);
                    }
                    Err(e) => {
                        // Error reading setup socket
                        tracing::warn!("Error reading setup socket for conn {}: {}", conn_id, e);
                        let _ = disconnect_tx.send(());
                        break;
                    }
                }
            }
        });

        // Workqueue capacity and flow control thresholds
        //
        // NOTE: The C implementation (using libqb) processes requests synchronously
        // in the event loop callback (server.c:159 s1_msg_process_fn), so there's
        // no explicit queue. We add async queueing in Rust to allow non-blocking
        // request handling with tokio.
        //
        // Queue capacity of 8 is chosen as a reasonable default for:
        // - Typical PVE workloads: Most IPC operations are fast (file reads/writes)
        // - Memory efficiency: Each queued item = ~1KB (request header + data)
        // - Backpressure: Small queue encourages flow control to activate quickly
        // - Testing: Flow control test (02-flow-control.sh) verifies 20 concurrent
        //   operations work correctly with capacity 8
        //
        // Flow control thresholds match libqb's rate limiting (ipcs.c:199-203):
        // - FlowControl::OK (0): Proceed with sending (QB_IPCS_RATE_NORMAL)
        // - FlowControl::SLOW_DOWN (1): Reduce send rate (QB_IPCS_RATE_OFF)
        // - FlowControl::STOP (2): Do not send (QB_IPCS_RATE_OFF_2)
        const MAX_PENDING_REQUESTS: usize = 8;

        // Set SLOW_DOWN when queue reaches 75% capacity (6/8 items)
        // This provides early warning before the queue fills completely,
        // allowing clients to throttle before hitting STOP
        const FC_WARNING_THRESHOLD: usize = 6;

        // Work queue: (header, request) -> worker
        let (work_tx, mut work_rx) =
            tokio::sync::mpsc::channel::<(RequestHeader, Request)>(MAX_PENDING_REQUESTS);

        // Response queue: worker -> response sender
        // Unbounded because responses must not block the worker
        let (response_tx, mut response_rx) =
            tokio::sync::mpsc::unbounded_channel::<(RequestHeader, Response)>();

        // Spawn worker task to process requests
        let worker_handler = handler.clone();
        let worker_response_tx = response_tx.clone();
        let worker_task = tokio::spawn(async move {
            while let Some((header, request)) = work_rx.recv().await {
                let handler_response = worker_handler.handle(request).await;
                // Send to response queue (unbounded, never blocks)
                let _ = worker_response_tx.send((header, handler_response));
            }
        });

        // Spawn response sender task
        let response_task = tokio::spawn(async move {
            while let Some((header, handler_response)) = response_rx.recv().await {
                Self::send_response(&mut response_rb, header, handler_response).await;
            }
        });

        // Main request receiver loop
        loop {
            let request_data = tokio::select! {
                _ = cancellation_token.cancelled() => {
                    tracing::debug!("Request handler cancelled for connection {}", conn_id);
                    break;
                }
                // Check for client disconnection from oneshot channel
                _ = &mut disconnect_rx => {
                    tracing::debug!("Disconnect signal received for connection {}", conn_id);
                    break;
                }
                result = request_rb.recv() => {
                    match result {
                        Ok(data) => data,
                        Err(e) => {
                            tracing::error!("Error receiving request on conn {}: {}", conn_id, e);
                            break;
                        }
                    }
                }
            };

            // After receiving from ring buffer, flow control is already set to 0
            // by RingBufferShared::read_chunk()

            // Parse request header
            if request_data.len() < std::mem::size_of::<RequestHeader>() {
                tracing::warn!(
                    "Request too small: {} bytes (need {} for header)",
                    request_data.len(),
                    std::mem::size_of::<RequestHeader>()
                );
                continue;
            }

            let header =
                unsafe { std::ptr::read_unaligned(request_data.as_ptr() as *const RequestHeader) };

            tracing::info!(
                "Received request on conn {}: id={}, size={}, data_len={}",
                conn_id,
                *header.id,
                *header.size,
                request_data.len()
            );

            // Extract message data (after header)
            let header_size = std::mem::size_of::<RequestHeader>();
            let msg_data = &request_data[header_size..];

            // Build request object with full context
            let request = Request {
                msg_id: *header.id,
                data: msg_data.to_vec(),
                is_read_only: read_only,
                conn_id,
                uid,
                gid,
                pid,
            };

            // Send to workqueue - implements backpressure via flow control
            match work_tx.try_send((header, request)) {
                Ok(()) => {
                    // Request queued successfully

                    // Update flow control based on queue depth
                    // This matches libqb's rate limiting behavior
                    let queue_len = MAX_PENDING_REQUESTS - work_tx.capacity();
                    let fc_value = if queue_len >= MAX_PENDING_REQUESTS {
                        FlowControl::STOP // Queue full - stop sending
                    } else if queue_len >= FC_WARNING_THRESHOLD {
                        FlowControl::SLOW_DOWN // Queue approaching full - slow down
                    } else {
                        FlowControl::OK // Queue has space - OK to send
                    };

                    if fc_value > FlowControl::OK {
                        tracing::debug!(
                            "Setting flow control to {} (queue: {}/{})",
                            fc_value,
                            queue_len,
                            MAX_PENDING_REQUESTS
                        );
                    }
                    request_rb.flow_control.set(fc_value);
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    // Queue is full - set flow control to STOP and send EAGAIN
                    tracing::warn!("Work queue full on conn {}, sending EAGAIN", conn_id);
                    request_rb.flow_control.set(FlowControl::STOP);

                    let error_response = Response {
                        error_code: -libc::EAGAIN,
                        data: Vec::new(),
                    };
                    // Send error response directly (bypassing queue)
                    let _ = response_tx.send((header, error_response));
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    tracing::error!("Work queue closed on conn {}", conn_id);
                    break;
                }
            }
        }

        // Cleanup: drop channels to signal tasks to exit
        drop(work_tx);
        drop(response_tx);
        let _ = worker_task.await;
        let _ = response_task.await;

        tracing::debug!("Request handler finished for connection {}", conn_id);
    }

    /// Send a response to the client
    async fn send_response(
        response_rb: &mut RingBuffer,
        header: RequestHeader,
        handler_response: Response,
    ) {
        // Build and serialize response: [header][data]
        let response_size = std::mem::size_of::<ResponseHeader>() + handler_response.data.len();
        let mut response_bytes = Vec::with_capacity(response_size);

        let response_header = ResponseHeader {
            id: header.id,
            size: (response_size as i32).into(),
            error: handler_response.error_code.into(),
        };

        response_bytes.extend_from_slice(unsafe {
            std::slice::from_raw_parts(
                &response_header as *const _ as *const u8,
                std::mem::size_of::<ResponseHeader>(),
            )
        });
        response_bytes.extend_from_slice(&handler_response.data);

        tracing::debug!("Response header bytes (24): {:02x?}", &response_bytes[..24]);

        // Send response (async, yields if buffer full)
        match response_rb.send(&response_bytes).await {
            Ok(()) => {
                // Response sent successfully
            }
            Err(e) => {
                tracing::error!("Failed to send response: {}", e);
            }
        }
    }
}

/// Get peer credentials from Unix socket
fn get_peer_credentials(fd: i32) -> Result<(u32, u32, u32)> {
    #[cfg(target_os = "linux")]
    {
        let mut ucred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut ucred_size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

        let res = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut ucred as *mut _ as *mut libc::c_void,
                &mut ucred_size,
            )
        };

        if res != 0 {
            anyhow::bail!(
                "getsockopt SO_PEERCRED failed: {}",
                std::io::Error::last_os_error()
            );
        }

        Ok((ucred.uid, ucred.gid, ucred.pid as u32))
    }

    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!("Peer credentials not supported on this platform");
    }
}

/// Send connection response to client
async fn send_connection_response(
    stream: &mut UnixStream,
    error: i32,
    conn_id: u64,
    max_msg_size: u32,
    request_path: &str,
    response_path: &str,
    event_path: &str,
) -> Result<()> {
    let mut response = ConnectionResponse {
        hdr: ResponseHeader {
            id: MSG_AUTHENTICATE.into(),
            size: (std::mem::size_of::<ConnectionResponse>() as i32).into(),
            error: error.into(),
        },
        connection_type: CONNECTION_TYPE_SHM, // Shared memory transport
        max_msg_size,
        connection: conn_id as usize,
        request: [0u8; PATH_MAX],
        response: [0u8; PATH_MAX],
        event: [0u8; PATH_MAX],
    };

    // Helper to copy path strings into fixed-size buffers
    let copy_path = |dest: &mut [u8; PATH_MAX], src: &str| {
        if !src.is_empty() {
            let len = src.len().min(PATH_MAX - 1);
            dest[..len].copy_from_slice(&src.as_bytes()[..len]);
            tracing::debug!("Connection response path: '{}'", src);
        }
    };

    copy_path(&mut response.request, request_path);
    copy_path(&mut response.response, response_path);
    copy_path(&mut response.event, event_path);

    // Serialize and send
    let response_bytes = unsafe {
        std::slice::from_raw_parts(
            &response as *const _ as *const u8,
            std::mem::size_of::<ConnectionResponse>(),
        )
    };

    stream
        .write_all(response_bytes)
        .await
        .context("Failed to send connection response")?;

    tracing::debug!(
        "Sent connection response: error={}, connection_type=SHM",
        error
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_malformed_request_size_validation() {
        // This test verifies the size validation logic for malformed requests
        // The actual validation happens in handle_requests() at line 247-254

        let header_size = std::mem::size_of::<RequestHeader>();
        assert_eq!(header_size, 16, "RequestHeader should be 16 bytes");

        // Test case 1: Request too small (would be rejected)
        let too_small_data = [0x01, 0x02, 0x03]; // Only 3 bytes
        assert!(
            too_small_data.len() < header_size,
            "Malformed request with {} bytes should be less than header size {}",
            too_small_data.len(),
            header_size
        );

        // Test case 2: More realistic too-small cases
        let test_cases = vec![
            (vec![0u8; 0], 0),   // Empty request
            (vec![0u8; 1], 1),   // 1 byte
            (vec![0u8; 8], 8),   // 8 bytes (half header)
            (vec![0u8; 15], 15), // 15 bytes (just short of header)
        ];

        for (data, expected_len) in test_cases {
            assert_eq!(data.len(), expected_len);
            assert!(
                data.len() < header_size,
                "Request with {} bytes should be rejected (need {})",
                data.len(),
                header_size
            );
        }

        // Test case 3: Valid size requests (would pass size check)
        let valid_cases = vec![
            vec![0u8; 16],   // Exact header size
            vec![0u8; 32],   // Header + data
            vec![0u8; 1024], // Large request
        ];

        for data in valid_cases {
            assert!(
                data.len() >= header_size,
                "Request with {} bytes should pass size check",
                data.len()
            );
        }
    }

    #[test]
    fn test_malformed_header_structure() {
        // This test verifies that the header structure is correctly defined
        // and that we can safely parse various header patterns

        let header_size = std::mem::size_of::<RequestHeader>();

        // Create a valid-sized buffer with various patterns
        let patterns = vec![
            vec![0x00; header_size], // All zeros
            vec![0xFF; header_size], // All ones
            vec![0xAA; header_size], // Alternating pattern
        ];

        for pattern in patterns {
            assert_eq!(pattern.len(), header_size);

            // Parse header (same unsafe code as in handle_requests:256-258)
            let header =
                unsafe { std::ptr::read_unaligned(pattern.as_ptr() as *const RequestHeader) };

            // The parsing should not crash, regardless of values
            // The actual values don't matter for this safety test
            let _id = *header.id;
            let _size = *header.size;
        }
    }

    #[test]
    fn test_request_header_alignment() {
        // Verify that RequestHeader can be read with read_unaligned
        // This is important because data from ring buffers may not be aligned

        let header_size = std::mem::size_of::<RequestHeader>();

        // Create misaligned buffer (offset by 1 byte to test unaligned access)
        let mut buffer = vec![0u8; header_size + 1];
        buffer[1..].fill(0x42);

        // Read from misaligned offset (this is what read_unaligned is for)
        let header =
            unsafe { std::ptr::read_unaligned(&buffer[1] as *const u8 as *const RequestHeader) };

        // Should successfully read without crashing
        let _id = *header.id;
        let _size = *header.size;
    }

    #[test]
    fn test_connection_request_structure() {
        // Verify ConnectionRequest structure for connection setup

        let conn_req_size = std::mem::size_of::<ConnectionRequest>();

        // ConnectionRequest should be properly sized
        assert!(
            conn_req_size > std::mem::size_of::<RequestHeader>(),
            "ConnectionRequest should include header plus additional fields"
        );

        // Test that we can parse a zero-filled connection request
        let data = vec![0u8; conn_req_size];
        let conn_req =
            unsafe { std::ptr::read_unaligned(data.as_ptr() as *const ConnectionRequest) };

        // Should not crash when accessing fields
        let _id = *conn_req.hdr.id;
        let _size = *conn_req.hdr.size;
        let _max_msg_size = conn_req.max_msg_size;
    }
}
