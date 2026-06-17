//! Background plugin-log maintenance: a periodic sweep that prunes each
//! installed plugin's rotated log segments by age + aggregate size.
//!
//! Modeled on [`crate::infrastructure::services::trash_cleanup_service`]: a
//! single spawned task on a fixed interval, an immediate first run, and
//! log-and-continue on error. `file-rotate` already compresses + caps segment
//! *count* at write time; this is the only thing that enforces the per-plugin
//! age/byte retention and the only thing that ever prunes *idle* plugins (which
//! never trigger a write-time rotation).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::time;
use tracing::{debug, info};

use super::log_store::PluginLogStore;
use crate::application::ports::plugin_ports::PluginManagementPort;

/// Periodically sweeps every installed plugin's logs against its retention.
pub struct PluginLogMaintenanceService {
    log_store: Arc<PluginLogStore>,
    manager: Arc<dyn PluginManagementPort>,
    interval_hours: u64,
}

impl PluginLogMaintenanceService {
    pub fn new(
        log_store: Arc<PluginLogStore>,
        manager: Arc<dyn PluginManagementPort>,
        interval_hours: u64,
    ) -> Self {
        Self {
            log_store,
            manager,
            interval_hours: interval_hours.max(1),
        }
    }

    /// Spawn the periodic sweep task.
    pub fn start(&self) {
        let log_store = self.log_store.clone();
        let manager = self.manager.clone();
        let interval_hours = self.interval_hours;

        info!(
            "Starting plugin log maintenance job with interval of {} hours",
            interval_hours
        );

        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(interval_hours * 60 * 60));
            // First tick fires immediately.
            loop {
                interval.tick().await;
                Self::sweep_all(&log_store, &manager).await;
            }
        });
    }

    async fn sweep_all(log_store: &PluginLogStore, manager: &Arc<dyn PluginManagementPort>) {
        let now = Utc::now();
        let plugins = manager.list();
        debug!("Plugin log sweep over {} plugin(s)", plugins.len());
        for plugin in plugins {
            log_store.request_sweep(&plugin.id, now).await;
        }
    }
}
