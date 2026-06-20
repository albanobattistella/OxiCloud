//! Background watchdog that samples primary DB-pool saturation.
//!
//! A runaway query that pins a connection, multiplied across a small pool, ends
//! in pool exhaustion: every new request then blocks on `acquire()` up to the
//! acquire timeout — the correlated tail-latency cliff where one slow query
//! degrades the whole server. `statement_timeout` (see `db.rs`) caps the cause;
//! this monitor surfaces the symptom early by logging a WARN when in-use
//! connections approach the configured maximum, so an operator can raise
//! `OXICLOUD_DB_MAX_CONNECTIONS` or hunt the slow query before users feel it.
//!
//! The loop only reads in-memory pool counters (`size()` / `num_idle()`) — it
//! never issues a query, so it can never itself contend for a connection.

use sqlx::PgPool;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tracing::{debug, info, warn};

/// WARN once in-use connections reach this fraction of the pool maximum. At
/// ≥90% the pool is one slow query away from forcing `acquire()` waits on
/// every request.
const WARN_UTILIZATION_PCT: u32 = 90;

/// A point-in-time sample of pool occupancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSample {
    /// Connections currently checked out (in use).
    pub active: u32,
    /// Connections sitting idle in the pool.
    pub idle: u32,
    /// Configured maximum connections.
    pub max: u32,
}

impl PoolSample {
    /// In-use connections as a percentage of the configured maximum.
    /// Saturates rather than dividing by zero on an unconfigured pool.
    pub fn utilization_pct(&self) -> u32 {
        if self.max == 0 {
            return 0;
        }
        ((self.active as u64 * 100) / self.max as u64) as u32
    }

    /// True once occupancy has reached the warn threshold.
    pub fn is_saturated(&self, warn_pct: u32) -> bool {
        self.utilization_pct() >= warn_pct
    }
}

/// Read a sqlx pool's occupancy. `size()` is total live connections
/// (idle + in-use); `num_idle()` is the idle subset.
fn sample(pool: &PgPool, max: u32) -> PoolSample {
    let size = pool.size();
    let idle = pool.num_idle() as u32;
    PoolSample {
        active: size.saturating_sub(idle),
        idle,
        max,
    }
}

/// Background saturation watchdog over the primary (user-facing) pool.
pub struct DbPoolMonitor {
    pool: PgPool,
    label: &'static str,
    max_connections: u32,
    interval: Duration,
    /// High-water mark of in-use connections since startup (diagnostics).
    peak_active: Arc<AtomicU32>,
}

impl DbPoolMonitor {
    pub fn new(
        pool: PgPool,
        label: &'static str,
        max_connections: u32,
        interval_secs: u64,
    ) -> Self {
        Self {
            pool,
            label,
            max_connections,
            // Floor the cadence so a misconfiguration can't busy-loop.
            interval: Duration::from_secs(interval_secs.max(1)),
            peak_active: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Spawn the sampling loop. Fire-and-forget.
    pub fn start(self) {
        info!(
            "Starting DB pool saturation monitor ({} pool, every {}s, warn ≥{}%)",
            self.label,
            self.interval.as_secs(),
            WARN_UTILIZATION_PCT,
        );
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let s = sample(&self.pool, self.max_connections);
                let peak = self
                    .peak_active
                    .fetch_max(s.active, Ordering::Relaxed)
                    .max(s.active);

                if s.is_saturated(WARN_UTILIZATION_PCT) {
                    warn!(
                        target: "oxicloud::db",
                        pool = self.label,
                        active = s.active,
                        idle = s.idle,
                        max = s.max,
                        utilization_pct = s.utilization_pct(),
                        peak_active = peak,
                        "⚠️ DB pool near saturation: {}/{} in use ({}%, peak {}) — requests may \
                         be queueing on acquire(); raise OXICLOUD_DB_MAX_CONNECTIONS or \
                         investigate slow queries",
                        s.active,
                        s.max,
                        s.utilization_pct(),
                        peak,
                    );
                } else {
                    debug!(
                        target: "oxicloud::db",
                        pool = self.label,
                        active = s.active,
                        idle = s.idle,
                        max = s.max,
                        "DB pool ok: {}/{} in use ({}%)",
                        s.active,
                        s.max,
                        s.utilization_pct(),
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utilization_and_saturation_thresholds() {
        // 18/20 in use = 90% → saturated at the 90% threshold, not at 95%.
        let near = PoolSample {
            active: 18,
            idle: 2,
            max: 20,
        };
        assert_eq!(near.utilization_pct(), 90);
        assert!(near.is_saturated(WARN_UTILIZATION_PCT));
        assert!(!near.is_saturated(95));

        // 4/20 in use = 20% → calm.
        let calm = PoolSample {
            active: 4,
            idle: 16,
            max: 20,
        };
        assert_eq!(calm.utilization_pct(), 20);
        assert!(!calm.is_saturated(WARN_UTILIZATION_PCT));

        // Fully checked out = 100% → saturated.
        let full = PoolSample {
            active: 20,
            idle: 0,
            max: 20,
        };
        assert_eq!(full.utilization_pct(), 100);
        assert!(full.is_saturated(WARN_UTILIZATION_PCT));

        // Degenerate zero-max pool: no div-by-zero, never saturated.
        let zero = PoolSample {
            active: 0,
            idle: 0,
            max: 0,
        };
        assert_eq!(zero.utilization_pct(), 0);
        assert!(!zero.is_saturated(WARN_UTILIZATION_PCT));
    }
}
