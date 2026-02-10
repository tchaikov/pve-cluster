//! Safe, idiomatic wrapper for Corosync CPG (Closed Process Group)
//!
//! This module provides a trait-based abstraction over the Corosync CPG C API,
//! handling the unsafe FFI boundary and callback lifecycle management internally.

use anyhow::Result;
use rust_corosync::{NodeId, cpg};
use std::sync::Arc;

/// Helper to extract CpgHandler from CPG context
///
/// # Safety
/// - Context must point to a valid Arc<dyn CpgHandler> leaked via Box::into_raw()
/// - Handler must still be alive (CpgService not dropped)
/// - Pointer must be properly aligned for Arc<dyn CpgHandler>
///
/// # Errors
/// Returns error if context is invalid, null, or misaligned
unsafe fn handler_from_context<'a>(handle: cpg::Handle) -> Result<&'a dyn CpgHandler> {
    let context = cpg::context_get(handle)
        .map_err(|e| anyhow::anyhow!("Failed to get CPG context: {e:?}"))?;

    if context == 0 {
        return Err(anyhow::anyhow!("CPG context is null - not initialized"));
    }

    // Validate pointer alignment
    if context % std::mem::align_of::<Arc<dyn CpgHandler>>() as u64 != 0 {
        return Err(anyhow::anyhow!("CPG context pointer misaligned"));
    }

    // Context points to a leaked Arc<dyn CpgHandler>
    // We borrow the Arc to get a reference to the handler
    let arc_ptr = context as *const Arc<dyn CpgHandler>;
    let arc_ref: &Arc<dyn CpgHandler> = unsafe { &*arc_ptr };
    Ok(arc_ref.as_ref())
}

/// Trait for handling CPG events in a safe, idiomatic way
///
/// Implementors receive callbacks when CPG events occur. The trait handles
/// all unsafe pointer conversion and context management internally.
pub trait CpgHandler: Send + Sync + 'static {
    fn on_deliver(&self, group_name: &str, nodeid: NodeId, pid: u32, msg: &[u8]);

    fn on_confchg(
        &self,
        group_name: &str,
        member_list: &[cpg::Address],
        left_list: &[cpg::Address],
        joined_list: &[cpg::Address],
    );
}

/// Safe wrapper for CPG handle that manages callback lifecycle
///
/// This service registers callbacks with the CPG handle and ensures proper
/// cleanup when dropped. It uses Arc reference counting to safely manage
/// the handler lifetime across the FFI boundary.
pub struct CpgService {
    handle: cpg::Handle,
    handler: Arc<dyn CpgHandler>,
}

impl CpgService {
    pub fn new<T: CpgHandler>(handler: Arc<T>) -> Result<Self> {
        fn cpg_deliver_callback(
            handle: &cpg::Handle,
            group_name: String,
            nodeid: NodeId,
            pid: u32,
            msg: &[u8],
            _msg_len: usize,
        ) {
            match unsafe { handler_from_context(*handle) } {
                Ok(handler) => handler.on_deliver(&group_name, nodeid, pid, msg),
                Err(e) => {
                    // Log error but don't panic in FFI context
                    tracing::error!("CPG deliver callback error: {}", e);
                }
            }
        }

        fn cpg_confchg_callback(
            handle: &cpg::Handle,
            group_name: &str,
            member_list: Vec<cpg::Address>,
            left_list: Vec<cpg::Address>,
            joined_list: Vec<cpg::Address>,
        ) {
            match unsafe { handler_from_context(*handle) } {
                Ok(handler) => handler.on_confchg(group_name, &member_list, &left_list, &joined_list),
                Err(e) => {
                    // Log error but don't panic in FFI context
                    tracing::error!("CPG confchg callback error: {}", e);
                }
            }
        }

        let model_data = cpg::ModelData::ModelV1(cpg::Model1Data {
            flags: cpg::Model1Flags::None,
            deliver_fn: Some(cpg_deliver_callback),
            confchg_fn: Some(cpg_confchg_callback),
            totem_confchg_fn: None,
        });

        let handle = cpg::initialize(&model_data, 0)?;

        let handler_dyn: Arc<dyn CpgHandler> = handler;
        let leaked_arc = Box::new(Arc::clone(&handler_dyn));
        let arc_ptr = Box::into_raw(leaked_arc) as u64;

        // Set context with error handling to prevent Arc leak
        if let Err(e) = cpg::context_set(handle, arc_ptr) {
            // Recover the leaked Arc on error
            unsafe {
                let _ = Box::from_raw(arc_ptr as *mut Arc<dyn CpgHandler>);
            }
            // Finalize CPG handle
            let _ = cpg::finalize(handle);
            return Err(e.into());
        }

        Ok(Self {
            handle,
            handler: handler_dyn,
        })
    }

    pub fn join(&self, group_name: &str) -> Result<()> {
        // IMPORTANT: C implementation uses strlen(name) + 1 for CPG name length,
        // which includes the trailing nul. To ensure compatibility with C nodes,
        // we must add \0 to the group name.
        // See src/pmxcfs/dfsm.c: dfsm->cpg_group_name.length = strlen(group_name) + 1;
        let group_string = format!("{group_name}\0");
        tracing::warn!(
            "CPG JOIN: Joining group '{}' (verify matches C's DCDB_CPG_GROUP_NAME='pve_dcdb_v1')",
            group_name
        );
        cpg::join(self.handle, &group_string)?;
        tracing::info!("CPG JOIN: Successfully joined group '{}'", group_name);
        Ok(())
    }

    pub fn leave(&self, group_name: &str) -> Result<()> {
        // Include trailing nul to match C's behavior (see join() comment)
        let group_string = format!("{group_name}\0");
        cpg::leave(self.handle, &group_string)?;
        Ok(())
    }

    pub fn mcast(&self, guarantee: cpg::Guarantee, msg: &[u8]) -> Result<()> {
        cpg::mcast_joined(self.handle, guarantee, msg)?;
        Ok(())
    }

    pub fn dispatch(&self) -> Result<(), rust_corosync::CsError> {
        cpg::dispatch(self.handle, rust_corosync::DispatchFlags::All)
    }

    pub fn fd(&self) -> Result<i32> {
        Ok(cpg::fd_get(self.handle)?)
    }

    pub fn handler(&self) -> &Arc<dyn CpgHandler> {
        &self.handler
    }

    pub fn handle(&self) -> cpg::Handle {
        self.handle
    }
}

impl Drop for CpgService {
    fn drop(&mut self) {
        // Recover leaked Arc
        match cpg::context_get(self.handle) {
            Ok(context) if context != 0 => {
                unsafe {
                    let _boxed = Box::from_raw(context as *mut Arc<dyn CpgHandler>);
                }
            }
            Ok(_) => {
                tracing::warn!("CPG context was null during drop");
            }
            Err(e) => {
                tracing::error!("Failed to get CPG context during drop: {:?}", e);
            }
        }

        // Finalize CPG handle
        if let Err(e) = cpg::finalize(self.handle) {
            tracing::error!("Failed to finalize CPG handle: {:?}", e);
        }
    }
}

/// SAFETY: CpgService is thread-safe because:
/// 1. cpg::Handle is thread-safe per Corosync documentation
/// 2. Handler is protected by Arc reference counting
/// 3. CpgHandler trait requires Send + Sync
/// 4. All mutable state is synchronized via CPG's internal locking
unsafe impl Send for CpgService {}
unsafe impl Sync for CpgService {}
