# pmxcfs-services

**Service Management Framework** for pmxcfs - tokio-based replacement for qb_loop.

Manages long-running services with automatic retry, event-driven dispatching,
periodic timers, and graceful shutdown. Replaces the C implementation's
`qb_loop`-based event management with a tokio async runtime.

## How It Fits Together

- **`Service` trait** (`service.rs`): Lifecycle interface that each service implements
  (`initialize` / `dispatch` / `finalize`, plus optional timer callbacks).
- **`ServiceManager`** (`manager.rs`): Accepts `Box<dyn Service>` via `add_service()`,
  then `spawn()` launches one background task per service that drives it through its lifecycle.
- Each service task handles:
  - **Initialization with retry**: Retries every 5 seconds on failure
  - **Event-driven dispatch**: Waits for file descriptor readability via `AsyncFd`
  - **Timer callbacks**: Optional periodic callbacks at configured intervals
  - **Reinitialization**: Automatic on dispatch failure or explicit request

Shutdown is coordinated through a `CancellationToken`:

```rust
let shutdown_token = manager.shutdown_token();
let handle = manager.spawn();
// ... later ...
shutdown_token.cancel();  // Signal graceful shutdown
handle.await;             // Wait for all services to finalize
```

## Usage Example

```rust
use pmxcfs_services::{Service, ServiceManager};
use std::os::unix::io::RawFd;

struct MyService {
    fd: Option<RawFd>,
}

#[async_trait]
impl Service for MyService {
    fn name(&self) -> &str { "my-service" }

    async fn initialize(&mut self) -> Result<RawFd> {
        let fd = connect_to_external_service()?;
        self.fd = Some(fd);
        Ok(fd)  // Return fd for event monitoring
    }

    async fn dispatch(&mut self) -> Result<bool> {
        handle_events()?;
        Ok(true)  // true = continue, false = reinitialize
    }

    async fn finalize(&mut self) -> Result<()> {
        close_connection(self.fd.take())?;
        Ok(())
    }

    // Optional: periodic timer callback
    fn timer_period(&self) -> Option<Duration> {
        Some(Duration::from_secs(10))
    }

    async fn timer_callback(&mut self) -> Result<()> {
        perform_periodic_task()?;
        Ok(())
    }
}
```

## Service Lifecycle

1. **Initialization**: Service calls `initialize()` which returns a file descriptor
   - On failure: Retries every 5 seconds indefinitely
   - On success: Registers fd with tokio's `AsyncFd` and enters running state

2. **Running**: Service waits for events using `tokio::select!`:
   - **FD readable**: Calls `dispatch()` when fd becomes readable
     - Returns `Ok(true)`: Continue running
     - Returns `Ok(false)`: Reinitialize (calls `finalize()` then `initialize()`)
     - Returns `Err(_)`: Reinitialize
   - **Timer deadline**: Calls `timer_callback()` at configured intervals (if enabled)

3. **Shutdown**: On `CancellationToken::cancel()`:
   - Calls `finalize()` for all services
   - Waits for all service tasks to complete

## C to Rust Mapping

### Data Structures

| C Type | Rust Type | Notes |
|--------|-----------|-------|
| [`cfs_loop_t`](../../pmxcfs/loop.h#L32) | `ServiceManager` | Event loop manager |
| [`cfs_service_t`](../../pmxcfs/loop.h#L34) | `dyn Service` | Service trait |
| [`cfs_service_callbacks_t`](../../pmxcfs/loop.h#L44-L49) | (trait methods) | Callbacks as trait methods |

### Functions

| C Function | Rust Equivalent |
|-----------|-----------------|
| [`cfs_loop_new()`](../../pmxcfs/loop.c) | `ServiceManager::new()` |
| [`cfs_loop_add_service()`](../../pmxcfs/loop.c) | `ServiceManager::add_service()` |
| [`cfs_loop_start_worker()`](../../pmxcfs/loop.c) | `ServiceManager::spawn()` |
| [`cfs_loop_stop_worker()`](../../pmxcfs/loop.c) | `shutdown_token.cancel()` + `handle.await` |
| [`cfs_service_new()`](../../pmxcfs/loop.c) | `struct` + `impl Service` |

## Key Differences from C Implementation

| Aspect | C (`loop.c`) | Rust |
|--------|-------------|------|
| Event loop | libqb `qb_loop`, single-threaded | tokio async runtime, multi-threaded |
| FD monitoring | Manual `qb_loop_poll_add()` | Automatic `AsyncFd` |
| Concurrency | Sequential callbacks | Parallel tasks per service |
| Retry interval | Configurable per service | Fixed 5 seconds (sufficient for all services) |
| Dispatch modes | FD-based or polling | FD-based only (all services use fds) |
| Priority levels | Per-service priorities | All equal (no priority needed) |
| Shutdown | `cfs_loop_stop_worker()` | `CancellationToken` → await tasks → finalize all |

## Design Simplifications

The Rust implementation is significantly simpler than the C version, reducing
the codebase by 67% while preserving all production functionality.

### Why Not Mirror the C Implementation?

The C implementation (`loop.c`) was designed for flexibility to support various
hypothetical use cases. However, after analyzing actual usage across the codebase,
we found that many features were never used:

- **Polling mode**: All services use file descriptors from Corosync libraries
- **Custom retry intervals**: All services work fine with a fixed 5-second retry
- **Non-restartable services**: All services need automatic retry on failure
- **Custom dispatch intervals**: All services are event-driven (no periodic polling)
- **Priority levels**: Service execution order doesn't matter in practice

Rather than maintaining unused complexity "just in case", the Rust implementation
focuses on what's actually needed. This makes the code easier to understand,
test, and maintain.

### Simplifications Applied

- **No polling mode**: All services use file descriptors from C libraries (Corosync)
- **Fixed retry interval**: 5 seconds is sufficient for all services
- **All services restartable**: No need for non-restartable mode
- **Single task per service**: Combines retry, dispatch, and timer logic
- **Direct return types**: No enums (`RawFd` instead of `InitResult`, `bool` instead of `DispatchAction`)

If future requirements demand more flexibility, these features can be added back
incrementally with clear use cases driving the design.

## References

### C Implementation
- [`src/pmxcfs/loop.h`](../../pmxcfs/loop.h) - Service loop API
- [`src/pmxcfs/loop.c`](../../pmxcfs/loop.c) - Service loop implementation

### Related Crates
- **pmxcfs-dfsm**: Uses `Service` trait for `ClusterDatabaseService`, `StatusSyncService`
- **pmxcfs**: Uses `ServiceManager` to orchestrate all cluster services
