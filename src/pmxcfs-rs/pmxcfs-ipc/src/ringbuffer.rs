/// Lock-free ring buffer implementation compatible with libqb's shared memory IPC
///
/// This module implements a SPSC (single-producer single-consumer) ring buffer
/// using shared memory, matching libqb's wire protocol and memory layout.
///
/// ## Design
///
/// - **Shared Memory**: Two mmap'd files (header + data) in /dev/shm
/// - **Lock-Free**: Uses atomic operations for read_pt/write_pt synchronization
/// - **Chunk-Based**: Messages stored as [size][magic][data] chunks
/// - **Wire-Compatible**: Matches libqb's qb_ringbuffer_shared_s layout
use anyhow::{Context, Result};
use memmap2::MmapMut;
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use tokio::sync::Notify;

/// Circular mmap wrapper for ring buffer data
///
/// This struct manages a circular memory mapping where the same file is mapped
/// twice in consecutive virtual addresses. This allows ring buffer operations
/// to wrap around naturally without modulo arithmetic.
///
/// Matches libqb's qb_sys_circular_mmap() behavior.
struct CircularMmap {
    /// Starting address of the 2x circular mapping
    addr: *mut libc::c_void,
    /// Size of the file (virtual mapping is 2x this size)
    size: usize,
}

impl CircularMmap {
    /// Create a circular mmap from a file descriptor
    ///
    /// Maps the file TWICE in consecutive virtual addresses, allowing ring buffer
    /// wraparound without modulo arithmetic. Matches libqb's qb_sys_circular_mmap().
    ///
    /// # Arguments
    /// - `fd`: File descriptor of the data file (must be sized to `size` bytes)
    /// - `size`: Size of the file in bytes (virtual mapping will be 2x this)
    ///
    /// # Safety
    /// The file must be properly sized before calling this function.
    unsafe fn new(fd: i32, size: usize) -> Result<Self> {
        // SAFETY: All operations in this function are inherently unsafe as they
        // manipulate raw memory mappings. The caller must ensure the fd is valid
        // and the file is properly sized.
        unsafe {
            // Step 1: Reserve 2x space with anonymous mmap
            let addr_orig = libc::mmap(
                std::ptr::null_mut(),
                size * 2,
                libc::PROT_NONE,
                libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
                -1,
                0,
            );

            if addr_orig == libc::MAP_FAILED {
                anyhow::bail!(
                    "Failed to reserve circular mmap space: {}",
                    std::io::Error::last_os_error()
                );
            }

            // Step 2: Map the file at the start of reserved space
            let addr1 = libc::mmap(
                addr_orig,
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_FIXED | libc::MAP_SHARED,
                fd,
                0,
            );

            if addr1 != addr_orig {
                libc::munmap(addr_orig, size * 2);
                anyhow::bail!(
                    "Failed to map first half of circular buffer: {}",
                    std::io::Error::last_os_error()
                );
            }

            // Step 3: Map the SAME file again right after
            let addr_next = (addr_orig as *mut u8).add(size) as *mut libc::c_void;
            let addr2 = libc::mmap(
                addr_next,
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_FIXED | libc::MAP_SHARED,
                fd,
                0,
            );

            if addr2 != addr_next {
                libc::munmap(addr_orig, size * 2);
                anyhow::bail!(
                    "Failed to map second half of circular buffer: {}",
                    std::io::Error::last_os_error()
                );
            }

            tracing::debug!(
                "Created circular mmap: {:p}, {} bytes (2x {} bytes file)",
                addr_orig,
                size * 2,
                size
            );

            Ok(Self {
                addr: addr_orig,
                size,
            })
        }
    }

    /// Get the base address as a mutable pointer to u32
    ///
    /// This is the most common use case for ring buffers which work with u32 words.
    fn as_mut_ptr(&self) -> *mut u32 {
        self.addr as *mut u32
    }

    /// Zero-initialize the circular mapping
    ///
    /// Only needs to write to the first half due to the circular nature.
    ///
    /// # Safety
    /// The circular mmap must be properly initialized and the address valid.
    unsafe fn zero_initialize(&mut self) {
        // SAFETY: Caller ensures the circular mmap is valid and mapped
        unsafe {
            std::ptr::write_bytes(self.addr as *mut u8, 0, self.size);
        }
    }
}

impl Drop for CircularMmap {
    fn drop(&mut self) {
        // Munmap the 2x circular mapping
        // Matches libqb's cleanup in qb_rb_close_helper
        unsafe {
            libc::munmap(self.addr, self.size * 2);
        }
        tracing::debug!(
            "Unmapped circular buffer: {:p}, {} bytes (2x {} bytes file)",
            self.addr,
            self.size * 2,
            self.size
        );
    }
}

/// Process-shared POSIX semaphore wrapper
///
/// This wraps the native Linux sem_t (32 bytes on x86_64) for inter-process
/// synchronization in the ring buffer.
///
/// **libqb compatibility note**: This corresponds to libqb's `rpl_sem_t` type.
/// On Linux with HAVE_SEM_TIMEDWAIT defined, rpl_sem_t is just an alias for
/// the native sem_t. The "rpl" prefix stands for "replacement" - libqb provides
/// a fallback implementation using mutexes/condvars on systems without proper
/// POSIX semaphore support (like BSD). Since we only target Linux, we use the
/// native sem_t directly.
#[repr(C)]
struct PosixSem {
    /// Raw sem_t storage (32 bytes on Linux x86_64)
    _sem: [u8; 32],
}

impl PosixSem {
    /// Initialize a POSIX semaphore in-place in shared memory
    ///
    /// This initializes the semaphore at its current memory location, which is
    /// critical for process-shared semaphores in mmap'd memory. The semaphore
    /// must not be moved after initialization.
    ///
    /// The semaphore is always initialized as:
    /// - **Process-shared** (pshared=1): Shared between processes via mmap
    /// - **Initial value 0**: No data available initially
    ///
    /// Matches libqb's semaphore initialization in `qb_rb_create_from_file`.
    ///
    /// # Safety
    /// The semaphore must remain at its current memory location and must not
    /// be moved or copied after initialization.
    unsafe fn init_in_place(&mut self) -> Result<()> {
        let sem_ptr = self._sem.as_mut_ptr() as *mut libc::sem_t;

        // pshared=1: Process-shared semaphore (for cross-process IPC)
        // initial_value=0: No data available initially (producers will post)
        const PSHARED: libc::c_int = 1;
        const INITIAL_VALUE: libc::c_uint = 0;

        // SAFETY: Caller ensures the semaphore memory is valid and will remain
        // at this location for its lifetime
        let ret = unsafe { libc::sem_init(sem_ptr, PSHARED, INITIAL_VALUE) };

        if ret != 0 {
            anyhow::bail!("sem_init failed: {}", std::io::Error::last_os_error());
        }

        Ok(())
    }

    /// Destroy the semaphore
    ///
    /// This should be called when the semaphore is no longer needed.
    /// Matches libqb's rpl_sem_destroy (which is sem_destroy on Linux).
    ///
    /// # Safety
    /// The semaphore must have been properly initialized and no threads should
    /// be waiting on it.
    unsafe fn destroy(&mut self) -> Result<()> {
        let sem_ptr = self._sem.as_mut_ptr() as *mut libc::sem_t;

        // SAFETY: Caller ensures the semaphore is initialized and not in use
        let ret = unsafe { libc::sem_destroy(sem_ptr) };

        if ret != 0 {
            anyhow::bail!("sem_destroy failed: {}", std::io::Error::last_os_error());
        }

        Ok(())
    }

    /// Post to the semaphore (increment)
    ///
    /// Matches libqb's rpl_sem_post (which is sem_post on Linux).
    unsafe fn post(&self) -> Result<()> {
        let ret = unsafe { libc::sem_post(self._sem.as_ptr() as *mut libc::sem_t) };

        if ret != 0 {
            anyhow::bail!("sem_post failed: {}", std::io::Error::last_os_error());
        }

        Ok(())
    }

    /// Wait on the semaphore asynchronously (decrement, blocking)
    ///
    /// Uses `spawn_blocking` to wait on the semaphore without blocking the tokio
    /// runtime. This provides true event-driven behavior while maintaining
    /// compatibility with libqb's semaphore-based notification mechanism.
    ///
    /// Matches libqb's `my_posix_sem_timedwait` / `sem_wait` behavior.
    ///
    /// # Safety
    /// The semaphore must be properly initialized and remain valid for the
    /// duration of the wait operation.
    async unsafe fn wait(&self) -> Result<()> {
        // Get raw pointer to semaphore
        let sem_ptr = self._sem.as_ptr() as *mut libc::sem_t;

        // Convert to usize for safe transfer between threads
        // This is safe because:
        // 1. The semaphore is in process-shared memory (mmap'd file)
        // 2. The memory remains valid for the lifetime of the containing structure
        // 3. We're only using the pointer on the blocking thread pool
        let sem_ptr_addr = sem_ptr as usize;

        // Use spawn_blocking to wait on the semaphore without blocking tokio runtime
        // This offloads the blocking sem_wait to tokio's dedicated blocking thread pool
        tokio::task::spawn_blocking(move || {
            // Reconstruct the pointer on the blocking thread
            // SAFETY: The semaphore is in shared memory and remains valid.
            // We're calling sem_wait on a process-shared semaphore from a thread
            // in the same process, which is safe.
            let sem_ptr = sem_ptr_addr as *mut libc::sem_t;
            let ret = unsafe { libc::sem_wait(sem_ptr) };

            if ret != 0 {
                let err = std::io::Error::last_os_error();
                // Handle EINTR by returning an error that causes retry
                if err.raw_os_error() == Some(libc::EINTR) {
                    anyhow::bail!("sem_wait interrupted (EINTR), will retry");
                }
                anyhow::bail!("sem_wait failed: {err}");
            }

            Ok(())
        })
        .await
        .context("spawn_blocking task failed")??;

        Ok(())
    }
}

/// Shared memory header matching libqb's qb_ringbuffer_shared_s layout
///
/// This structure is mmap'd and shared between processes.
/// Field order and alignment must exactly match libqb for compatibility.
///
/// Note: libqb's struct has `char user_data[1]` which contributes 1 byte to sizeof(),
/// then the struct is padded to 8-byte alignment (7 bytes padding).
/// Additional shared_user_data_size bytes are allocated beyond sizeof().
#[repr(C, align(8))]
struct RingBufferShared {
    /// Write pointer (word index, not byte offset)
    write_pt: AtomicU32,
    /// Read pointer (word index, not byte offset)
    read_pt: AtomicU32,
    /// Ring buffer size in words (u32 units)
    word_size: u32,
    /// Path to header file
    hdr_path: [u8; libc::PATH_MAX as usize],
    /// Path to data file
    data_path: [u8; libc::PATH_MAX as usize],
    /// Reference count (for cleanup)
    ref_count: AtomicU32,
    /// Process-shared semaphore for notification
    posix_sem: PosixSem,
    /// Flexible array member placeholder (matches C's char user_data[1])
    /// Actual user_data starts here and continues beyond sizeof(RingBufferShared)
    user_data: [u8; 1],
    // 7 bytes of padding added by align(8) to reach 8248 bytes total
}

impl RingBufferShared {
    /// Chunk header size in 32-bit words (matching libqb)
    const CHUNK_HEADER_WORDS: usize = 2;

    /// Chunk magic numbers (matching libqb qb_ringbuffer_int.h)
    const CHUNK_MAGIC: u32 = 0xA1A1A1A1; // Valid allocated chunk
    const CHUNK_MAGIC_DEAD: u32 = 0xD0D0D0D0; // Reclaimed/dead chunk
    const CHUNK_MAGIC_ALLOC: u32 = 0xA110CED0; // Chunk being allocated

    /// Calculate the next pointer position after a chunk of given size
    ///
    /// This implements libqb's qb_rb_chunk_step logic (ringbuffer.c:464-484):
    /// 1. Skip chunk header (CHUNK_HEADER_WORDS)
    /// 2. Skip user data (rounded up to word boundary)
    /// 3. Wrap around if needed
    ///
    /// # Arguments
    /// - `current_pt`: Current read or write pointer (in words)
    /// - `data_size_bytes`: Size of the data payload in bytes
    ///
    /// # Returns
    /// New pointer position (in words), wrapped to [0, word_size)
    fn chunk_step(&self, current_pt: u32, data_size_bytes: usize) -> u32 {
        let word_size = self.word_size as usize;

        // Convert bytes to words, rounding up to word boundary
        // This matches libqb's logic:
        //   pointer += (chunk_size / sizeof(uint32_t));
        //   if ((chunk_size % (sizeof(uint32_t) * QB_RB_WORD_ALIGN)) != 0) pointer++;
        let data_words = data_size_bytes.div_ceil(std::mem::size_of::<u32>());

        // Calculate new position: current + header + data (in words)
        let new_pt = (current_pt as usize + Self::CHUNK_HEADER_WORDS + data_words) % word_size;

        new_pt as u32
    }

    /// Initialize a RingBufferShared structure in-place in shared memory
    ///
    /// This initializes the ring buffer header at its current memory location, which is
    /// critical for process-shared data structures in mmap'd memory. The structure
    /// must not be moved after initialization.
    ///
    /// # Arguments
    /// - `word_size`: Size of ring buffer in 32-bit words
    /// - `hdr_path`: Path to the header file (will be copied into the structure)
    /// - `data_path`: Path to the data file (will be copied into the structure)
    ///
    /// # Safety
    /// The RingBufferShared must remain at its current memory location and must not
    /// be moved or copied after initialization.
    unsafe fn init_in_place(
        &mut self,
        word_size: u32,
        hdr_path: &std::path::Path,
        data_path: &std::path::Path,
    ) -> Result<()> {
        // SAFETY: Caller ensures this structure is in shared memory and will remain
        // at this location for its lifetime
        unsafe {
            // Zero-initialize the entire structure first
            std::ptr::write_bytes(self as *mut Self, 0, 1);

            // Initialize atomic fields
            self.write_pt = AtomicU32::new(0);
            self.read_pt = AtomicU32::new(0);
            self.word_size = word_size;
            self.ref_count = AtomicU32::new(1);

            // Initialize semaphore in-place in shared memory
            // This is critical - the semaphore must be initialized at its final location
            self.posix_sem
                .init_in_place()
                .context("Failed to initialize semaphore")?;

            // Copy header path into structure
            let hdr_path_str = hdr_path.to_string_lossy();
            let hdr_path_bytes = hdr_path_str.as_bytes();
            let len = hdr_path_bytes.len().min(libc::PATH_MAX as usize - 1);
            self.hdr_path[..len].copy_from_slice(&hdr_path_bytes[..len]);

            // Copy data path into structure
            let data_path_str = data_path.to_string_lossy();
            let data_path_bytes = data_path_str.as_bytes();
            let len = data_path_bytes.len().min(libc::PATH_MAX as usize - 1);
            self.data_path[..len].copy_from_slice(&data_path_bytes[..len]);
        }

        Ok(())
    }

    /// Calculate free space in the ring buffer (in words)
    ///
    /// Returns the number of free words (u32 units) available for allocation.
    /// This uses atomic loads to read the pointers safely.
    fn space_free_words(&self) -> usize {
        let write_pt = self.write_pt.load(Ordering::Acquire);
        let read_pt = self.read_pt.load(Ordering::Acquire);
        let word_size = self.word_size as usize;

        if write_pt >= read_pt {
            if write_pt == read_pt {
                word_size // Buffer is empty, all space available
            } else {
                (read_pt as usize + word_size - write_pt as usize) - 1
            }
        } else {
            (read_pt as usize - write_pt as usize) - 1
        }
    }

    /// Calculate free space in bytes
    ///
    /// Converts the word count to bytes by multiplying by sizeof(uint32_t).
    /// Matches libqb's qb_rb_space_free (ringbuffer.c:373).
    fn space_free_bytes(&self) -> usize {
        self.space_free_words() * std::mem::size_of::<u32>()
    }

    /// Check if a chunk of given size (in bytes) can fit in the buffer
    ///
    /// Includes chunk header overhead and alignment requirements.
    fn chunk_fits(&self, message_size: usize, chunk_margin: usize) -> bool {
        let required_bytes = message_size + chunk_margin;
        self.space_free_bytes() >= required_bytes
    }

    /// Write a chunk to the ring buffer
    ///
    /// This performs the complete chunk write operation:
    /// 1. Allocate space in the ring buffer
    /// 2. Write the message data (handling wraparound)
    /// 3. Commit the chunk (update write_pt, set magic)
    /// 4. Post to semaphore to wake readers
    ///
    /// # Safety
    /// Caller must ensure:
    /// - shared_data points to valid ring buffer data
    /// - There is sufficient space (checked via chunk_fits)
    /// - No other thread is writing concurrently
    unsafe fn write_chunk(&self, shared_data: *mut u32, message: &[u8]) -> Result<()> {
        let msg_len = message.len();
        let word_size = self.word_size as usize;

        // Get current write pointer
        let write_pt = self.write_pt.load(Ordering::Acquire);

        // Write chunk header: [size=0][magic=ALLOC]
        // Matches libqb's qb_rb_chunk_alloc (ringbuffer.c:439-440)
        unsafe {
            *shared_data.add(write_pt as usize) = 0; // Size is 0 during allocation
            *shared_data.add((write_pt as usize + 1) % word_size) = Self::CHUNK_MAGIC_ALLOC;
        }

        // Write message data
        let data_offset = (write_pt as usize + Self::CHUNK_HEADER_WORDS) % word_size;
        let data_ptr = unsafe { shared_data.add(data_offset) as *mut u8 };

        // Handle wraparound - calculate remaining bytes in buffer before wraparound
        let remaining = (word_size - data_offset) * std::mem::size_of::<u32>();
        if msg_len <= remaining {
            // No wraparound needed
            unsafe {
                std::ptr::copy_nonoverlapping(message.as_ptr(), data_ptr, msg_len);
            }
        } else {
            // Need to wrap around
            unsafe {
                std::ptr::copy_nonoverlapping(message.as_ptr(), data_ptr, remaining);
                std::ptr::copy_nonoverlapping(
                    message.as_ptr().add(remaining),
                    shared_data as *mut u8,
                    msg_len - remaining,
                );
            }
        }

        // Calculate new write pointer - matches libqb's qb_rb_chunk_step logic
        let new_write_pt = self.chunk_step(write_pt, msg_len);

        // Commit: write size, update write pointer, then set magic with atomic RELEASE
        // This matches libqb's qb_rb_chunk_commit behavior (ringbuffer.c:497-504)
        unsafe {
            // 1. Write chunk size
            *shared_data.add(write_pt as usize) = msg_len as u32;

            // 2. Update write pointer
            self.write_pt.store(new_write_pt, Ordering::Relaxed);

            // 3. Set magic with RELEASE
            // RELEASE ensures all previous writes (data, size, write_pt) are visible before magic
            let magic_offset = (write_pt as usize + 1) % word_size;
            let magic_ptr = shared_data.add(magic_offset) as *mut AtomicU32;
            (*magic_ptr).store(Self::CHUNK_MAGIC, Ordering::Release);

            // 4. Post to semaphore to wake up waiting readers
            self.posix_sem
                .post()
                .context("Failed to post to semaphore")?;
        }

        tracing::debug!(
            "Wrote chunk: {} bytes, write_pt {} -> {}",
            msg_len,
            write_pt,
            new_write_pt
        );

        Ok(())
    }

    /// Read a chunk from the ring buffer
    ///
    /// This reads the chunk at the current read pointer, validates it,
    /// copies the data, and reclaims the chunk.
    ///
    /// Returns None if the buffer is empty (read_pt == write_pt).
    ///
    /// # Safety
    /// Caller must ensure:
    /// - shared_data points to valid ring buffer data
    /// - flow_control_ptr (if Some) points to valid i32
    /// - No other thread is reading concurrently
    unsafe fn read_chunk(
        &self,
        shared_data: *mut u32,
        flow_control_ptr: Option<*mut i32>,
    ) -> Result<Option<Vec<u8>>> {
        let word_size = self.word_size as usize;

        // Get current read pointer
        let read_pt = self.read_pt.load(Ordering::Acquire);
        let write_pt = self.write_pt.load(Ordering::Acquire);

        // Check if buffer is empty
        if read_pt == write_pt {
            return Ok(None);
        }

        // Read chunk header with ACQUIRE to see all writes
        //
        // Memory ordering protocol (matching libqb):
        // 1. Writer: writes chunk_size, then write_pt, then sets magic with RELEASE
        // 2. Reader: reads magic with ACQUIRE, ensuring previous writes are visible
        // 3. Since magic was set AFTER chunk_size, the Acquire fence guarantees
        //    chunk_size is visible (even though we read it non-atomically below)
        //
        // This protocol is safe because:
        // - Only one reader (SPSC ring buffer)
        // - Size is written before magic, and magic acts as a "ready" flag
        // - Acquire-Release pair establishes happens-before relationship
        let magic_offset = (read_pt as usize + 1) % word_size;
        let magic_ptr = unsafe { shared_data.add(magic_offset) as *const AtomicU32 };
        let chunk_magic = unsafe { (*magic_ptr).load(Ordering::Acquire) };

        // Read chunk size (non-atomic, but safe due to Acquire fence above)
        let chunk_size = unsafe { *shared_data.add(read_pt as usize) };

        // Validate chunk size is within reasonable bounds
        // Maximum chunk size is the ring buffer size minus overhead
        let max_chunk_size = (word_size * std::mem::size_of::<u32>()).saturating_sub(Self::CHUNK_HEADER_WORDS * std::mem::size_of::<u32>() + 64);
        if chunk_size == 0 || chunk_size as usize > max_chunk_size {
            anyhow::bail!(
                "Invalid chunk size {} at read_pt {} (max allowed: {})",
                chunk_size,
                read_pt,
                max_chunk_size
            );
        }

        tracing::debug!(
            "Reading chunk: read_pt={}, write_pt={}, size={}, magic=0x{:08x}",
            read_pt,
            write_pt,
            chunk_size,
            chunk_magic
        );

        // Verify magic
        if chunk_magic != Self::CHUNK_MAGIC {
            anyhow::bail!(
                "Invalid chunk magic at read_pt={}: expected 0x{:08x}, got 0x{:08x}",
                read_pt,
                Self::CHUNK_MAGIC,
                chunk_magic
            );
        }

        // Read message data
        let data_offset = (read_pt as usize + Self::CHUNK_HEADER_WORDS) % word_size;
        let data_ptr = unsafe { shared_data.add(data_offset) as *const u8 };

        let mut message = vec![0u8; chunk_size as usize];

        // Handle wraparound - calculate remaining bytes in buffer before wraparound
        let remaining = (word_size - data_offset) * std::mem::size_of::<u32>();
        if chunk_size as usize <= remaining {
            // No wraparound
            unsafe {
                std::ptr::copy_nonoverlapping(data_ptr, message.as_mut_ptr(), chunk_size as usize);
            }
        } else {
            // Wraparound
            unsafe {
                std::ptr::copy_nonoverlapping(data_ptr, message.as_mut_ptr(), remaining);
                std::ptr::copy_nonoverlapping(
                    shared_data as *const u8,
                    message.as_mut_ptr().add(remaining),
                    chunk_size as usize - remaining,
                );
            }
        }

        // Reclaim chunk: clear header and update read pointer
        let new_read_pt = self.chunk_step(read_pt, chunk_size as usize);

        unsafe {
            // Clear chunk size
            *shared_data.add(read_pt as usize) = 0;

            // Set magic to DEAD with RELEASE
            let magic_ptr = shared_data.add(magic_offset) as *mut AtomicU32;
            (*magic_ptr).store(Self::CHUNK_MAGIC_DEAD, Ordering::Release);

            // Update read_pt
            self.read_pt.store(new_read_pt, Ordering::Relaxed);

            // Signal flow control - server is ready for next request
            if let Some(fc_ptr) = flow_control_ptr {
                let refcount = self.ref_count.load(Ordering::Acquire);
                if refcount == 2 {
                    let fc_atomic = fc_ptr as *mut AtomicI32;
                    (*fc_atomic).store(0, Ordering::Relaxed);
                }
            }
        }

        Ok(Some(message))
    }
}

/// Flow control mechanism for ring buffer backpressure
///
/// Implements libqb's flow control protocol for IPC communication.
/// The server writes flow control values to shared memory, and clients
/// read these values to determine if they should back off.
///
/// Flow control values (matching libqb's rate limiting):
/// - `OK`: Proceed with sending (QB_IPCS_RATE_NORMAL)
/// - `SLOW_DOWN`: Approaching capacity, reduce send rate (QB_IPCS_RATE_OFF)
/// - `STOP`: Queue full, do not send (QB_IPCS_RATE_OFF_2)
///
/// ## Disabled Flow Control
///
/// When constructed with a null fc_ptr, flow control is disabled and all
/// operations become no-ops. This matches libqb's behavior for response/event
/// rings which don't need backpressure signaling.
///
/// Matches libqb's qb_ipc_shm_fc_get/qb_ipc_shm_fc_set (ipc_shm.c:176-195)
pub struct FlowControl {
    /// Pointer to flow control field in shared memory (i32 atomic)
    /// Located in shared_user_data area of RingBufferShared
    /// If null, flow control is disabled (no-op mode)
    fc_ptr: *mut i32,
    /// Pointer to shared header for refcount checks
    /// If null, flow control is disabled (no-op mode)
    shared_hdr: *mut RingBufferShared,
}

impl FlowControl {
    /// OK to send - queue has space (QB_IPCS_RATE_NORMAL)
    pub const OK: i32 = 0;

    /// Slow down - queue approaching full (QB_IPCS_RATE_OFF)
    pub const SLOW_DOWN: i32 = 1;

    /// Stop sending - queue full (QB_IPCS_RATE_OFF_2)
    pub const STOP: i32 = 2;

    /// Create a new FlowControl instance
    ///
    /// Pass null pointers to create a disabled (no-op) flow control instance.
    /// This is used for response/event rings that don't need backpressure.
    ///
    /// # Safety
    /// - If fc_ptr is non-null, it must point to valid shared memory for an i32
    /// - If shared_hdr is non-null, it must point to valid RingBufferShared
    /// - Both must remain valid for the lifetime of FlowControl (if non-null)
    unsafe fn new(fc_ptr: *mut i32, shared_hdr: *mut RingBufferShared) -> Self {
        // Initialize to 0 if enabled - server is ready for requests
        // libqb clients check: if (fc > 0 && fc <= fc_enable_max) return EAGAIN
        // So 0 means "ready to transmit", > 0 means "flow control active/blocked"
        if !fc_ptr.is_null() {
            let fc_atomic = fc_ptr as *mut AtomicI32;
            unsafe {
                (*fc_atomic).store(0, Ordering::Relaxed);
            }
        }

        Self { fc_ptr, shared_hdr }
    }

    /// Check if flow control is enabled
    #[inline]
    fn is_enabled(&self) -> bool {
        !self.fc_ptr.is_null()
    }

    /// Get the raw flow control pointer (for internal use)
    #[inline]
    fn fc_ptr(&self) -> *mut i32 {
        self.fc_ptr
    }

    /// Get flow control value
    ///
    /// Matches libqb's qb_ipc_shm_fc_get (ipc_shm.c:185-195).
    /// Returns:
    /// - 0: Ready for requests (or flow control disabled)
    /// - >0: Flow control active (client should retry)
    /// - <0: Error (not connected)
    ///
    /// Note: This method is primarily for libqb clients, not used internally by server
    #[allow(dead_code)]
    pub fn get(&self) -> i32 {
        if !self.is_enabled() {
            return 0; // Disabled = always ready
        }

        // Check if both client and server are connected (refcount == 2)
        let refcount = unsafe { (*self.shared_hdr).ref_count.load(Ordering::Acquire) };
        if refcount != 2 {
            return -libc::ENOTCONN;
        }

        // Read flow control value atomically
        unsafe {
            let fc_atomic = self.fc_ptr as *const AtomicI32;
            (*fc_atomic).load(Ordering::Relaxed)
        }
    }

    /// Set flow control value
    ///
    /// Matches libqb's qb_ipc_shm_fc_set (ipc_shm.c:176-182).
    /// - fc_enable = 0: Ready for requests
    /// - fc_enable > 0: Flow control active (backpressure)
    ///
    /// No-op if flow control is disabled.
    pub fn set(&self, fc_enable: i32) {
        if !self.is_enabled() {
            return; // Disabled = no-op
        }

        tracing::trace!("Setting flow control to {}", fc_enable);
        unsafe {
            let fc_atomic = self.fc_ptr as *mut AtomicI32;
            (*fc_atomic).store(fc_enable, Ordering::Relaxed);
        }
    }
}

// Safety: FlowControl uses atomic operations for synchronization
unsafe impl Send for FlowControl {}
unsafe impl Sync for FlowControl {}

/// Ring buffer handle
///
/// Owns the mmap'd memory regions and provides async message-passing API.
pub struct RingBuffer {
    /// Mmap of shared header
    _mmap_hdr: MmapMut,
    /// Circular mmap of shared data (2x virtual mapping)
    _mmap_data: CircularMmap,
    /// Pointer to shared header (inside _mmap_hdr)
    shared_hdr: *mut RingBufferShared,
    /// Pointer to shared data array (inside _mmap_data)
    shared_data: *mut u32,
    /// Flow control mechanism
    /// Always present, but may be disabled (no-op) for response/event rings
    pub flow_control: FlowControl,
    /// Notifier for when data becomes available (for consumers)
    data_available: Arc<Notify>,
    /// Notifier for when space becomes available (for producers)
    space_available: Arc<Notify>,
    /// Whether this instance created the ring buffer (and thus owns cleanup)
    /// Matches libqb's QB_RB_FLAG_CREATE flag
    is_creator: bool,
}

// Safety: RingBuffer uses atomic operations for synchronization
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    /// Chunk margin for space calculations (in bytes)
    /// Matches libqb: sizeof(uint32_t) * (CHUNK_HEADER_WORDS + WORD_ALIGN + CACHE_LINE_WORDS)
    /// We don't use cache line alignment, so CACHE_LINE_WORDS = 0
    const CHUNK_MARGIN: usize = 4 * (RingBufferShared::CHUNK_HEADER_WORDS + 1);

    /// Create a new ring buffer in shared memory
    ///
    /// Creates two files in `/dev/shm`:
    /// - `{base_dir}/qb-{name}-header`
    /// - `{base_dir}/qb-{name}-data`
    ///
    /// # Arguments
    /// - `base_dir`: Directory for shared memory files (typically "/dev/shm")
    /// - `name`: Ring buffer name
    /// - `size_bytes`: Size of ring buffer data in bytes
    /// - `shared_user_data_size`: Extra bytes to allocate after RingBufferShared for flow control
    ///
    /// The header file size will be: sizeof(RingBufferShared) + shared_user_data_size
    /// This matches libqb's behavior: sizeof(qb_ringbuffer_shared_s) + shared_user_data_size
    pub fn new(
        base_dir: impl AsRef<Path>,
        name: &str,
        size_bytes: usize,
        shared_user_data_size: usize,
    ) -> Result<Self> {
        let base_dir = base_dir.as_ref();

        // Match libqb's size calculation exactly:
        // 1. Add CHUNK_MARGIN + 1 (13 bytes)
        //    CHUNK_MARGIN = sizeof(uint32_t) * (CHUNK_HEADER_WORDS + WORD_ALIGN + CACHE_LINE_WORDS)
        //    = 4 * (2 + 1 + 0) = 12 bytes (without cache line alignment)
        let size = size_bytes
            .checked_add(Self::CHUNK_MARGIN + 1)
            .context("Ring buffer size overflow when adding CHUNK_MARGIN")?;

        // 2. Round up to page size (typically 4096)
        let page_size = 4096; // Standard page size on Linux
        let pages_needed = size.div_ceil(page_size);
        let real_size = pages_needed
            .checked_mul(page_size)
            .context("Ring buffer size overflow when rounding to page size")?;

        // 3. Calculate word_size from rounded size
        let word_size = real_size / 4;

        tracing::info!(
            "Creating ring buffer '{}': size_bytes={}, real_size={}, word_size={} ({}words = {} bytes)",
            name,
            size_bytes,
            real_size,
            word_size,
            word_size,
            real_size
        );

        // Create header file
        let hdr_filename = format!("qb-{name}-header");
        let hdr_path = base_dir.join(&hdr_filename);

        let hdr_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&hdr_path)
            .context("Failed to create header file")?;

        // Resize to fit RingBufferShared structure + shared_user_data
        // This matches libqb: sizeof(qb_ringbuffer_shared_s) + shared_user_data_size
        let hdr_size = std::mem::size_of::<RingBufferShared>() + shared_user_data_size;
        hdr_file
            .set_len(hdr_size as u64)
            .context("Failed to resize header file")?;

        // Mmap header
        let mut mmap_hdr =
            unsafe { MmapMut::map_mut(&hdr_file) }.context("Failed to mmap header")?;

        // Create data file path (needed for init_in_place)
        let data_filename = format!("qb-{name}-data");
        let data_path = base_dir.join(&data_filename);

        // Initialize shared header
        let shared_hdr = mmap_hdr.as_mut_ptr() as *mut RingBufferShared;

        unsafe {
            (*shared_hdr).init_in_place(word_size as u32, &hdr_path, &data_path)?;
        }

        // Create data file
        let data_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&data_path)
            .context("Failed to create data file")?;

        // Create data file with real_size (NOT 2x real_size!)
        // libqb creates the file with real_size, then uses circular mmap to map it TWICE
        // in consecutive virtual address space. The file itself is only real_size bytes.
        // During cleanup, libqb unmaps 2*real_size bytes (the circular mmap), but the
        // file itself remains real_size bytes.
        data_file
            .set_len(real_size as u64)
            .context("Failed to resize data file")?;

        // Create circular mmap - maps the file TWICE in consecutive virtual memory
        // This matches libqb's qb_sys_circular_mmap implementation
        let data_fd = data_file.as_raw_fd();
        let mut mmap_data = unsafe {
            CircularMmap::new(data_fd, real_size).context("Failed to create circular mmap")?
        };

        // Zero-initialize the data (only need to zero first half due to circular mapping)
        unsafe {
            mmap_data.zero_initialize();
        }

        let shared_data = mmap_data.as_mut_ptr();

        // Write sentinel value at end of buffer (matches libqb behavior)
        // This works now because we have circular mmap with 2x virtual space!
        unsafe {
            *shared_data.add(word_size) = 5;
        }

        // Initialize flow control
        // If shared_user_data_size >= sizeof(i32), flow control is enabled (for request ring)
        // Otherwise, flow control is disabled (for response/event rings)
        let flow_control = if shared_user_data_size >= std::mem::size_of::<i32>() {
            unsafe {
                // Get pointer to user_data field within the structure
                // This matches libqb's: return rb->shared_hdr->user_data;
                let fc_ptr = std::ptr::addr_of_mut!((*shared_hdr).user_data) as *mut i32;
                FlowControl::new(fc_ptr, shared_hdr)
            }
        } else {
            // Disabled flow control (null pointers = no-op mode)
            unsafe { FlowControl::new(std::ptr::null_mut(), std::ptr::null_mut()) }
        };

        Ok(Self {
            _mmap_hdr: mmap_hdr,
            _mmap_data: mmap_data,
            shared_hdr,
            shared_data,
            flow_control,
            data_available: Arc::new(Notify::new()),
            space_available: Arc::new(Notify::new()),
            is_creator: true, // This instance created the ring buffer
        })
    }

    /// Send a message into the ring buffer (async)
    ///
    /// Allocates a chunk, writes the message data, and commits the chunk.
    /// Awaits if there's insufficient space.
    pub async fn send(&mut self, message: &[u8]) -> Result<()> {
        loop {
            match self.try_send(message) {
                Ok(()) => {
                    // Notify consumers that data is available
                    self.data_available.notify_one();
                    return Ok(());
                }
                Err(e) if e.to_string().contains("Insufficient space") => {
                    // Wait for space to become available
                    self.space_available.notified().await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Try to send a message without blocking
    ///
    /// Returns an error if there's insufficient space.
    pub fn try_send(&mut self, message: &[u8]) -> Result<()> {
        // Check if we have enough space
        if !unsafe { (*self.shared_hdr).chunk_fits(message.len(), Self::CHUNK_MARGIN) } {
            let space_free = self.space_free();
            let required = Self::CHUNK_MARGIN + message.len();
            anyhow::bail!(
                "Insufficient space: need {required} bytes, have {space_free} bytes free"
            );
        }

        // Write the chunk using RingBufferShared
        unsafe { (*self.shared_hdr).write_chunk(self.shared_data, message)? };

        Ok(())
    }

    /// Receive a message from the ring buffer (async)
    ///
    /// Awaits if no message is available.
    /// After processing, the chunk is automatically reclaimed.
    ///
    /// ## Implementation Note
    ///
    /// libqb uses semaphore-based blocking (sem_timedwait) to wait for data
    /// (see qb_rb_chunk_peek in libqb/lib/ringbuffer.c).
    ///
    /// We use tokio's `spawn_blocking` to wait on the POSIX semaphore without
    /// blocking the async runtime. This provides true event-driven behavior with
    /// zero polling overhead, while maintaining compatibility with libqb clients.
    pub async fn recv(&mut self) -> Result<Vec<u8>> {
        loop {
            // Wait on POSIX semaphore asynchronously
            // This matches libqb's timedwait_fn behavior in qb_rb_chunk_peek
            // SAFETY: The semaphore is properly initialized in new() and remains
            // valid for the lifetime of RingBuffer
            unsafe { (*self.shared_hdr).posix_sem.wait().await? };

            // Semaphore was decremented, data should be available
            // Read and reclaim the chunk
            match self.recv_after_semwait()? {
                Some(data) => {
                    // Notify producers that space is available
                    self.space_available.notify_one();
                    return Ok(data);
                }
                None => {
                    // Spurious wakeup or race condition - semaphore was decremented
                    // but no valid data found. This shouldn't happen in normal operation.
                    tracing::warn!("Spurious semaphore wakeup detected, retrying");
                    continue;
                }
            }
        }
    }

    /// Receive a message after semaphore has been decremented
    ///
    /// This is called after `PosixSem::wait()` has successfully decremented
    /// the semaphore. It reads the chunk data and reclaims the chunk.
    ///
    /// Returns `None` if the buffer is empty despite semaphore being decremented
    /// (which indicates a bug or race condition).
    fn recv_after_semwait(&mut self) -> Result<Option<Vec<u8>>> {
        // Get fc_ptr if flow control is enabled, otherwise null
        let fc_ptr = if self.flow_control.is_enabled() {
            Some(self.flow_control.fc_ptr())
        } else {
            None
        };
        unsafe { (*self.shared_hdr).read_chunk(self.shared_data, fc_ptr) }
    }

    /// Calculate free space in the ring buffer (in bytes)
    fn space_free(&self) -> usize {
        unsafe { (*self.shared_hdr).space_free_bytes() }
    }

    /// Clean up ring buffer files with path validation
    ///
    /// This validates paths from shared memory to prevent path traversal attacks.
    /// Only removes files that:
    /// - Start with /dev/shm/qb-
    /// - Don't contain ..
    /// - Are less than 256 characters
    fn cleanup_ring_buffer_files(&self) {
        unsafe {
            let hdr_path =
                std::ffi::CStr::from_ptr((*self.shared_hdr).hdr_path.as_ptr() as *const i8);
            let data_path =
                std::ffi::CStr::from_ptr((*self.shared_hdr).data_path.as_ptr() as *const i8);

            // Validate and remove header file
            if let Ok(hdr_path_str) = hdr_path.to_str()
                && !hdr_path_str.is_empty()
                && hdr_path_str.starts_with("/dev/shm/qb-")
                && !hdr_path_str.contains("..")
                && hdr_path_str.len() < 256
            {
                if let Err(e) = std::fs::remove_file(hdr_path_str) {
                    tracing::debug!("Failed to remove header file {}: {}", hdr_path_str, e);
                } else {
                    tracing::debug!("Removed header file: {}", hdr_path_str);
                }
            } else if let Ok(hdr_path_str) = hdr_path.to_str() {
                tracing::error!(
                    "SECURITY: Refusing to remove suspicious header path from shared memory: {}",
                    hdr_path_str
                );
            }

            // Validate and remove data file
            if let Ok(data_path_str) = data_path.to_str()
                && !data_path_str.is_empty()
                && data_path_str.starts_with("/dev/shm/qb-")
                && !data_path_str.contains("..")
                && data_path_str.len() < 256
            {
                if let Err(e) = std::fs::remove_file(data_path_str) {
                    tracing::debug!("Failed to remove data file {}: {}", data_path_str, e);
                } else {
                    tracing::debug!("Removed data file: {}", data_path_str);
                }
            } else if let Ok(data_path_str) = data_path.to_str() {
                tracing::error!(
                    "SECURITY: Refusing to remove suspicious data path from shared memory: {}",
                    data_path_str
                );
            }
        }
    }
}

impl Drop for RingBuffer {
    fn drop(&mut self) {
        // Decrement ref count
        let ref_count = unsafe { (*self.shared_hdr).ref_count.fetch_sub(1, Ordering::AcqRel) };

        tracing::debug!(
            "Dropping ring buffer, ref_count: {} -> {}",
            ref_count,
            ref_count - 1
        );

        // If last reference AND we created it, clean up semaphore and files
        // This matches libqb's behavior: only the creator (QB_RB_FLAG_CREATE) destroys the semaphore
        if ref_count == 1 && self.is_creator {
            unsafe {
                // Destroy the semaphore before cleaning up the mmap
                // Matches libqb's cleanup in qb_rb_close_helper
                if let Err(e) = (*self.shared_hdr).posix_sem.destroy() {
                    tracing::error!("CRITICAL: Failed to destroy semaphore: {}", e);
                }
            }

            // Clean up ring buffer files with path validation
            self.cleanup_ring_buffer_files();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ringbuffer_basic() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut rb = RingBuffer::new(temp_dir.path(), "test", 4096, 0)?;

        // Send a message
        rb.send(b"hello world").await?;

        // Receive the message
        let msg = rb.recv().await?;
        assert_eq!(msg, b"hello world");

        Ok(())
    }

    #[tokio::test]
    async fn test_ringbuffer_multiple_messages() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut rb = RingBuffer::new(temp_dir.path(), "test", 4096, 0)?;

        // Send multiple messages
        rb.send(b"message 1").await?;
        rb.send(b"message 2").await?;
        rb.send(b"message 3").await?;

        // Receive in order
        assert_eq!(rb.recv().await?, b"message 1");
        assert_eq!(rb.recv().await?, b"message 2");
        assert_eq!(rb.recv().await?, b"message 3");

        Ok(())
    }

    #[tokio::test]
    async fn test_ringbuffer_nonblocking_send() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut rb = RingBuffer::new(temp_dir.path(), "test", 4096, 0)?;

        // Test try_send (non-blocking send) with async recv
        rb.try_send(b"data")?;
        let msg = rb.recv().await?;
        assert_eq!(msg, b"data");

        Ok(())
    }

    #[tokio::test]
    async fn test_ringbuffer_wraparound() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut rb = RingBuffer::new(temp_dir.path(), "test", 256, 0)?;

        // Fill and drain to force wraparound
        for _ in 0..10 {
            rb.send(b"data").await?;
            rb.recv().await?;
        }

        // Should still work
        rb.send(b"after wrap").await?;
        assert_eq!(rb.recv().await?, b"after wrap");

        Ok(())
    }
}
