//! Configuration history audit trail for runtime config changes.
//!
//! Tracks all config modifications made via `sandbox_set_config` with
//! timestamps, keys, old/new values, and attribution.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// A single configuration change entry in the audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChange {
    /// When the change was made.
    pub timestamp: DateTime<Utc>,
    /// Dot-notation key that changed (e.g., "pool.max_total").
    pub key: String,
    /// Previous value as string, or null if the key was new.
    pub old_value: Option<String>,
    /// New value as string.
    pub new_value: String,
    /// Who/what made the change (e.g., "sandbox_set_config").
    pub changed_by: String,
}

impl ConfigChange {
    /// Create a new config change entry.
    pub fn new(
        key: impl Into<String>,
        old_value: Option<String>,
        new_value: impl Into<String>,
        changed_by: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            key: key.into(),
            old_value,
            new_value: new_value.into(),
            changed_by: changed_by.into(),
        }
    }
}

/// In-memory configuration history with bounded storage.
///
/// ConfigHistory stores all config changes in a VecDeque with a maximum
/// capacity to prevent unbounded memory growth. When capacity is exceeded,
/// the oldest entry is dropped.
#[derive(Debug, Clone)]
pub struct ConfigHistory {
    changes: VecDeque<ConfigChange>,
    max_capacity: usize,
}

impl Default for ConfigHistory {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl ConfigHistory {
    /// Create a new ConfigHistory with the specified maximum capacity.
    pub fn new(max_capacity: usize) -> Self {
        Self {
            changes: VecDeque::with_capacity(max_capacity),
            max_capacity,
        }
    }

    /// Add a new config change to the history.
    ///
    /// If the history is at capacity, the oldest entry is silently dropped.
    pub fn add(&mut self, change: ConfigChange) {
        if self.changes.len() >= self.max_capacity {
            self.changes.pop_front();
        }
        self.changes.push_back(change);
    }

    /// Add a config change using convenience constructor.
    pub fn record(
        &mut self,
        key: impl Into<String>,
        old_value: Option<String>,
        new_value: impl Into<String>,
        changed_by: impl Into<String>,
    ) {
        self.add(ConfigChange::new(key, old_value, new_value, changed_by));
    }

    /// Return all config changes in chronological order (oldest first).
    pub fn get_all(&self) -> Vec<&ConfigChange> {
        self.changes.iter().collect()
    }

    /// Return the most recent `n` config changes (newest first).
    pub fn get_recent(&self, n: usize) -> Vec<&ConfigChange> {
        self.changes.iter().rev().take(n).collect()
    }

    /// Return the number of stored changes.
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// Return true if no changes are stored.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_change() {
        let mut history = ConfigHistory::new(100);
        assert!(history.is_empty());

        history.record(
            "pool.max_total",
            Some("10".into()),
            "15",
            "sandbox_set_config",
        );

        assert_eq!(history.len(), 1);
        let changes = history.get_all();
        assert_eq!(changes[0].key, "pool.max_total");
        assert_eq!(changes[0].old_value.as_deref(), Some("10"));
        assert_eq!(changes[0].new_value, "15");
    }

    #[test]
    fn test_get_all_ordered() {
        let mut history = ConfigHistory::new(100);
        history.record("a", None, "1", "test");
        history.record("b", None, "2", "test");
        history.record("c", None, "3", "test");

        let changes = history.get_all();
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].key, "a");
        assert_eq!(changes[1].key, "b");
        assert_eq!(changes[2].key, "c");
    }

    #[test]
    fn test_get_recent() {
        let mut history = ConfigHistory::new(100);
        for i in 0..10 {
            history.record(format!("key_{}", i), None, format!("{}", i), "test");
        }

        let recent = history.get_recent(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].key, "key_9");
        assert_eq!(recent[1].key, "key_8");
        assert_eq!(recent[2].key, "key_7");
    }

    #[test]
    fn test_empty_history() {
        let history = ConfigHistory::new(100);
        assert!(history.is_empty());
        assert!(history.get_all().is_empty());
        assert!(history.get_recent(5).is_empty());
    }

    #[test]
    fn test_capacity_eviction() {
        let mut history = ConfigHistory::new(3);
        history.record("1", None, "1", "test");
        history.record("2", None, "2", "test");
        history.record("3", None, "3", "test");
        assert_eq!(history.len(), 3);

        // Adding a 4th entry should evict the oldest
        history.record("4", None, "4", "test");
        assert_eq!(history.len(), 3);

        let keys: Vec<_> = history.get_all().iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, vec!["2", "3", "4"]);
    }

    #[test]
    fn test_timestamp_set() {
        let mut history = ConfigHistory::new(100);
        history.record("key", None, "value", "test");

        let change = history.get_all()[0];
        // Timestamp should be set to approximately now
        let age = Utc::now() - change.timestamp;
        assert!(age.num_seconds() < 5, "timestamp should be set to now");
    }
}
