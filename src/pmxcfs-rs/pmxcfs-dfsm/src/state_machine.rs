/// DFSM state machine implementation
///
/// This module contains the main Dfsm struct and its implementation
/// for managing distributed state synchronization.
use anyhow::{Context, Result};
use parking_lot::{Mutex as ParkingMutex, RwLock};
use pmxcfs_api_types::MemberInfo;
use rust_corosync::{NodeId, cpg};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

use super::cpg_service::{CpgHandler, CpgService};
use super::dfsm_message::DfsmMessage;
use super::message::Message;
use super::types::{DfsmMode, QueuedMessage, SyncEpoch};
use crate::{Callbacks, NodeSyncInfo};

/// Maximum queue length to prevent memory exhaustion
/// C implementation uses unbounded GSequence/GList, but we add a limit for safety
/// This value should be tuned based on production workload
const MAX_QUEUE_LEN: usize = 500;

/// Result of a synchronous message send
/// Matches C's dfsm_result_t structure
#[derive(Debug, Clone)]
pub struct MessageResult {
    /// Message count for tracking
    pub msgcount: u64,
    /// Result code from deliver callback (0 = success, negative = errno)
    pub result: i32,
    /// Whether the message was processed successfully
    pub processed: bool,
}

/// Extension trait to add broadcast() method to Option<Arc<Dfsm<M>>>
///
/// This allows calling `.broadcast()` directly on Option<Arc<Dfsm<M>>> fields
/// without explicit None checking at call sites.
pub trait DfsmBroadcast<M: Message> {
    fn broadcast(&self, msg: M);
}

impl<M: Message> DfsmBroadcast<M> for Option<Arc<Dfsm<M>>> {
    fn broadcast(&self, msg: M) {
        if let Some(dfsm) = self {
            let _ = dfsm.broadcast(msg);
        }
    }
}

/// DFSM state machine
///
/// The generic parameter `M` specifies the message type this DFSM handles:
/// - `Dfsm<FuseMessage>` for main database operations
/// - `Dfsm<KvStoreMessage>` for status synchronization
pub struct Dfsm<M> {
    /// CPG service for cluster communication (matching C's dfsm_t->cpg_handle)
    cpg_service: RwLock<Option<Arc<CpgService>>>,

    /// Cluster group name for CPG
    cluster_name: String,

    /// Callbacks for application integration
    callbacks: Arc<dyn Callbacks<Message = M>>,

    /// Current operating mode
    mode: RwLock<DfsmMode>,

    /// Current sync epoch
    sync_epoch: RwLock<SyncEpoch>,

    /// Local epoch counter
    local_epoch_counter: ParkingMutex<u32>,

    /// Node synchronization info
    sync_nodes: RwLock<Vec<NodeSyncInfo>>,

    /// Message queue (ordered by count)
    msg_queue: ParkingMutex<BTreeMap<u64, QueuedMessage<M>>>,

    /// Sync queue for messages during update mode
    sync_queue: ParkingMutex<VecDeque<QueuedMessage<M>>>,

    /// Message counter for ordering (atomic for lock-free increment)
    msg_counter: AtomicU64,

    /// Lowest node ID in cluster (leader)
    lowest_nodeid: RwLock<u32>,

    /// Our node ID (set during init_cpg via cpg_local_get)
    nodeid: AtomicU32,

    /// Our process ID
    pid: u32,

    /// Protocol version for cluster compatibility
    protocol_version: u32,

    /// State verification - SHA-256 checksum
    checksum: ParkingMutex<[u8; 32]>,

    /// Checksum epoch (when it was computed)
    checksum_epoch: ParkingMutex<SyncEpoch>,

    /// Checksum ID for verification
    checksum_id: ParkingMutex<u64>,

    /// Checksum counter for verify requests
    checksum_counter: ParkingMutex<u64>,

    /// Message count received (for synchronous send tracking)
    /// Matches C's dfsm->msgcount_rcvd
    msgcount_rcvd: AtomicU64,

    /// Pending message results for synchronous sends
    /// Matches C's dfsm->results (GHashTable)
    /// Maps msgcount -> oneshot sender for result delivery
    /// Uses tokio oneshot channels - the idiomatic pattern for one-time async notifications
    message_results: ParkingMutex<HashMap<u64, oneshot::Sender<MessageResult>>>,
}

impl<M: Message> Dfsm<M> {
    /// Create a new DFSM instance
    ///
    /// Note: nodeid will be obtained from CPG via cpg_local_get() during init_cpg()
    pub fn new(cluster_name: String, callbacks: Arc<dyn Callbacks<Message = M>>) -> Result<Self> {
        Self::new_with_protocol_version(cluster_name, callbacks, DfsmMessage::<M>::DEFAULT_PROTOCOL_VERSION)
    }

    /// Create a new DFSM instance with a specific protocol version
    ///
    /// This is used when the DFSM needs to use a non-default protocol version,
    /// such as the status/kvstore DFSM which uses protocol version 0 for
    /// compatibility with the C implementation.
    ///
    /// Note: nodeid will be obtained from CPG via cpg_local_get() during init_cpg()
    pub fn new_with_protocol_version(
        cluster_name: String,
        callbacks: Arc<dyn Callbacks<Message = M>>,
        protocol_version: u32,
    ) -> Result<Self> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;
        let pid = std::process::id();

        Ok(Self {
            cpg_service: RwLock::new(None),
            cluster_name,
            callbacks,
            mode: RwLock::new(DfsmMode::Start),
            sync_epoch: RwLock::new(SyncEpoch {
                epoch: 0,
                time: now,
                nodeid: 0,
                pid,
            }),
            local_epoch_counter: ParkingMutex::new(0),
            sync_nodes: RwLock::new(Vec::new()),
            msg_queue: ParkingMutex::new(BTreeMap::new()),
            sync_queue: ParkingMutex::new(VecDeque::new()),
            msg_counter: AtomicU64::new(0),
            lowest_nodeid: RwLock::new(0),
            nodeid: AtomicU32::new(0), // Will be set by init_cpg() using cpg_local_get()
            pid,
            protocol_version,
            checksum: ParkingMutex::new([0u8; 32]),
            checksum_epoch: ParkingMutex::new(SyncEpoch {
                epoch: 0,
                time: 0,
                nodeid: 0,
                pid: 0,
            }),
            checksum_id: ParkingMutex::new(0),
            checksum_counter: ParkingMutex::new(0),
            msgcount_rcvd: AtomicU64::new(0),
            message_results: ParkingMutex::new(HashMap::new()),
        })
    }

    pub fn get_mode(&self) -> DfsmMode {
        *self.mode.read()
    }

    pub fn set_mode(&self, new_mode: DfsmMode) {
        let mut mode = self.mode.write();
        let old_mode = *mode;

        // Match C's dfsm_set_mode logic (dfsm.c:450-456):
        // Allow transition if:
        // 1. new_mode < DFSM_ERROR_MODE_START (normal modes), OR
        // 2. (old_mode < DFSM_ERROR_MODE_START OR new_mode >= old_mode)
        //    - If already in error mode, only allow transitions to higher error codes
        if old_mode != new_mode {
            let allow_transition = !new_mode.is_error() ||
                                   (!old_mode.is_error() || new_mode >= old_mode);

            if !allow_transition {
                tracing::debug!(
                    "DFSM: blocking transition from {:?} to {:?} (error mode can only go to higher codes)",
                    old_mode, new_mode
                );
                return;
            }
        } else {
            // No-op transition
            return;
        }

        *mode = new_mode;
        drop(mode);

        if new_mode.is_error() {
            tracing::error!("DFSM: {}", new_mode);
        } else {
            tracing::info!("DFSM: {}", new_mode);
        }
    }

    pub fn is_leader(&self) -> bool {
        let lowest = *self.lowest_nodeid.read();
        lowest > 0 && lowest == self.nodeid.load(Ordering::Relaxed)
    }

    pub fn get_nodeid(&self) -> u32 {
        self.nodeid.load(Ordering::Relaxed)
    }

    pub fn get_pid(&self) -> u32 {
        self.pid
    }

    /// Check if DFSM is synced and ready
    pub fn is_synced(&self) -> bool {
        self.get_mode() == DfsmMode::Synced
    }

    /// Check if DFSM encountered an error
    pub fn is_error(&self) -> bool {
        self.get_mode().is_error()
    }
}

impl<M: Message> Dfsm<M> {
    fn send_sync_start(&self) -> Result<()> {
        tracing::debug!("DFSM: sending SYNC_START message");
        let sync_epoch = *self.sync_epoch.read();
        self.send_dfsm_message(&DfsmMessage::<M>::SyncStart { sync_epoch })
    }

    fn send_state(&self) -> Result<()> {
        tracing::debug!("DFSM: generating and sending state");

        let state_data = self
            .callbacks
            .get_state()
            .context("Failed to get state from callbacks")?;

        tracing::info!("DFSM: sending state ({} bytes)", state_data.len());

        let sync_epoch = *self.sync_epoch.read();
        let dfsm_msg: DfsmMessage<M> = DfsmMessage::State {
            sync_epoch,
            data: state_data,
        };
        self.send_dfsm_message(&dfsm_msg)?;

        Ok(())
    }

    pub(super) fn send_dfsm_message(&self, message: &DfsmMessage<M>) -> Result<()> {
        let serialized = message.serialize();

        if let Some(ref service) = *self.cpg_service.read() {
            service
                .mcast(cpg::Guarantee::TypeAgreed, &serialized)
                .context("Failed to broadcast DFSM message")?;
            Ok(())
        } else {
            anyhow::bail!("CPG not initialized")
        }
    }

    pub fn process_state(&self, nodeid: u32, pid: u32, state: &[u8]) -> Result<()> {
        tracing::debug!(
            "DFSM: processing state from node {}/{} ({} bytes)",
            nodeid,
            pid,
            state.len()
        );

        let mut sync_nodes = self.sync_nodes.write();

        // Find node in sync_nodes
        let node_info = sync_nodes
            .iter_mut()
            .find(|n| n.node_id == nodeid && n.pid == pid);

        let node_info = match node_info {
            Some(ni) => ni,
            None => {
                // Non-member sent state - immediate LEAVE (matches C: dfsm.c:823-828)
                tracing::error!(
                    "DFSM: received state from non-member {}/{} - entering LEAVE mode",
                    nodeid, pid
                );
                drop(sync_nodes);
                self.set_mode(DfsmMode::Leave);
                return Err(anyhow::anyhow!("State from non-member"));
            }
        };

        // Check for duplicate state (matches C: dfsm.c:830-835)
        if node_info.state.is_some() {
            tracing::error!(
                "DFSM: received duplicate state from member {}/{} - entering LEAVE mode",
                nodeid, pid
            );
            drop(sync_nodes);
            self.set_mode(DfsmMode::Leave);
            return Err(anyhow::anyhow!("Duplicate state from member"));
        }

        // Store state
        node_info.state = Some(state.to_vec());

        let all_received = sync_nodes.iter().all(|n| n.state.is_some());
        drop(sync_nodes);

        if all_received {
            tracing::info!("DFSM: received all states, processing synchronization");
            self.process_state_sync()?;
        }

        Ok(())
    }

    fn process_state_sync(&self) -> Result<()> {
        tracing::info!("DFSM: processing state synchronization");

        let sync_nodes = self.sync_nodes.read().clone();

        match self.callbacks.process_state_update(&sync_nodes) {
            Ok(synced) => {
                if synced {
                    tracing::info!("DFSM: state synchronization successful");

                    let my_nodeid = self.nodeid.load(Ordering::Relaxed);
                    let mut sync_nodes_write = self.sync_nodes.write();
                    if let Some(node) = sync_nodes_write
                        .iter_mut()
                        .find(|n| n.node_id == my_nodeid && n.pid == self.pid)
                    {
                        node.synced = true;
                    }
                    drop(sync_nodes_write);

                    self.set_mode(DfsmMode::Synced);
                    self.callbacks.on_synced();
                    self.deliver_message_queue()?;
                } else {
                    tracing::info!("DFSM: entering UPDATE mode, waiting for leader");
                    self.set_mode(DfsmMode::Update);
                    self.deliver_message_queue()?;
                }
            }
            Err(e) => {
                tracing::error!("DFSM: state synchronization failed: {}", e);
                self.set_mode(DfsmMode::Error);
                return Err(e);
            }
        }

        Ok(())
    }

    pub fn queue_message(&self, nodeid: u32, pid: u32, msg_count: u64, message: M, timestamp: u64)
    where
        M: Clone,
    {
        tracing::debug!(
            "DFSM: queueing message {} from {}/{}",
            msg_count,
            nodeid,
            pid
        );

        let qm = QueuedMessage {
            nodeid,
            pid,
            _msg_count: msg_count,
            message,
            timestamp,
        };

        // Hold mode read lock during queueing decision to prevent TOCTOU race
        // This ensures mode cannot change between check and queue selection
        let mode_guard = self.mode.read();
        let mode = *mode_guard;

        let node_synced = self
            .sync_nodes
            .read()
            .iter()
            .find(|n| n.node_id == nodeid && n.pid == pid)
            .map(|n| n.synced)
            .unwrap_or(false);

        if mode == DfsmMode::Update && node_synced {
            let mut sync_queue = self.sync_queue.lock();

            // Check sync queue size limit
            // Queues use a bounded size (MAX_QUEUE_LEN=500) to prevent memory exhaustion
            // from slow or stuck nodes. When full, oldest messages are dropped.
            // This matches distributed system semantics where old updates can be superseded.
            //
            // Monitoring: Track queue depth via metrics/logs to detect congestion:
            // - Sustained high queue depth indicates slow message processing
            // - Frequent drops indicate network partitions or overload
            if sync_queue.len() >= MAX_QUEUE_LEN {
                tracing::warn!(
                    "DFSM: sync queue full ({} messages), dropping oldest - possible network congestion or slow node",
                    sync_queue.len()
                );
                sync_queue.pop_front();
            }

            sync_queue.push_back(qm);
        } else {
            let mut msg_queue = self.msg_queue.lock();

            // Check message queue size limit (same rationale as sync queue)
            if msg_queue.len() >= MAX_QUEUE_LEN {
                tracing::warn!(
                    "DFSM: message queue full ({} messages), dropping oldest - possible network congestion or slow node",
                    msg_queue.len()
                );
                // Drop oldest message (lowest count)
                if let Some((&oldest_count, _)) = msg_queue.iter().next() {
                    msg_queue.remove(&oldest_count);
                }
            }

            msg_queue.insert(msg_count, qm);
        }

        // Release mode lock after queueing decision completes
        drop(mode_guard);
    }

    pub(super) fn deliver_message_queue(&self) -> Result<()>
    where
        M: Clone,
    {
        let mut queue = self.msg_queue.lock();
        if queue.is_empty() {
            return Ok(());
        }

        tracing::info!("DFSM: delivering {} queued messages", queue.len());

        // Hold mode lock during iteration to prevent mode changes mid-delivery
        let mode_guard = self.mode.read();
        let mode = *mode_guard;
        let sync_nodes = self.sync_nodes.read().clone();

        let mut to_remove = Vec::new();
        let mut to_sync_queue = Vec::new();

        for (count, qm) in queue.iter() {
            let node_info = sync_nodes
                .iter()
                .find(|n| n.node_id == qm.nodeid && n.pid == qm.pid);

            let Some(info) = node_info else {
                tracing::debug!(
                    "DFSM: removing message from non-member {}/{}",
                    qm.nodeid,
                    qm.pid
                );
                to_remove.push(*count);
                continue;
            };

            if mode == DfsmMode::Synced && info.synced {
                tracing::debug!("DFSM: delivering message {}", count);

                match self.callbacks.deliver_message(
                    qm.nodeid,
                    qm.pid,
                    qm.message.clone(),
                    qm.timestamp,
                ) {
                    Ok((result, processed)) => {
                        tracing::debug!(
                            "DFSM: message delivered, result={}, processed={}",
                            result,
                            processed
                        );
                        // Record result for synchronous sends
                        self.record_message_result(*count, result, processed);
                    }
                    Err(e) => {
                        tracing::error!("DFSM: failed to deliver message: {}", e);
                        // Record error result
                        self.record_message_result(*count, -libc::EIO, false);
                    }
                }

                to_remove.push(*count);
            } else if mode == DfsmMode::Update && info.synced {
                // Collect messages to move instead of acquiring sync_queue lock
                // while holding msg_queue lock to prevent deadlock
                to_sync_queue.push(qm.clone());
                to_remove.push(*count);
            }
        }

        // Remove processed messages from queue
        for count in to_remove {
            queue.remove(&count);
        }

        // Release locks before acquiring sync_queue to prevent deadlock
        drop(mode_guard);
        drop(queue);

        // Now move messages to sync_queue without holding msg_queue
        if !to_sync_queue.is_empty() {
            let mut sync_queue = self.sync_queue.lock();
            for qm in to_sync_queue {
                sync_queue.push_back(qm);
            }
        }

        Ok(())
    }

    pub(super) fn deliver_sync_queue(&self) -> Result<()> {
        let mut sync_queue = self.sync_queue.lock();
        let queue_len = sync_queue.len();

        if queue_len == 0 {
            return Ok(());
        }

        tracing::info!("DFSM: delivering {} sync queue messages", queue_len);

        while let Some(qm) = sync_queue.pop_front() {
            tracing::debug!(
                "DFSM: delivering sync message from {}/{}",
                qm.nodeid,
                qm.pid
            );

            match self
                .callbacks
                .deliver_message(qm.nodeid, qm.pid, qm.message, qm.timestamp)
            {
                Ok((result, processed)) => {
                    tracing::debug!(
                        "DFSM: sync message delivered, result={}, processed={}",
                        result,
                        processed
                    );
                    // Record result for synchronous sends
                    self.record_message_result(qm._msg_count, result, processed);
                }
                Err(e) => {
                    tracing::error!("DFSM: failed to deliver sync message: {}", e);
                    // Record error result
                    self.record_message_result(qm._msg_count, -libc::EIO, false);
                }
            }
        }

        Ok(())
    }

    /// Send a message to the cluster
    ///
    /// Creates a properly formatted Normal message with C-compatible headers.
    pub fn send_message(&self, message: M) -> Result<u64> {
        let msg_count = self.msg_counter.fetch_add(1, Ordering::SeqCst) + 1;

        tracing::debug!("DFSM: sending message {}", msg_count);

        let dfsm_msg = DfsmMessage::from_message(msg_count, message, self.protocol_version);

        self.send_dfsm_message(&dfsm_msg)?;

        Ok(msg_count)
    }

    /// Send a message to the cluster and wait for delivery result
    ///
    /// This is the async equivalent of send_message(), matching C's dfsm_send_message_sync().
    /// It broadcasts the message via CPG and waits for it to be delivered to the local node,
    /// returning the result from the deliver callback.
    ///
    /// Uses tokio oneshot channels - the idiomatic pattern for one-time async result delivery.
    /// This avoids any locking or notification complexity.
    ///
    /// # Cancellation Safety
    /// If this future is dropped before completion, the cleanup guard ensures the HashMap
    /// entry is removed, preventing memory leaks.
    ///
    /// # Arguments
    /// * `message` - The message to send
    /// * `timeout` - Maximum time to wait for delivery (typically 10 seconds)
    ///
    /// # Returns
    /// * `Ok(MessageResult)` - The result from the local deliver callback
    ///   - Caller should check `result.result < 0` for errno-based errors
    /// * `Err(_)` - If send failed, timeout occurred, or channel closed unexpectedly
    pub async fn send_message_sync(&self, message: M, timeout: Duration) -> Result<MessageResult> {
        let msg_count = self.msg_counter.fetch_add(1, Ordering::SeqCst) + 1;

        tracing::debug!("DFSM: sending synchronous message {}", msg_count);

        // Create oneshot channel for result delivery (tokio best practice)
        let (tx, rx) = oneshot::channel();

        // Register the sender before broadcasting
        self.message_results.lock().insert(msg_count, tx);

        // RAII guard ensures cleanup on timeout, send error, or cancellation
        // (record_message_result also removes, so double-remove is harmless)
        struct CleanupGuard<'a> {
            msg_count: u64,
            results: &'a ParkingMutex<HashMap<u64, oneshot::Sender<MessageResult>>>,
        }
        impl Drop for CleanupGuard<'_> {
            fn drop(&mut self) {
                self.results.lock().remove(&self.msg_count);
            }
        }
        let _guard = CleanupGuard {
            msg_count,
            results: &self.message_results,
        };

        // Send the message
        let dfsm_msg = DfsmMessage::from_message(msg_count, message, self.protocol_version);
        self.send_dfsm_message(&dfsm_msg)?;

        // Wait for delivery with timeout (clean tokio pattern)
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => {
                // Got result successfully - return it to caller
                // Caller should check result.result < 0 for errno-based errors
                Ok(result)
            }
            Ok(Err(_)) => {
                // Channel closed without sending - shouldn't happen
                anyhow::bail!("DFSM: message {} sender dropped", msg_count);
            }
            Err(_) => {
                // Timeout - guard will clean up
                anyhow::bail!("DFSM: message {} timed out after {:?}", msg_count, timeout);
            }
        }
        // On cancellation (future dropped), guard cleans up automatically
    }

    /// Record the result of a delivered message (for synchronous sends)
    ///
    /// Called from deliver_message_queue() when a message is delivered.
    /// Matches C's dfsm_record_local_result().
    ///
    /// Uses tokio oneshot channel to send result - clean, non-blocking, and can't fail.
    fn record_message_result(&self, msg_count: u64, result: i32, processed: bool) {
        tracing::debug!(
            "DFSM: recording result for message {}: result={}, processed={}",
            msg_count,
            result,
            processed
        );

        // Update msgcount_rcvd
        self.msgcount_rcvd.store(msg_count, Ordering::SeqCst);

        // Send result via oneshot channel if someone is waiting
        let mut results = self.message_results.lock();
        if let Some(tx) = results.remove(&msg_count) {
            let msg_result = MessageResult {
                msgcount: msg_count,
                result,
                processed,
            };

            // Send result through oneshot channel (non-blocking, infallible)
            // If receiver was dropped (timeout), this silently fails - which is fine
            let _ = tx.send(msg_result);
        }
    }

    /// Send a TreeEntry update to the cluster (leader only, during synchronization)
    ///
    /// This is used by the leader to send individual database entries to followers
    /// that need to catch up. Matches C's dfsm_send_update().
    pub fn send_update(&self, tree_entry: pmxcfs_memdb::TreeEntry) -> Result<()> {
        tracing::debug!("DFSM: sending Update for inode {}", tree_entry.inode);

        let sync_epoch = *self.sync_epoch.read();
        let dfsm_msg: DfsmMessage<M> = DfsmMessage::from_tree_entry(tree_entry, sync_epoch);
        self.send_dfsm_message(&dfsm_msg)?;

        Ok(())
    }

    /// Send UpdateComplete signal to cluster (leader only, after sending all updates)
    ///
    /// Signals to followers that all Update messages have been sent and they can
    /// now transition to Synced mode. Matches C's dfsm_send_update_complete().
    pub fn send_update_complete(&self) -> Result<()> {
        tracing::info!("DFSM: sending UpdateComplete");

        let sync_epoch = *self.sync_epoch.read();
        let dfsm_msg: DfsmMessage<M> = DfsmMessage::UpdateComplete { sync_epoch };
        self.send_dfsm_message(&dfsm_msg)?;

        Ok(())
    }

    /// Request checksum verification (leader only)
    /// This should be called periodically by the leader to verify cluster state consistency
    pub fn verify_request(&self) -> Result<()> {
        // Only leader should send verify requests
        if !self.is_leader() {
            return Ok(());
        }

        // Only verify when synced
        if self.get_mode() != DfsmMode::Synced {
            return Ok(());
        }

        // Check if we need to wait for previous verification to complete
        let checksum_counter = *self.checksum_counter.lock();
        let checksum_id = *self.checksum_id.lock();

        if checksum_counter != checksum_id {
            tracing::debug!(
                "DFSM: delaying verify request {:016x}",
                checksum_counter + 1
            );
            return Ok(());
        }

        // Increment counter and send verify request
        *self.checksum_counter.lock() = checksum_counter + 1;
        let new_counter = checksum_counter + 1;

        tracing::debug!("DFSM: sending verify request {:016x}", new_counter);

        // Send VERIFY_REQUEST message with counter
        let sync_epoch = *self.sync_epoch.read();
        let dfsm_msg: DfsmMessage<M> = DfsmMessage::VerifyRequest {
            sync_epoch,
            csum_id: new_counter,
        };
        self.send_dfsm_message(&dfsm_msg)?;

        Ok(())
    }

    /// Handle verify request from leader
    pub fn handle_verify_request(&self, message_epoch: SyncEpoch, csum_id: u64) -> Result<()> {
        tracing::debug!("DFSM: received verify request {:016x}", csum_id);

        // Compute current state checksum
        let mut checksum = [0u8; 32];
        self.callbacks.compute_checksum(&mut checksum)?;

        // Save checksum info
        // Store the epoch FROM THE MESSAGE (matching C: dfsm.c:736)
        *self.checksum.lock() = checksum;
        *self.checksum_epoch.lock() = message_epoch;
        *self.checksum_id.lock() = csum_id;

        // Send the checksum verification response
        tracing::debug!("DFSM: sending verify response");

        let sync_epoch = *self.sync_epoch.read();
        let dfsm_msg = DfsmMessage::Verify {
            sync_epoch,
            csum_id,
            checksum,
        };
        self.send_dfsm_message(&dfsm_msg)?;

        Ok(())
    }

    /// Handle verify response from a node
    pub fn handle_verify(
        &self,
        message_epoch: SyncEpoch,
        csum_id: u64,
        received_checksum: &[u8; 32],
    ) -> Result<()> {
        tracing::debug!("DFSM: received verify response");

        let our_checksum_id = *self.checksum_id.lock();
        let our_checksum_epoch = *self.checksum_epoch.lock();

        // Check if this verification matches our saved checksum
        // Compare with MESSAGE epoch, not current epoch (matching C: dfsm.c:766-767)
        if our_checksum_id == csum_id && our_checksum_epoch == message_epoch {
            let our_checksum = *self.checksum.lock();

            // Compare checksums
            if our_checksum != *received_checksum {
                tracing::error!(
                    "DFSM: checksum mismatch! Expected {:016x?}, got {:016x?}",
                    &our_checksum[..8],
                    &received_checksum[..8]
                );
                tracing::error!("DFSM: data divergence detected - restarting cluster sync");
                self.set_mode(DfsmMode::Leave);
                return Err(anyhow::anyhow!("Checksum verification failed"));
            } else {
                tracing::info!("DFSM: data verification successful");
            }
        } else {
            tracing::debug!("DFSM: skipping verification - no checksum saved or epoch mismatch");
        }

        Ok(())
    }

    /// Invalidate saved checksum (called on membership changes)
    pub fn invalidate_checksum(&self) {
        let counter = *self.checksum_counter.lock();
        *self.checksum_id.lock() = counter;

        // Reset checksum epoch
        *self.checksum_epoch.lock() = SyncEpoch {
            epoch: 0,
            time: 0,
            nodeid: 0,
            pid: 0,
        };

        tracing::debug!("DFSM: checksum invalidated");
    }

    /// Broadcast a message to the cluster
    ///
    /// Checks if the cluster is synced before broadcasting.
    /// If not synced, the message is silently dropped.
    pub fn broadcast(&self, msg: M) -> Result<()> {
        if !self.is_synced() {
            return Ok(());
        }

        tracing::debug!("Broadcasting {:?}", msg);
        self.send_message(msg)?;
        tracing::debug!("Broadcast successful");

        Ok(())
    }

    /// Handle incoming DFSM message from cluster (called by CpgHandler)
    fn handle_dfsm_message(
        &self,
        nodeid: u32,
        pid: u32,
        message: DfsmMessage<M>,
    ) -> anyhow::Result<()> {
        // Validate epoch for state messages (all except Normal and SyncStart)
        // This matches C implementation's epoch checking in dfsm.c:665-673
        let should_validate_epoch = !matches!(
            message,
            DfsmMessage::Normal { .. } | DfsmMessage::SyncStart { .. }
        );

        if should_validate_epoch {
            let current_epoch = *self.sync_epoch.read();
            let message_epoch = match &message {
                DfsmMessage::State { sync_epoch, .. }
                | DfsmMessage::Update { sync_epoch, .. }
                | DfsmMessage::UpdateComplete { sync_epoch }
                | DfsmMessage::VerifyRequest { sync_epoch, .. }
                | DfsmMessage::Verify { sync_epoch, .. } => *sync_epoch,
                _ => unreachable!(),
            };

            if message_epoch != current_epoch {
                tracing::debug!(
                    "DFSM: ignoring message with wrong epoch (expected {:?}, got {:?})",
                    current_epoch,
                    message_epoch
                );
                return Ok(());
            }
        }

        // Match on typed message variants
        match message {
            DfsmMessage::Normal {
                msg_count,
                timestamp,
                protocol_version: _,
                message: app_msg,
            } => self.handle_normal_message(nodeid, pid, msg_count, timestamp, app_msg),
            DfsmMessage::SyncStart { sync_epoch } => self.handle_sync_start(nodeid, sync_epoch),
            DfsmMessage::State {
                sync_epoch: _,
                data,
            } => self.process_state(nodeid, pid, &data),
            DfsmMessage::Update {
                sync_epoch: _,
                tree_entry,
            } => self.handle_update(nodeid, pid, tree_entry),
            DfsmMessage::UpdateComplete { sync_epoch: _ } => self.handle_update_complete(),
            DfsmMessage::VerifyRequest {
                sync_epoch,
                csum_id,
            } => self.handle_verify_request(sync_epoch, csum_id),
            DfsmMessage::Verify {
                sync_epoch,
                csum_id,
                checksum,
            } => self.handle_verify(sync_epoch, csum_id, &checksum),
        }
    }

    /// Handle membership change notification (called by CpgHandler)
    fn handle_membership_change(&self, members: &[MemberInfo]) -> anyhow::Result<()> {
        tracing::info!(
            "DFSM: handling membership change ({} members)",
            members.len()
        );

        // Invalidate saved checksum
        self.invalidate_checksum();

        // Update epoch
        let mut counter = self.local_epoch_counter.lock();
        *counter += 1;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;

        let new_epoch = SyncEpoch {
            epoch: *counter,
            time: now,
            nodeid: self.nodeid.load(Ordering::Relaxed),
            pid: self.pid,
        };

        *self.sync_epoch.write() = new_epoch;
        drop(counter);

        // Find lowest node ID (leader)
        let lowest = members.iter().map(|m| m.node_id).min().unwrap_or(0);
        *self.lowest_nodeid.write() = lowest;

        // Call cleanup callback before releasing sync resources (matches C: dfsm.c:512-514)
        let old_sync_nodes = self.sync_nodes.read().clone();
        if !old_sync_nodes.is_empty() {
            self.callbacks.cleanup_sync_resources(&old_sync_nodes);
        }

        // Initialize sync nodes
        let mut sync_nodes = self.sync_nodes.write();
        sync_nodes.clear();

        for member in members {
            sync_nodes.push(NodeSyncInfo {
                node_id: member.node_id,
                pid: member.pid,
                state: None,
                synced: false,
            });
        }
        drop(sync_nodes);

        // Clear queues
        self.sync_queue.lock().clear();

        // Call membership change callback (matches C: dfsm.c:1180-1182)
        self.callbacks.on_membership_change(members);

        // Determine next mode
        if members.len() == 1 {
            // Single node - already synced
            tracing::info!("DFSM: single node cluster, marking as synced");
            self.set_mode(DfsmMode::Synced);

            // Mark ourselves as synced
            let mut sync_nodes = self.sync_nodes.write();
            if let Some(node) = sync_nodes.first_mut() {
                node.synced = true;
            }

            // Deliver queued messages
            self.deliver_message_queue()?;
        } else {
            // Multi-node - start synchronization
            tracing::info!("DFSM: multi-node cluster, starting sync");
            self.set_mode(DfsmMode::StartSync);

            // If we're the leader, initiate sync
            if self.is_leader() {
                tracing::info!("DFSM: we are leader, sending sync start");
                self.send_sync_start()?;

                // Leader also needs to send its own state
                // (CPG doesn't loop back messages to sender)
                self.send_state().context("Failed to send leader state")?;
            }
        }

        Ok(())
    }

    /// Handle normal application message
    fn handle_normal_message(
        &self,
        nodeid: u32,
        pid: u32,
        msg_count: u64,
        timestamp: u32,
        message: M,
    ) -> Result<()> {
        // C version: deliver immediately if in Synced mode, otherwise queue
        if self.get_mode() == DfsmMode::Synced {
            // Deliver immediately - message is already deserialized
            match self.callbacks.deliver_message(
                nodeid,
                pid,
                message,
                timestamp as u64, // Convert back to u64 for callback compatibility
            ) {
                Ok((result, processed)) => {
                    tracing::debug!(
                        "DFSM: message delivered immediately, result={}, processed={}",
                        result,
                        processed
                    );
                    // Record result for synchronous sends
                    self.record_message_result(msg_count, result, processed);
                }
                Err(e) => {
                    tracing::error!("DFSM: failed to deliver message: {}", e);
                    // Record error result
                    self.record_message_result(msg_count, -libc::EIO, false);
                }
            }
        } else {
            // Queue for later delivery - store typed message directly
            self.queue_message(nodeid, pid, msg_count, message, timestamp as u64);
        }
        Ok(())
    }

    /// Handle SyncStart message from leader
    fn handle_sync_start(&self, nodeid: u32, new_epoch: SyncEpoch) -> Result<()> {
        tracing::info!(
            "DFSM: received SyncStart from node {} with epoch {:?}",
            nodeid,
            new_epoch
        );

        // Adopt the new epoch from the leader (critical for sync protocol!)
        // This matches C implementation which updates dfsm->sync_epoch
        *self.sync_epoch.write() = new_epoch;
        tracing::debug!("DFSM: adopted new sync epoch from leader");

        // Send our state back to the cluster
        // BUT: don't send if we're the leader (we already sent our state in handle_membership_change)
        let my_nodeid = self.nodeid.load(Ordering::Relaxed);
        if nodeid != my_nodeid {
            self.send_state()
                .context("Failed to send state in response to SyncStart")?;
            tracing::debug!("DFSM: sent state in response to SyncStart");
        } else {
            tracing::debug!("DFSM: skipping state send (we're the leader who already sent state)");
        }

        Ok(())
    }

    /// Handle Update message from leader
    fn handle_update(
        &self,
        nodeid: u32,
        pid: u32,
        tree_entry: pmxcfs_memdb::TreeEntry,
    ) -> Result<()> {
        // Serialize TreeEntry for callback (process_update expects raw bytes for now)
        let serialized = tree_entry.serialize_for_update();
        if let Err(e) = self.callbacks.process_update(nodeid, pid, &serialized) {
            tracing::error!("DFSM: failed to process update: {}", e);
        }
        Ok(())
    }

    /// Handle UpdateComplete message
    fn handle_update_complete(&self) -> Result<()> {
        tracing::info!("DFSM: received UpdateComplete from leader");
        self.deliver_sync_queue()?;
        self.set_mode(DfsmMode::Synced);
        self.callbacks.on_synced();
        Ok(())
    }
}

/// Implementation of CpgHandler trait for DFSM
///
/// This allows Dfsm to receive CPG callbacks in an idiomatic Rust way,
/// with all unsafe pointer handling managed by the CpgService.
impl<M: Message> CpgHandler for Dfsm<M> {
    fn on_deliver(&self, _group_name: &str, nodeid: NodeId, pid: u32, msg: &[u8]) {
        tracing::debug!(
            "DFSM CPG message from node {} (pid {}): {} bytes",
            u32::from(nodeid),
            pid,
            msg.len()
        );

        // Deserialize DFSM protocol message
        match DfsmMessage::<M>::deserialize(msg) {
            Ok(dfsm_msg) => {
                if let Err(e) = self.handle_dfsm_message(u32::from(nodeid), pid, dfsm_msg) {
                    tracing::error!("Error handling DFSM message: {}", e);
                }
            }
            Err(e) => {
                tracing::error!("Failed to deserialize DFSM message: {}", e);
            }
        }
    }

    fn on_confchg(
        &self,
        _group_name: &str,
        member_list: &[cpg::Address],
        _left_list: &[cpg::Address],
        _joined_list: &[cpg::Address],
    ) {
        tracing::info!("DFSM CPG membership change: {} members", member_list.len());

        // Build MemberInfo list from CPG addresses
        let members: Vec<MemberInfo> = member_list
            .iter()
            .map(|addr| MemberInfo {
                node_id: u32::from(addr.nodeid),
                pid: addr.pid,
                joined_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            })
            .collect();

        // Notify DFSM of membership change
        if let Err(e) = self.handle_membership_change(&members) {
            tracing::error!("Failed to handle membership change: {}", e);
        }
    }
}

impl<M: Message> Dfsm<M> {
    /// Initialize CPG (Closed Process Group) for cluster communication
    ///
    /// Uses the idiomatic CpgService wrapper which handles all unsafe FFI
    /// and callback management internally.
    pub fn init_cpg(self: &Arc<Self>) -> Result<()> {
        tracing::info!("DFSM: Initializing CPG");

        // Create CPG service with this Dfsm as the handler
        // CpgService handles all callback registration and context management
        let cpg_service = Arc::new(CpgService::new(Arc::clone(self))?);

        // Get our node ID from CPG (matches C's cpg_local_get)
        // This MUST be done after cpg_initialize but before joining the group
        let nodeid = cpg::local_get(cpg_service.handle())?;
        let nodeid_u32 = u32::from(nodeid);
        self.nodeid.store(nodeid_u32, Ordering::Relaxed);
        tracing::info!("DFSM: Got node ID {} from CPG", nodeid_u32);

        // Join the CPG group
        let group_name = &self.cluster_name;
        cpg_service
            .join(group_name)
            .context("Failed to join CPG group")?;

        tracing::info!("DFSM joined CPG group '{}'", group_name);

        // Store the service
        *self.cpg_service.write() = Some(cpg_service);

        // Dispatch once to get initial membership
        if let Some(ref service) = *self.cpg_service.read()
            && let Err(e) = service.dispatch()
        {
            tracing::warn!("Failed to dispatch CPG events: {:?}", e);
        }

        tracing::info!("DFSM CPG initialized successfully");
        Ok(())
    }

    /// Dispatch CPG events (should be called periodically from event loop)
    /// Matching C's service_dfsm_dispatch
    pub fn dispatch_events(&self) -> Result<(), rust_corosync::CsError> {
        if let Some(ref service) = *self.cpg_service.read() {
            service.dispatch()
        } else {
            Ok(())
        }
    }

    /// Get CPG file descriptor for event monitoring
    pub fn fd_get(&self) -> Result<i32> {
        if let Some(ref service) = *self.cpg_service.read() {
            service.fd()
        } else {
            Err(anyhow::anyhow!("CPG service not initialized"))
        }
    }

    /// Stop DFSM services (leave CPG group and finalize)
    pub fn stop_services(&self) -> Result<()> {
        tracing::info!("DFSM: Stopping services");

        // Leave the CPG group before dropping the service
        let group_name = self.cluster_name.clone();
        if let Some(ref service) = *self.cpg_service.read()
            && let Err(e) = service.leave(&group_name)
        {
            tracing::warn!("Error leaving CPG group: {:?}", e);
        }

        // Drop the service (CpgService::drop handles finalization)
        *self.cpg_service.write() = None;

        tracing::info!("DFSM services stopped");
        Ok(())
    }
}
