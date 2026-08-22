use std::{collections::BTreeMap, net::Ipv4Addr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    pub version: u32,
    pub current_ipv4: Option<Ipv4Addr>,
    pub current_since: Option<DateTime<Utc>>,
    pub last_check_at: Option<DateTime<Utc>>,
    pub typical_change_seconds: Option<u64>,
    pub synced_servers: BTreeMap<String, Ipv4Addr>,
    pub pending_sync: Option<PendingSync>,
    pub pending_notification: Option<PendingNotification>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            version: 1,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingSync {
    pub ipv4: Ipv4Addr,
    pub detected_at: DateTime<Utc>,
    pub servers: BTreeMap<String, PendingAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAttempt {
    pub attempts: u32,
    pub next_retry_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingNotification {
    pub ipv4: Ipv4Addr,
    pub detected_at: DateTime<Utc>,
    pub message: String,
    pub attempts: u32,
    pub next_retry_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub version: u32,
    pub records: Vec<ChangeRecord>,
}

impl History {
    pub fn new() -> Self {
        Self {
            version: 1,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub old_ipv4: Ipv4Addr,
    pub new_ipv4: Ipv4Addr,
    pub detected_at: DateTime<Utc>,
    pub interval_seconds: u64,
}
