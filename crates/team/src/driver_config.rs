//! Per-group driver settings from `team.yaml` (`groups.<name>.driver`) and an
//! hourly counter for wakes and token spend. The clock is passed in, never read.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The cadence knobs a group driver may set. Any other key is refused.
pub const CADENCE_KNOBS: [&str; 2] = ["poll_seconds", "idle_seconds"];

fn default_escalate_after_min() -> u64 {
    10
}

/// One group's driver settings.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriverConfig {
    /// The model the group's driver uses.
    #[serde(default)]
    pub model: Option<String>,
    /// Cadence knobs the driver may set, by allow-listed name.
    #[serde(default)]
    pub cadence: BTreeMap<String, u64>,
    /// Minutes before an unanswered item escalates. Must be at least 1.
    #[serde(default = "default_escalate_after_min")]
    pub escalate_after_min: u64,
    /// Token budget per hour. Zero means no AI spend is allowed.
    #[serde(default)]
    pub budget_tokens_per_hour: u64,
}

impl DriverConfig {
    /// Refuse unknown cadence knobs and a zero escalation time.
    pub fn validate(&self) -> Result<(), String> {
        for key in self.cadence.keys() {
            if !CADENCE_KNOBS.contains(&key.as_str()) {
                return Err(format!("unknown cadence knob `{key}`"));
            }
        }
        if self.escalate_after_min == 0 {
            return Err("escalate_after_min must be at least 1".to_string());
        }
        Ok(())
    }
}

/// The driver of group `group` in the `team.yaml` text `yaml`. `Ok(None)` when
/// the group has no driver key; `Err` naming the group when it is malformed.
pub fn driver_for(yaml: &str, group: &str) -> Result<Option<DriverConfig>, String> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|e| format!("team.yaml: {e}"))?;
    let raw = doc
        .get("groups")
        .and_then(|g| g.get(group))
        .and_then(|g| g.get("driver"));
    let raw = match raw {
        None | Some(serde_yaml::Value::Null) => return Ok(None),
        Some(v) => v,
    };
    let cfg: DriverConfig =
        serde_yaml::from_value(raw.clone()).map_err(|e| format!("group `{group}` driver: {e}"))?;
    cfg.validate()
        .map_err(|e| format!("group `{group}` driver: {e}"))?;
    Ok(Some(cfg))
}

/// Wakes and tokens counted within one hour (`now_s / 3600`). A new hour
/// resets both counters.
#[derive(Clone, Debug, PartialEq)]
pub struct HourBudget {
    limit: u64,
    hour: u64,
    wakes: u32,
    tokens: u64,
}

impl HourBudget {
    /// A counter that is over budget once this hour's tokens reach `limit`.
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            hour: 0,
            wakes: 0,
            tokens: 0,
        }
    }

    fn hour_of(now_s: u64) -> u64 {
        now_s / 3600
    }

    fn roll(&mut self, now_s: u64) {
        let hour = Self::hour_of(now_s);
        if hour != self.hour {
            self.hour = hour;
            self.wakes = 0;
            self.tokens = 0;
        }
    }

    /// Count `tokens` of spend in the hour of `now_s`.
    pub fn spend(&mut self, now_s: u64, tokens: u64) {
        self.roll(now_s);
        self.tokens = self.tokens.saturating_add(tokens);
    }

    /// Count one wake in the hour of `now_s`.
    pub fn wake(&mut self, now_s: u64) {
        self.roll(now_s);
        self.wakes = self.wakes.saturating_add(1);
    }

    /// True once this hour's spend reaches the limit.
    pub fn over_budget(&self, now_s: u64) -> bool {
        self.tokens_this_hour(now_s) >= self.limit
    }

    /// Wakes counted in the hour of `now_s`.
    pub fn wakes_this_hour(&self, now_s: u64) -> u32 {
        if Self::hour_of(now_s) == self.hour {
            self.wakes
        } else {
            0
        }
    }

    /// Tokens spent in the hour of `now_s`.
    pub fn tokens_this_hour(&self, now_s: u64) -> u64 {
        if Self::hour_of(now_s) == self.hour {
            self.tokens
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "groups:\n  a:\n    driver:\n      model: m-small\n  b:\n    driver:\n      model: m-large\n  c:\n    name: plain\n";

    #[test]
    fn model_is_read_from_group_config() {
        let a = driver_for(YAML, "a").unwrap().unwrap();
        assert_eq!(a.model, Some("m-small".to_string()));
        let b = driver_for(YAML, "b").unwrap().unwrap();
        assert_eq!(b.model, Some("m-large".to_string()));
    }

    #[test]
    fn a_group_without_driver_has_none() {
        assert_eq!(driver_for(YAML, "c").unwrap(), None);
        assert_eq!(driver_for(YAML, "missing").unwrap(), None);
    }

    #[test]
    fn an_unknown_cadence_knob_is_refused() {
        let yaml = "groups:\n  a:\n    driver:\n      cadence:\n        bogus: 5\n";
        let err = driver_for(yaml, "a").unwrap_err();
        assert!(err.contains("group `a`"), "{err}");
        assert!(err.contains("unknown cadence knob `bogus`"), "{err}");
    }

    #[test]
    fn an_allowed_cadence_knob_is_kept() {
        let yaml = "groups:\n  a:\n    driver:\n      cadence:\n        poll_seconds: 30\n";
        let cfg = driver_for(yaml, "a").unwrap().unwrap();
        assert_eq!(cfg.cadence.get("poll_seconds"), Some(&30));
    }

    #[test]
    fn zero_escalation_minutes_is_refused() {
        let yaml = "groups:\n  a:\n    driver:\n      escalate_after_min: 0\n";
        let err = driver_for(yaml, "a").unwrap_err();
        assert!(err.contains("group `a`"), "{err}");
        assert!(err.contains("at least 1"), "{err}");
    }

    #[test]
    fn escalate_after_defaults_to_ten() {
        let yaml = "groups:\n  a:\n    driver:\n      model: m-small\n";
        let cfg = driver_for(yaml, "a").unwrap().unwrap();
        assert_eq!(cfg.escalate_after_min, 10);
        assert_eq!(cfg.budget_tokens_per_hour, 0);
    }

    #[test]
    fn over_budget_driver_stops_waking() {
        let mut b = HourBudget::new(100);
        b.spend(0, 99);
        assert!(!b.over_budget(0));
        b.spend(0, 1);
        assert!(b.over_budget(0));
    }

    #[test]
    fn a_new_hour_resets_the_counters() {
        let mut b = HourBudget::new(100);
        b.spend(0, 100);
        b.wake(0);
        assert!(b.over_budget(0));
        assert_eq!(b.wakes_this_hour(0), 1);
        assert_eq!(b.tokens_this_hour(3600), 0);
        assert_eq!(b.wakes_this_hour(3600), 0);
        assert!(!b.over_budget(3600));
    }

    #[test]
    fn wakes_are_counted_per_hour() {
        let mut b = HourBudget::new(100);
        b.wake(10);
        b.wake(20);
        b.wake(30);
        assert_eq!(b.wakes_this_hour(30), 3);
        b.wake(3600);
        assert_eq!(b.wakes_this_hour(3600), 1);
    }
}
