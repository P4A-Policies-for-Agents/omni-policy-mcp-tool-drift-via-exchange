//! Per-instance evidence emission debouncer.
//!
//! A drift storm can generate one enforcement decision per request.
//! Without a gate, every request would produce a fresh evidence row.
//! This module keeps emission to at most 1 row per
//! `(tool_name, detection_class)` per 60 s window per policy instance.
//!
//! Invariants:
//! - Map is capped at `cap` entries. When full, LRU-evict on insert.
//! - `should_emit` returns `true` on the first observation OR when the
//!   window has elapsed since the last emission; `false` otherwise.

use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_WINDOW_SECS: u64 = 60;
pub const DEFAULT_CAP: usize = 1024;

type Key = (String, &'static str);

pub struct Debouncer {
    entries: HashMap<Key, u64>,
    order: VecDeque<Key>,
    window_secs: u64,
    cap: usize,
}

impl Debouncer {
    pub fn new(window_secs: u64, cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::with_capacity(cap),
            window_secs,
            cap,
        }
    }

    pub fn should_emit(
        &mut self,
        tool: &str,
        class: &'static str,
        now_secs: u64,
    ) -> bool {
        let key: Key = (tool.to_string(), class);
        if let Some(&last) = self.entries.get(&key) {
            if now_secs.saturating_sub(last) < self.window_secs {
                return false;
            }
            self.entries.insert(key.clone(), now_secs);
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
            return true;
        }
        if self.entries.len() >= self.cap {
            if let Some(evict) = self.order.pop_front() {
                self.entries.remove(&evict);
            }
        }
        self.entries.insert(key.clone(), now_secs);
        self.order.push_back(key);
        true
    }
}

impl Default for Debouncer {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_SECS, DEFAULT_CAP)
    }
}

pub fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_emission_is_allowed() {
        let mut d = Debouncer::new(60, 8);
        assert!(d.should_emit("get_weather", "descriptor_drift", 100));
    }

    #[test]
    fn second_emission_within_window_is_denied() {
        let mut d = Debouncer::new(60, 8);
        assert!(d.should_emit("get_weather", "descriptor_drift", 100));
        assert!(!d.should_emit("get_weather", "descriptor_drift", 130));
        assert!(!d.should_emit("get_weather", "descriptor_drift", 159));
    }

    #[test]
    fn emission_after_window_is_allowed() {
        let mut d = Debouncer::new(60, 8);
        assert!(d.should_emit("get_weather", "descriptor_drift", 100));
        assert!(!d.should_emit("get_weather", "descriptor_drift", 159));
        assert!(d.should_emit("get_weather", "descriptor_drift", 161));
    }

    #[test]
    fn distinct_tool_or_class_keys_are_independent() {
        let mut d = Debouncer::new(60, 8);
        assert!(d.should_emit("get_weather", "descriptor_drift", 100));
        assert!(d.should_emit("get_weather", "unpinned_tool", 100));
        assert!(d.should_emit("other_tool", "descriptor_drift", 100));
    }

    #[test]
    fn sustained_burst_produces_one_emit_per_window() {
        let mut d = Debouncer::new(60, 8);
        let mut emitted = 0;
        for t in 0..200 {
            if d.should_emit("get_weather", "descriptor_drift", t) {
                emitted += 1;
            }
        }
        assert_eq!(emitted, 4);
    }

    #[test]
    fn lru_evicts_when_capacity_exceeded() {
        let mut d = Debouncer::new(60, 2);
        assert!(d.should_emit("a", "descriptor_drift", 100));
        assert!(d.should_emit("b", "descriptor_drift", 100));
        assert!(d.should_emit("c", "descriptor_drift", 100));
        assert!(d.should_emit("a", "descriptor_drift", 105));
    }

    #[test]
    fn refresh_updates_lru_position() {
        let mut d = Debouncer::new(30, 2);
        assert!(d.should_emit("a", "c", 100));
        assert!(d.should_emit("b", "c", 100));
        assert!(d.should_emit("a", "c", 200));
        assert!(d.should_emit("c", "c", 200));
        assert!(!d.should_emit("a", "c", 205));
        assert!(d.should_emit("b", "c", 206));
    }
}
