//! In-memory tracking of active IPTV provider connections.
//!
//! Prevents exceeding provider connection limits and enables
//! automatic rotation across providers.

use dashmap::DashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Global provider usage tracker.
static TRACKER: LazyLock<DashMap<Uuid, ProviderUsage>> = LazyLock::new(DashMap::new);

struct ProviderUsage {
    active_sessions: AtomicU32,
    max_connections: u32,
    last_activity: Instant,
}

/// Check if a provider has capacity for a new connection.
pub fn has_capacity(provider_id: Uuid) -> bool {
    match TRACKER.get(&provider_id) {
        Some(usage) => usage.active_sessions.load(Ordering::Relaxed) < usage.max_connections,
        None => true, // Unknown provider = assume capacity
    }
}

/// Register a provider's max connections (call during sync when account_info is fetched).
pub fn register_provider(provider_id: Uuid, max_connections: u32) {
    TRACKER.entry(provider_id).or_insert_with(|| ProviderUsage {
        active_sessions: AtomicU32::new(0),
        max_connections,
        last_activity: Instant::now(),
    });
    // Update max_connections if changed
    if let Some(mut usage) = TRACKER.get_mut(&provider_id) {
        usage.max_connections = max_connections;
    }
}

/// Acquire a connection slot. Returns false if at capacity.
pub fn acquire(provider_id: Uuid) -> bool {
    let mut usage = TRACKER.entry(provider_id).or_insert_with(|| ProviderUsage {
        active_sessions: AtomicU32::new(0),
        max_connections: 1, // Default: 1 connection
        last_activity: Instant::now(),
    });
    let current = usage.active_sessions.load(Ordering::Relaxed);
    if current >= usage.max_connections {
        return false;
    }
    usage.active_sessions.fetch_add(1, Ordering::Relaxed);
    usage.last_activity = Instant::now();
    true
}

/// Release a connection slot.
pub fn release(provider_id: Uuid) {
    if let Some(usage) = TRACKER.get(&provider_id) {
        let current = usage.active_sessions.load(Ordering::Relaxed);
        if current > 0 {
            usage.active_sessions.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Touch activity timestamp (call on heartbeat).
pub fn touch(provider_id: Uuid) {
    if let Some(mut usage) = TRACKER.get_mut(&provider_id) {
        usage.last_activity = Instant::now();
    }
}

/// Clean up stale sessions (no activity for 60+ seconds).
pub fn cleanup_stale() {
    let cutoff = Instant::now() - Duration::from_secs(60);
    for entry in TRACKER.iter_mut() {
        if entry.last_activity < cutoff {
            entry.active_sessions.store(0, Ordering::Relaxed);
        }
    }
}
