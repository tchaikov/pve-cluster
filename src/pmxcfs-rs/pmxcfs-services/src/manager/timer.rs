//! Timer callback management
//!
//! Periodically invokes timer callbacks for running services that have
//! configured a timer period.

use super::state::{ManagedService, ServiceState, lock_or_recover};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

/// Spawn a task that periodically invokes timer callbacks for running services.
///
/// The tick interval is set to the minimum timer period across all services
/// (or 1 second if no services have timers), ensuring sub-second timer periods
/// are respected.
///
/// The task exits when `token` is cancelled.
pub(crate) fn spawn_timer_task(
    services: Arc<HashMap<String, Arc<ManagedService>>>,
    token: CancellationToken,
) -> JoinHandle<()> {
    // Compute minimum timer period to respect sub-second configurations
    let min_period = services
        .values()
        .filter_map(|m| m.config.timer_period)
        .min()
        .unwrap_or(Duration::from_secs(1));

    tokio::spawn(async move {
        let mut timer_interval = interval(min_period);
        timer_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = timer_interval.tick() => {
                    invoke_timer_callbacks(&services).await;
                }
            }
        }
    })
}

/// Invoke timer callbacks for running services whose period has elapsed.
async fn invoke_timer_callbacks(services: &HashMap<String, Arc<ManagedService>>) {
    let now = Instant::now();

    for (name, managed) in services {
        if managed.load_state() != ServiceState::Running {
            continue;
        }

        let Some(period) = managed.config.timer_period else {
            continue;
        };

        // Check if it's time to invoke timer
        let should_invoke =
            match *lock_or_recover(&managed.last_timer_invoke, "last_timer_invoke") {
                Some(last) => now.duration_since(last) >= period,
                None => true, // First invocation
            };

        if !should_invoke {
            continue;
        }

        *lock_or_recover(&managed.last_timer_invoke, "last_timer_invoke") = Some(now);

        debug!(service = %name, "Invoking timer callback");

        let mut service = managed.service.lock().await;

        if let Err(e) = service.timer_callback().await {
            warn!(service = %name, error = %e, "Timer callback failed");
        }
    }
}
