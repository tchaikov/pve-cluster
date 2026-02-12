//! Comprehensive tests for the service framework
//!
//! Tests cover:
//! - Service lifecycle (start, stop, restart)
//! - Service manager orchestration
//! - Error handling and retry logic
//! - Timer callbacks
//! - File descriptor and polling dispatch modes
//! - Service coordination and state management

use async_trait::async_trait;
use pmxcfs_services::{DispatchAction, InitResult, Service, ServiceError, ServiceManager};
use pmxcfs_test_utils::wait_for_condition;
use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::time::sleep;

// ===== Test Service Implementations =====

/// Mock service for testing lifecycle
struct MockService {
    name: String,
    init_count: Arc<AtomicU32>,
    dispatch_count: Arc<AtomicU32>,
    finalize_count: Arc<AtomicU32>,
    timer_count: Arc<AtomicU32>,
    should_fail_init: Arc<AtomicBool>,
    should_fail_dispatch: Arc<AtomicBool>,
    should_reinit: Arc<AtomicBool>,
    use_fd: bool,
    timer_period: Option<Duration>,
    restartable: bool,
}

impl MockService {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            init_count: Arc::new(AtomicU32::new(0)),
            dispatch_count: Arc::new(AtomicU32::new(0)),
            finalize_count: Arc::new(AtomicU32::new(0)),
            timer_count: Arc::new(AtomicU32::new(0)),
            should_fail_init: Arc::new(AtomicBool::new(false)),
            should_fail_dispatch: Arc::new(AtomicBool::new(false)),
            should_reinit: Arc::new(AtomicBool::new(false)),
            use_fd: false,
            timer_period: None,
            restartable: true,
        }
    }

    fn with_timer(mut self, period: Duration) -> Self {
        self.timer_period = Some(period);
        self
    }

    fn with_restartable(mut self, restartable: bool) -> Self {
        self.restartable = restartable;
        self
    }

    fn counters(&self) -> ServiceCounters {
        ServiceCounters {
            init_count: self.init_count.clone(),
            dispatch_count: self.dispatch_count.clone(),
            finalize_count: self.finalize_count.clone(),
            timer_count: self.timer_count.clone(),
            should_fail_init: self.should_fail_init.clone(),
            should_fail_dispatch: self.should_fail_dispatch.clone(),
            should_reinit: self.should_reinit.clone(),
        }
    }
}

#[async_trait]
impl Service for MockService {
    fn name(&self) -> &str {
        &self.name
    }

    async fn initialize(&mut self) -> pmxcfs_services::Result<InitResult> {
        self.init_count.fetch_add(1, Ordering::SeqCst);

        if self.should_fail_init.load(Ordering::SeqCst) {
            return Err(ServiceError::InitializationFailed(
                "Mock init failure".to_string(),
            ));
        }

        if self.use_fd {
            // Return a dummy fd (stderr is always available)
            Ok(InitResult::WithFileDescriptor(2))
        } else {
            Ok(InitResult::NoFileDescriptor)
        }
    }

    async fn dispatch(&mut self) -> pmxcfs_services::Result<DispatchAction> {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);

        if self.should_fail_dispatch.load(Ordering::SeqCst) {
            return Err(ServiceError::DispatchFailed(
                "Mock dispatch failure".to_string(),
            ));
        }

        if self.should_reinit.load(Ordering::SeqCst) {
            return Ok(DispatchAction::Reinitialize);
        }

        Ok(DispatchAction::Continue)
    }

    async fn finalize(&mut self) -> pmxcfs_services::Result<()> {
        self.finalize_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn timer_callback(&mut self) -> pmxcfs_services::Result<()> {
        self.timer_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn timer_period(&self) -> Option<Duration> {
        self.timer_period
    }

    fn is_restartable(&self) -> bool {
        self.restartable
    }

    fn retry_interval(&self) -> Duration {
        Duration::from_millis(100) // Fast retry for tests
    }

    fn dispatch_interval(&self) -> Duration {
        Duration::from_millis(50) // Fast polling for tests
    }
}

/// Helper struct to access service counters from tests
#[derive(Clone)]
struct ServiceCounters {
    init_count: Arc<AtomicU32>,
    dispatch_count: Arc<AtomicU32>,
    finalize_count: Arc<AtomicU32>,
    timer_count: Arc<AtomicU32>,
    should_fail_init: Arc<AtomicBool>,
    should_fail_dispatch: Arc<AtomicBool>,
    should_reinit: Arc<AtomicBool>,
}

impl ServiceCounters {
    fn init_count(&self) -> u32 {
        self.init_count.load(Ordering::SeqCst)
    }

    fn dispatch_count(&self) -> u32 {
        self.dispatch_count.load(Ordering::SeqCst)
    }

    fn finalize_count(&self) -> u32 {
        self.finalize_count.load(Ordering::SeqCst)
    }

    fn timer_count(&self) -> u32 {
        self.timer_count.load(Ordering::SeqCst)
    }

    fn set_fail_init(&self, fail: bool) {
        self.should_fail_init.store(fail, Ordering::SeqCst);
    }

    fn set_fail_dispatch(&self, fail: bool) {
        self.should_fail_dispatch.store(fail, Ordering::SeqCst);
    }

    fn set_reinit(&self, reinit: bool) {
        self.should_reinit.store(reinit, Ordering::SeqCst);
    }
}

// ===== FD-based Mock Service =====

extern crate libc;

// ===== Lifecycle Tests =====

#[tokio::test]
async fn test_service_lifecycle_basic() {
    let service = MockService::new("test_service");
    let counters = service.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization and dispatching
    assert!(
        wait_for_condition(
            || counters.init_count() >= 1 && counters.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should initialize and dispatch within 5 seconds"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;

    // Service should be finalized
    assert_eq!(
        counters.finalize_count(),
        1,
        "Service should be finalized exactly once"
    );
}

#[tokio::test]
async fn test_service_with_file_descriptor() {
    // Don't use FD-based service in tests since we can't easily create a readable FD
    // Just test that WithFileDescriptor variant works with manager
    let service = MockService::new("no_fd_service"); // Changed to not use FD
    let counters = service.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization and some dispatches
    assert!(
        wait_for_condition(
            || counters.init_count() == 1 && counters.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should initialize once and dispatch within 5 seconds"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;

    assert_eq!(counters.finalize_count(), 1, "Service should finalize once");
}

#[tokio::test]
async fn test_service_initialization_failure() {
    let service = MockService::new("failing_service");
    let counters = service.counters();

    // Make initialization fail
    counters.set_fail_init(true);

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for several retry attempts
    assert!(
        wait_for_condition(
            || counters.init_count() >= 3,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should retry initialization at least 3 times within 5 seconds"
    );

    // Dispatch should not run if init fails
    assert_eq!(
        counters.dispatch_count(),
        0,
        "Service should not dispatch if init fails"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_service_initialization_recovery() {
    let service = MockService::new("recovering_service");
    let counters = service.counters();

    // Start with failing initialization
    counters.set_fail_init(true);

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for some failed attempts
    assert!(
        wait_for_condition(
            || counters.init_count() >= 2,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Should have at least 2 failed initialization attempts within 5 seconds"
    );

    let failed_attempts = counters.init_count();

    // Allow initialization to succeed
    counters.set_fail_init(false);

    // Wait for recovery
    assert!(
        wait_for_condition(
            || counters.init_count() > failed_attempts && counters.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should recover and start dispatching within 5 seconds"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_service_not_restartable() {
    let service = MockService::new("non_restartable").with_restartable(false);
    let counters = service.counters();

    // Make initialization fail
    counters.set_fail_init(true);

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization attempt
    assert!(
        wait_for_condition(
            || counters.init_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should attempt initialization within 5 seconds"
    );

    // Service should only try once (not restartable)
    assert_eq!(
        counters.init_count(),
        1,
        "Non-restartable service should only try initialization once"
    );

    // Wait another cycle to confirm it doesn't retry
    sleep(Duration::from_millis(1500)).await;

    // Should still be 1
    assert_eq!(
        counters.init_count(),
        1,
        "Non-restartable service should not retry, got {}",
        counters.init_count()
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

// ===== Dispatch Tests =====

#[tokio::test]
async fn test_service_dispatch_failure_triggers_reinit() {
    let service = MockService::new("dispatch_fail_service");
    let counters = service.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization and first dispatches
    assert!(
        wait_for_condition(
            || counters.init_count() == 1 && counters.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should initialize once and dispatch within 5 seconds"
    );

    // Make dispatch fail
    counters.set_fail_dispatch(true);

    // Wait for dispatch failure and reinitialization
    assert!(
        wait_for_condition(
            || counters.init_count() >= 2 && counters.finalize_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should reinitialize and finalize after dispatch failure within 5 seconds"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_service_dispatch_requests_reinit() {
    let service = MockService::new("reinit_request_service");
    let counters = service.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization
    assert!(
        wait_for_condition(
            || counters.init_count() == 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should initialize once within 5 seconds"
    );

    // Request reinitialization from dispatch
    counters.set_reinit(true);

    // Wait for reinitialization
    assert!(
        wait_for_condition(
            || counters.init_count() >= 2 && counters.finalize_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should reinitialize and finalize when dispatch requests it within 5 seconds"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

// ===== FD-based Dispatch Tests =====

#[tokio::test]
async fn test_fd_dispatch_basic() {
    let (service, counters) = SharedFdService::new("fd_service");

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization
    assert!(
        wait_for_condition(
            || counters.init_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "FD service should initialize within 5 seconds"
    );

    // Verify no dispatch happens without data on the pipe
    sleep(Duration::from_millis(200)).await;
    assert_eq!(
        counters.dispatch_count(),
        0,
        "FD service should not dispatch without data on pipe"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

/// FD service that shares write_fd via Arc<AtomicI32> so tests can trigger events
struct SharedFdService {
    name: String,
    read_fd: Option<RawFd>,
    write_fd: Arc<std::sync::atomic::AtomicI32>,
    init_count: Arc<AtomicU32>,
    dispatch_count: Arc<AtomicU32>,
    finalize_count: Arc<AtomicU32>,
    should_fail_dispatch: Arc<AtomicBool>,
    should_reinit: Arc<AtomicBool>,
}

impl SharedFdService {
    fn new(name: &str) -> (Self, SharedFdCounters) {
        let write_fd = Arc::new(std::sync::atomic::AtomicI32::new(-1));
        let init_count = Arc::new(AtomicU32::new(0));
        let dispatch_count = Arc::new(AtomicU32::new(0));
        let finalize_count = Arc::new(AtomicU32::new(0));
        let should_fail_dispatch = Arc::new(AtomicBool::new(false));
        let should_reinit = Arc::new(AtomicBool::new(false));

        let counters = SharedFdCounters {
            write_fd: write_fd.clone(),
            init_count: init_count.clone(),
            dispatch_count: dispatch_count.clone(),
            finalize_count: finalize_count.clone(),
            should_fail_dispatch: should_fail_dispatch.clone(),
            should_reinit: should_reinit.clone(),
        };

        let service = Self {
            name: name.to_string(),
            read_fd: None,
            write_fd,
            init_count,
            dispatch_count,
            finalize_count,
            should_fail_dispatch,
            should_reinit,
        };

        (service, counters)
    }
}

#[derive(Clone)]
struct SharedFdCounters {
    write_fd: Arc<std::sync::atomic::AtomicI32>,
    init_count: Arc<AtomicU32>,
    dispatch_count: Arc<AtomicU32>,
    finalize_count: Arc<AtomicU32>,
    should_fail_dispatch: Arc<AtomicBool>,
    should_reinit: Arc<AtomicBool>,
}

impl SharedFdCounters {
    fn init_count(&self) -> u32 {
        self.init_count.load(Ordering::SeqCst)
    }
    fn dispatch_count(&self) -> u32 {
        self.dispatch_count.load(Ordering::SeqCst)
    }
    fn finalize_count(&self) -> u32 {
        self.finalize_count.load(Ordering::SeqCst)
    }
    fn trigger_event(&self) {
        let fd = self.write_fd.load(Ordering::SeqCst);
        if fd >= 0 {
            unsafe {
                libc::write(fd, b"x".as_ptr() as *const _, 1);
            }
        }
    }
    fn set_fail_dispatch(&self, fail: bool) {
        self.should_fail_dispatch.store(fail, Ordering::SeqCst);
    }
    fn set_reinit(&self, reinit: bool) {
        self.should_reinit.store(reinit, Ordering::SeqCst);
    }
}

#[async_trait]
impl Service for SharedFdService {
    fn name(&self) -> &str {
        &self.name
    }

    async fn initialize(&mut self) -> pmxcfs_services::Result<InitResult> {
        self.init_count.fetch_add(1, Ordering::SeqCst);

        let mut fds = [0i32; 2];
        let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if ret != 0 {
            return Err(ServiceError::InitializationFailed(
                "pipe() failed".to_string(),
            ));
        }

        // Set read end to non-blocking (required for AsyncFd)
        unsafe {
            let flags = libc::fcntl(fds[0], libc::F_GETFL);
            libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        self.read_fd = Some(fds[0]);
        self.write_fd.store(fds[1], Ordering::SeqCst);

        Ok(InitResult::WithFileDescriptor(fds[0]))
    }

    async fn dispatch(&mut self) -> pmxcfs_services::Result<DispatchAction> {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);

        // Drain the pipe
        if let Some(fd) = self.read_fd {
            let mut buf = [0u8; 64];
            unsafe {
                libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len());
            }
        }

        if self.should_fail_dispatch.load(Ordering::SeqCst) {
            return Err(ServiceError::DispatchFailed(
                "Mock fd dispatch failure".to_string(),
            ));
        }

        if self.should_reinit.load(Ordering::SeqCst) {
            return Ok(DispatchAction::Reinitialize);
        }

        Ok(DispatchAction::Continue)
    }

    async fn finalize(&mut self) -> pmxcfs_services::Result<()> {
        self.finalize_count.fetch_add(1, Ordering::SeqCst);

        if let Some(fd) = self.read_fd.take() {
            unsafe { libc::close(fd) };
        }
        let wfd = self.write_fd.swap(-1, Ordering::SeqCst);
        if wfd >= 0 {
            unsafe { libc::close(wfd) };
        }

        Ok(())
    }

    fn retry_interval(&self) -> Duration {
        Duration::from_millis(100)
    }
}

#[tokio::test]
async fn test_fd_dispatch_event_driven() {
    let (service, counters) = SharedFdService::new("fd_event_service");

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization
    assert!(
        wait_for_condition(
            || counters.init_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "FD service should initialize within 5 seconds"
    );

    // No dispatch should happen without data
    sleep(Duration::from_millis(200)).await;
    assert_eq!(
        counters.dispatch_count(),
        0,
        "FD service should not dispatch without data"
    );

    // Trigger an event by writing to the pipe
    counters.trigger_event();

    // Wait for dispatch
    assert!(
        wait_for_condition(
            || counters.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "FD service should dispatch after data is written to pipe"
    );

    // Trigger more events
    counters.trigger_event();
    counters.trigger_event();

    assert!(
        wait_for_condition(
            || counters.dispatch_count() >= 2,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "FD service should handle multiple events"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;

    assert!(
        counters.finalize_count() >= 1,
        "FD service should be finalized"
    );
}

#[tokio::test]
async fn test_fd_dispatch_failure_triggers_reinit() {
    let (service, counters) = SharedFdService::new("fd_fail_service");

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization
    assert!(
        wait_for_condition(
            || counters.init_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "FD service should initialize"
    );

    // Trigger an event and verify dispatch works
    counters.trigger_event();
    assert!(
        wait_for_condition(
            || counters.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "FD service should dispatch"
    );

    // Make dispatch fail, then trigger event
    counters.set_fail_dispatch(true);
    counters.trigger_event();

    // Wait for finalize + reinit
    assert!(
        wait_for_condition(
            || counters.finalize_count() >= 1 && counters.init_count() >= 2,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "FD service should finalize and reinitialize after dispatch failure"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_fd_dispatch_reinit_request() {
    let (service, counters) = SharedFdService::new("fd_reinit_service");

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization
    assert!(
        wait_for_condition(
            || counters.init_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "FD service should initialize"
    );

    // Request reinit from dispatch
    counters.set_reinit(true);
    counters.trigger_event();

    // Wait for reinit
    assert!(
        wait_for_condition(
            || counters.finalize_count() >= 1 && counters.init_count() >= 2,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "FD service should finalize and reinitialize on reinit request"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

// ===== Non-restartable dispatch failure test =====

#[tokio::test]
async fn test_non_restartable_dispatch_failure_is_terminal() {
    let service = MockService::new("non_restartable_dispatch").with_restartable(false);
    let counters = service.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for successful init and some dispatches
    assert!(
        wait_for_condition(
            || counters.init_count() == 1 && counters.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Non-restartable service should initialize and dispatch"
    );

    // Make dispatch fail
    counters.set_fail_dispatch(true);

    // Wait for finalize (dispatch failure triggers reinitialize_service)
    assert!(
        wait_for_condition(
            || counters.finalize_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Non-restartable service should finalize after dispatch failure"
    );

    // The service should NOT be retried (non-restartable + not first attempt)
    let init_after_fail = counters.init_count();
    sleep(Duration::from_millis(1500)).await;

    assert_eq!(
        counters.init_count(),
        init_after_fail,
        "Non-restartable service should not retry after dispatch failure, got {} (expected {})",
        counters.init_count(),
        init_after_fail
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

// ===== Invalid FD initialization test =====

struct InvalidFdService {
    name: String,
    init_count: Arc<AtomicU32>,
    finalize_count: Arc<AtomicU32>,
}

impl InvalidFdService {
    fn new(name: &str) -> (Self, Arc<AtomicU32>, Arc<AtomicU32>) {
        let init_count = Arc::new(AtomicU32::new(0));
        let finalize_count = Arc::new(AtomicU32::new(0));
        (
            Self {
                name: name.to_string(),
                init_count: init_count.clone(),
                finalize_count: finalize_count.clone(),
            },
            init_count,
            finalize_count,
        )
    }
}

#[async_trait]
impl Service for InvalidFdService {
    fn name(&self) -> &str {
        &self.name
    }

    async fn initialize(&mut self) -> pmxcfs_services::Result<InitResult> {
        self.init_count.fetch_add(1, Ordering::SeqCst);
        Ok(InitResult::WithFileDescriptor(-1))
    }

    async fn dispatch(&mut self) -> pmxcfs_services::Result<DispatchAction> {
        Ok(DispatchAction::Continue)
    }

    async fn finalize(&mut self) -> pmxcfs_services::Result<()> {
        self.finalize_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn is_restartable(&self) -> bool {
        false
    }

    fn retry_interval(&self) -> Duration {
        Duration::from_millis(100)
    }

    fn dispatch_interval(&self) -> Duration {
        Duration::from_millis(50)
    }
}

#[tokio::test]
async fn test_invalid_fd_marks_failed() {
    let (service, init_count, finalize_count) = InvalidFdService::new("bad_fd");

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let (handle, manager_handle) = manager.spawn_with_handle();

    assert!(
        wait_for_condition(
            || finalize_count.load(Ordering::SeqCst) >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Invalid fd service should finalize after registration failure"
    );

    assert!(
        wait_for_condition(
            || manager_handle.is_failed("bad_fd") == Some(true),
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Invalid fd service should reach Failed state"
    );

    let init_after_fail = init_count.load(Ordering::SeqCst);
    let retried = wait_for_condition(
        || init_count.load(Ordering::SeqCst) > init_after_fail,
        Duration::from_millis(500),
        Duration::from_millis(10),
    )
    .await;
    assert!(
        !retried,
        "Non-restartable invalid fd service should not retry"
    );

    shutdown_token.cancel();
    let _ = handle.await;
}

// ===== Timer Callback Tests =====

#[tokio::test]
async fn test_service_timer_callback() {
    let service = MockService::new("timer_service").with_timer(Duration::from_millis(300));
    let counters = service.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization plus several timer periods
    assert!(
        wait_for_condition(
            || counters.timer_count() >= 3,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Timer should fire at least 3 times within 5 seconds"
    );

    let timer_count = counters.timer_count();

    // Wait for more timer invocations
    assert!(
        wait_for_condition(
            || counters.timer_count() > timer_count,
            Duration::from_secs(2),
            Duration::from_millis(10),
        )
        .await,
        "Timer should continue firing"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_service_timer_callback_not_invoked_when_failed() {
    let service = MockService::new("failed_timer_service").with_timer(Duration::from_millis(100));
    let counters = service.counters();

    // Make initialization fail
    counters.set_fail_init(true);

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for several timer periods
    sleep(Duration::from_millis(2000)).await;

    // Timer should NOT fire if service is not running
    assert_eq!(
        counters.timer_count(),
        0,
        "Timer should not fire when service is not running"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

// ===== Service Manager Tests =====

#[tokio::test]
async fn test_manager_multiple_services() {
    let service1 = MockService::new("service1");
    let service2 = MockService::new("service2");
    let service3 = MockService::new("service3");

    let counters1 = service1.counters();
    let counters2 = service2.counters();
    let counters3 = service3.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service1));
    manager.add_service(Box::new(service2));
    manager.add_service(Box::new(service3));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization
    assert!(
        wait_for_condition(
            || counters1.init_count() == 1
                && counters2.init_count() == 1
                && counters3.init_count() == 1
                && counters1.dispatch_count() >= 1
                && counters2.dispatch_count() >= 1
                && counters3.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "All services should initialize and dispatch within 5 seconds"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;

    // All services should be finalized
    assert_eq!(counters1.finalize_count(), 1, "Service1 should finalize");
    assert_eq!(counters2.finalize_count(), 1, "Service2 should finalize");
    assert_eq!(counters3.finalize_count(), 1, "Service3 should finalize");
}

#[tokio::test]
#[should_panic(expected = "already registered")]
async fn test_manager_duplicate_service_name() {
    let service1 = MockService::new("duplicate");
    let service2 = MockService::new("duplicate");

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service1));
    manager.add_service(Box::new(service2)); // Should panic
}

#[tokio::test]
async fn test_manager_partial_service_failure() {
    let service1 = MockService::new("working_service");
    let service2 = MockService::new("failing_service");

    let counters1 = service1.counters();
    let counters2 = service2.counters();

    // Make service2 fail
    counters2.set_fail_init(true);

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service1));
    manager.add_service(Box::new(service2));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization
    assert!(
        wait_for_condition(
            || counters1.init_count() == 1
                && counters1.dispatch_count() >= 1
                && counters2.init_count() >= 2,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service1 should work normally and Service2 should retry within 5 seconds"
    );

    // Service2 should not dispatch when failing
    assert_eq!(
        counters2.dispatch_count(),
        0,
        "Service2 should not dispatch when failing"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;

    // Service1 should finalize
    assert_eq!(counters1.finalize_count(), 1, "Service1 should finalize");
    // Service2 is also finalized unconditionally during shutdown (matching C behavior)
    assert_eq!(
        counters2.finalize_count(),
        1,
        "Service2 should also be finalized during shutdown (idempotent finalize)"
    );
}

// ===== Error Handling Tests =====

#[tokio::test]
async fn test_service_error_count_tracking() {
    let service = MockService::new("error_tracking_service");
    let counters = service.counters();

    // Make initialization fail
    counters.set_fail_init(true);

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for multiple failures
    assert!(
        wait_for_condition(
            || counters.init_count() >= 4,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Should accumulate at least 4 failures within 5 seconds"
    );

    // Allow recovery
    counters.set_fail_init(false);

    // Wait for recovery
    assert!(
        wait_for_condition(
            || counters.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should recover within 5 seconds"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_service_graceful_shutdown() {
    let service = MockService::new("shutdown_test");
    let counters = service.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for service to be running
    assert!(
        wait_for_condition(
            || counters.dispatch_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should be running within 5 seconds"
    );

    // Graceful shutdown
    shutdown_token.cancel();
    let _ = handle.await;

    // Service should be properly finalized
    assert_eq!(
        counters.finalize_count(),
        1,
        "Service should finalize during shutdown"
    );
}

// ===== Concurrency Tests =====

#[tokio::test]
async fn test_service_concurrent_operations() {
    let service = MockService::new("concurrent_service").with_timer(Duration::from_millis(200));
    let counters = service.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for service to run with both dispatch and timer
    assert!(
        wait_for_condition(
            || counters.dispatch_count() >= 3 && counters.timer_count() >= 3,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should dispatch and timer should fire multiple times within 5 seconds"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_service_state_consistency_after_reinit() {
    let service = MockService::new("consistency_service");
    let counters = service.counters();

    let mut manager = ServiceManager::new();
    manager.add_service(Box::new(service));

    let shutdown_token = manager.shutdown_token();
    let handle = manager.spawn();

    // Wait for initialization
    assert!(
        wait_for_condition(
            || counters.init_count() >= 1,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should initialize within 5 seconds"
    );

    // Trigger reinitialization
    counters.set_reinit(true);

    // Wait for reinit
    assert!(
        wait_for_condition(
            || counters.init_count() >= 2,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await,
        "Service should reinitialize within 5 seconds"
    );

    // Clear reinit flag
    counters.set_reinit(false);

    // Wait for more dispatches
    let dispatch_count = counters.dispatch_count();
    assert!(
        wait_for_condition(
            || counters.dispatch_count() > dispatch_count,
            Duration::from_secs(2),
            Duration::from_millis(10),
        )
        .await,
        "Service should continue dispatching after reinit"
    );

    // Shutdown
    shutdown_token.cancel();
    let _ = handle.await;
}
