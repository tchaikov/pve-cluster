# pmxcfs-services

**Service Management Framework** for pmxcfs - tokio-based replacement for qb_loop.

Manages long-running services with automatic retry, event-driven dispatching,
periodic timers, and graceful shutdown. Replaces the C implementation's
`qb_loop`-based event management with a tokio async runtime.

## How It Fits Together

- **`Service` trait** (`service.rs`): Lifecycle interface that each service implements
  (`initialize` / `dispatch` / `finalize`, plus optional timer callbacks).
- **`ServiceManager`** (`manager/mod.rs`): Accepts `Box<dyn Service>` via `add_service()`,
  then `spawn()` launches background tasks that drive every service through its lifecycle.
- Internally the manager spawns three kinds of tasks:
  - **Retry task** (`manager/retry.rs`): re-initializes services that failed startup.
  - **Timer task** (`manager/timer.rs`): fires periodic callbacks for running services.
  - **Dispatch tasks** (`manager/dispatch.rs`): one per service, either event-driven
    (via `AsyncFd` on a file descriptor) or polling at a configured interval.

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
use pmxcfs_services::{Service, InitResult, DispatchAction, ServiceManager};

struct MyService {
    fd: Option<RawFd>,
}

#[async_trait]
impl Service for MyService {
    fn name(&self) -> &str { "my-service" }

    async fn initialize(&mut self) -> Result<InitResult> {
        let fd = connect_to_external_service()?;
        self.fd = Some(fd);
        Ok(InitResult::WithFileDescriptor(fd))
    }

    async fn dispatch(&mut self) -> Result<DispatchAction> {
        handle_events()?;
        Ok(DispatchAction::Continue)
    }

    async fn finalize(&mut self) -> Result<()> {
        close_connection(self.fd.take())?;
        Ok(())
    }
}
```

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
| Priority levels | Per-service priorities | All equal (no priority) |
| Shutdown | `cfs_loop_stop_worker()` | `CancellationToken` &#8594; await tasks &#8594; finalize all |

## References

### C Implementation
- [`src/pmxcfs/loop.h`](../../pmxcfs/loop.h) - Service loop API
- [`src/pmxcfs/loop.c`](../../pmxcfs/loop.c) - Service loop implementation

### Related Crates
- **pmxcfs-dfsm**: Uses `Service` trait for `ClusterDatabaseService`, `StatusSyncService`
- **pmxcfs**: Uses `ServiceManager` to orchestrate all cluster services
