use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};

use crate::{
    config::DetectionConfig,
    model::{History, PendingAttempt},
};

pub fn prune_history(history: &mut History, now: DateTime<Utc>, retention_days: u64) -> bool {
    let cutoff = now - TimeDelta::days(retention_days.min(i64::MAX as u64) as i64);
    let old_len = history.records.len();
    history
        .records
        .retain(|record| record.detected_at >= cutoff);
    history.records.len() != old_len
}

pub fn typical_change_seconds(history: &History, sample_size: usize) -> Option<u64> {
    if sample_size == 0 || history.records.is_empty() {
        return None;
    }
    let start = history.records.len().saturating_sub(sample_size);
    let mut values: Vec<u64> = history.records[start..]
        .iter()
        .map(|record| record.interval_seconds)
        .collect();
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[middle])
    } else {
        let lower = values[middle - 1];
        let upper = values[middle];
        Some(lower / 2 + upper / 2 + (lower % 2 + upper % 2) / 2)
    }
}

pub fn check_interval(
    now: DateTime<Utc>,
    current_since: Option<DateTime<Utc>>,
    typical_seconds: Option<u64>,
    config: &DetectionConfig,
) -> Duration {
    let Some(typical) = typical_seconds else {
        return Duration::from_secs(config.fast_interval_seconds);
    };
    let Some(since) = current_since else {
        return Duration::from_secs(config.fast_interval_seconds);
    };

    let elapsed = now.signed_duration_since(since).num_seconds().max(0) as u64;
    let slow_boundary = typical.saturating_mul(config.slow_until_percent as u64) / 100;
    if elapsed < slow_boundary {
        Duration::from_secs(config.slow_interval_seconds)
    } else {
        Duration::from_secs(config.fast_interval_seconds)
    }
}

pub fn retry_delay(attempts: u32, initial_seconds: u64, max_seconds: u64) -> Duration {
    let multiplier = 1_u64
        .checked_shl(attempts.saturating_sub(1).min(63))
        .unwrap_or(u64::MAX);
    Duration::from_secs(initial_seconds.saturating_mul(multiplier).min(max_seconds))
}

pub fn next_pending_retry<'a>(
    attempts: impl Iterator<Item = &'a PendingAttempt>,
) -> Option<DateTime<Utc>> {
    attempts.map(|attempt| attempt.next_retry_at).min()
}

pub fn format_period(seconds: Option<u64>) -> String {
    let Some(seconds) = seconds else {
        return "暂无足够历史数据".to_owned();
    };
    let total_minutes = seconds / 60;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    format!("{hours}小时{minutes}分")
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use chrono::TimeZone;

    use super::*;
    use crate::model::ChangeRecord;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 22, hour, 0, 0).unwrap()
    }

    fn history(intervals: &[u64]) -> History {
        History {
            version: 1,
            records: intervals
                .iter()
                .enumerate()
                .map(|(index, seconds)| ChangeRecord {
                    old_ipv4: Ipv4Addr::new(1, 1, 1, (index + 1) as u8),
                    new_ipv4: Ipv4Addr::new(1, 1, 1, (index + 2) as u8),
                    detected_at: at(index as u32),
                    interval_seconds: *seconds,
                })
                .collect(),
        }
    }

    #[test]
    fn median_uses_latest_samples_and_handles_even_count() {
        assert_eq!(
            typical_change_seconds(&history(&[99, 6, 6, 6, 24]), 4),
            Some(6)
        );
        assert_eq!(typical_change_seconds(&history(&[5, 8]), 10), Some(6));
        assert_eq!(typical_change_seconds(&History::new(), 10), None);
    }

    #[test]
    fn frequency_switches_at_seventy_five_percent() {
        let config = DetectionConfig::default();
        let since = at(0);
        assert_eq!(
            check_interval(at(4), Some(since), Some(6 * 3600), &config),
            Duration::from_secs(1800)
        );
        let boundary = since + TimeDelta::minutes(270);
        assert_eq!(
            check_interval(boundary, Some(since), Some(6 * 3600), &config),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn retry_is_capped() {
        assert_eq!(retry_delay(1, 30, 900), Duration::from_secs(30));
        assert_eq!(retry_delay(6, 30, 900), Duration::from_secs(900));
        assert_eq!(retry_delay(100, 30, 900), Duration::from_secs(900));
    }

    #[test]
    fn period_is_human_readable() {
        assert_eq!(format_period(Some(6 * 3600 + 17 * 60)), "6小时17分");
        assert_eq!(format_period(None), "暂无足够历史数据");
    }
}
