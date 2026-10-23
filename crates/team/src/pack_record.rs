//! A pack's measured record (#41): success rate and cost per task from real
//! shifts, signed by the measuring shift. This module does local aggregation,
//! the canonical signing payload and the opt-in gate. It publishes nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};

/// One task's outcome, as the measuring shift saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskOutcome {
    /// Hash of the pack the task ran under.
    pub pack_hash: String,
    /// Whether the task succeeded.
    pub succeeded: bool,
    /// Cost of the task, when the shift reported one.
    pub cost: Option<f64>,
    /// Wall-clock seconds the task took.
    pub elapsed_s: f64,
}

/// A pack's measured record. It carries only these fields: no paths, task
/// text, repo names or secrets.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredRecord {
    /// Hash of the measured pack.
    pub pack_hash: String,
    /// Number of tasks measured for this pack.
    pub tasks: u64,
    /// Number of those tasks that succeeded.
    pub successes: u64,
    /// `successes / tasks`, or 0.0 when there are no tasks.
    pub success_rate: f64,
    /// Mean reported cost over the tasks that reported one; None if none did.
    pub cost_per_task: Option<f64>,
    /// Team id of the shift that measured the pack.
    pub measured_by: String,
    /// Signature over [`signing_payload`]; None until the record is signed.
    pub signature: Option<String>,
}

/// Aggregate the outcomes that belong to `pack_hash` into a record. Outcomes
/// for other packs are ignored. The record is unsigned.
pub fn aggregate(pack_hash: &str, outcomes: &[TaskOutcome], measured_by: &str) -> MeasuredRecord {
    let mine: Vec<&TaskOutcome> = outcomes
        .iter()
        .filter(|o| o.pack_hash == pack_hash)
        .collect();
    let tasks = mine.len() as u64;
    let successes = mine.iter().filter(|o| o.succeeded).count() as u64;
    let success_rate = if tasks == 0 {
        0.0
    } else {
        successes as f64 / tasks as f64
    };
    let costs: Vec<f64> = mine.iter().filter_map(|o| o.cost).collect();
    let cost_per_task = if costs.is_empty() {
        None
    } else {
        Some(costs.iter().sum::<f64>() / costs.len() as f64)
    };
    MeasuredRecord {
        pack_hash: pack_hash.to_string(),
        tasks,
        successes,
        success_rate,
        cost_per_task,
        measured_by: measured_by.to_string(),
        signature: None,
    }
}

/// The canonical bytes a signature covers: a JSON object with sorted keys and
/// every field except `signature`. The output is deterministic.
pub fn signing_payload(rec: &MeasuredRecord) -> Vec<u8> {
    let mut m: BTreeMap<&str, Value> = BTreeMap::new();
    m.insert("pack_hash", json!(rec.pack_hash));
    m.insert("tasks", json!(rec.tasks));
    m.insert("successes", json!(rec.successes));
    m.insert("success_rate", json!(rec.success_rate));
    m.insert("cost_per_task", json!(rec.cost_per_task));
    m.insert("measured_by", json!(rec.measured_by));
    serde_json::to_vec(&m).unwrap_or_default()
}

/// The record if it may be published: only when the owner opted in AND the
/// record is signed. Otherwise None.
pub fn publishable(rec: &MeasuredRecord, opt_in: bool) -> Option<MeasuredRecord> {
    if opt_in && rec.signature.is_some() {
        Some(rec.clone())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(pack: &str, succeeded: bool, cost: Option<f64>) -> TaskOutcome {
        TaskOutcome {
            pack_hash: pack.to_string(),
            succeeded,
            cost,
            elapsed_s: 1.0,
        }
    }

    #[test]
    fn aggregate_counts_only_this_pack() {
        let outs = vec![
            outcome("p", true, None),
            outcome("q", true, None),
            outcome("q", false, None),
        ];
        let rec = aggregate("p", &outs, "team-a");
        assert_eq!(rec.tasks, 1);
        assert_eq!(rec.successes, 1);
        assert_eq!(rec.pack_hash, "p");
        assert_eq!(rec.measured_by, "team-a");
        assert!(rec.signature.is_none());
    }

    #[test]
    fn rate_and_cost_per_task_are_right() {
        let outs = vec![
            outcome("p", true, Some(1.0)),
            outcome("p", false, Some(3.0)),
            outcome("p", true, None),
        ];
        let rec = aggregate("p", &outs, "team-a");
        assert_eq!(rec.tasks, 3);
        assert_eq!(rec.successes, 2);
        assert!((rec.success_rate - 2.0 / 3.0).abs() < 1e-9);
        // Mean over the two tasks that reported a cost.
        assert_eq!(rec.cost_per_task, Some(2.0));
    }

    #[test]
    fn cost_is_none_when_no_task_reported_cost() {
        let outs = vec![outcome("p", true, None), outcome("p", false, None)];
        let rec = aggregate("p", &outs, "team-a");
        assert_eq!(rec.cost_per_task, None);
    }

    #[test]
    fn an_empty_set_gives_zero_tasks_not_nan() {
        let rec = aggregate("p", &[], "team-a");
        assert_eq!(rec.tasks, 0);
        assert_eq!(rec.successes, 0);
        assert_eq!(rec.success_rate, 0.0);
        assert!(!rec.success_rate.is_nan());
        assert_eq!(rec.cost_per_task, None);
    }

    #[test]
    fn signing_payload_is_deterministic_and_excludes_signature() {
        let outs = vec![outcome("p", true, Some(0.5))];
        let mut rec = aggregate("p", &outs, "team-a");
        let unsigned = signing_payload(&rec);
        assert_eq!(unsigned, signing_payload(&rec));
        rec.signature = Some("sig-one".to_string());
        assert_eq!(unsigned, signing_payload(&rec));
        let text = String::from_utf8(unsigned).unwrap();
        assert!(!text.contains("signature"));
    }

    #[test]
    fn nothing_is_publishable_without_opt_in() {
        let mut rec = aggregate("p", &[outcome("p", true, None)], "team-a");
        rec.signature = Some("sig".to_string());
        assert!(publishable(&rec, false).is_none());
        assert!(publishable(&rec, true).is_some());
    }

    #[test]
    fn an_unsigned_record_is_not_publishable() {
        let rec = aggregate("p", &[outcome("p", true, None)], "team-a");
        assert!(publishable(&rec, true).is_none());
    }

    #[test]
    fn a_record_carries_no_task_text_or_paths() {
        let rec = aggregate("p", &[outcome("p", true, Some(1.0))], "team-a");
        let v: Value = serde_json::from_slice(&signing_payload(&rec)).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "cost_per_task",
                "measured_by",
                "pack_hash",
                "success_rate",
                "successes",
                "tasks",
            ]
        );
    }
}
