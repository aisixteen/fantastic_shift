//! Cadencer feed: the pure data side of the dispatcher's cadencer. It turns the
//! board's rows into the tick-payload tree, plans the digest deliveries, and
//! meters what the digests and the driver cost per hour. Nothing here reads a
//! clock, sends a message or touches the store.

use serde_json::{json, Value};

/// One message the cadencer sends: to an agent, with the digest text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// The agent that receives the digest.
    pub to: String,
    /// The digest lines joined by `\n`.
    pub text: String,
}

/// Seconds the cost meter reports over.
const WINDOW_S: u64 = 3600;

/// Convert the board's rows into the tick-payload node shape. Missing fields
/// take neutral defaults; a row without a string `id` is skipped.
pub fn tree_from_board(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .filter_map(|row| {
            let id = row.get("id")?.as_str()?;
            Some(json!({
                "id": id,
                "parent": row.get("parent").cloned().unwrap_or(Value::Null),
                "has_ai": row.get("has_ai").and_then(Value::as_bool).unwrap_or(false),
                "state": row
                    .get("state")
                    .or_else(|| row.get("status"))
                    .cloned()
                    .unwrap_or(json!("")),
                "task": row.get("task").cloned().unwrap_or(json!("")),
                "idle_s": row.get("idle_s").cloned().unwrap_or(json!(0)),
                "time_on_task_s": row.get("time_on_task_s").cloned().unwrap_or(json!(0)),
                "cost": row.get("cost").cloned().unwrap_or(json!(0.0)),
                "cache_hit": row.get("cache_hit").and_then(Value::as_bool).unwrap_or(false),
                "new_results": row.get("new_results").cloned().unwrap_or(json!([])),
                "driver": row.get("driver").cloned().unwrap_or(json!("")),
            }))
        })
        .collect()
}

fn parent_of(n: &Value) -> Option<&str> {
    n.get("parent").and_then(Value::as_str)
}

fn id_of(n: &Value) -> &str {
    n.get("id").and_then(Value::as_str).unwrap_or("")
}

/// The digest lines a group's driver reads: one line per direct child of
/// `group`. A child with members of its own is one rollup line with its head
/// count, so nothing two levels down is named.
pub fn digest_lines(nodes: &[Value], group: &str) -> Vec<String> {
    nodes
        .iter()
        .filter(|n| parent_of(n) == Some(group))
        .map(|child| {
            let id = id_of(child);
            let below = nodes.iter().filter(|n| parent_of(n) == Some(id)).count();
            if below == 0 {
                let state = child.get("state").and_then(Value::as_str).unwrap_or("");
                format!("{id}: {state}")
            } else {
                format!("{id}: subgroup with {below} members")
            }
        })
        .collect()
}

/// Plan one delivery per digest. A group with an AI driver sends to that
/// driver; a group without one sends to `board` and `maintainer`. A digest
/// with no lines sends nothing.
pub fn plan_delivery(
    digests: &[(String, Vec<String>)],
    has_ai: &dyn Fn(&str) -> bool,
    driver_of: &dyn Fn(&str) -> String,
) -> Vec<Delivery> {
    let mut out = Vec::new();
    for (group, lines) in digests {
        if lines.is_empty() {
            continue;
        }
        let text = lines.join("\n");
        if has_ai(group) {
            out.push(Delivery {
                to: driver_of(group),
                text,
            });
        } else {
            for to in ["board", "maintainer"] {
                out.push(Delivery {
                    to: to.into(),
                    text: text.clone(),
                });
            }
        }
    }
    out
}

/// Digest bytes and driver tokens over the last hour. Times are passed in.
#[derive(Debug, Clone, Default)]
pub struct CostMeter {
    digests: Vec<(u64, u64)>,
    tokens: Vec<(u64, u64)>,
}

fn in_window(at: u64, now_s: u64) -> bool {
    now_s.saturating_sub(at) < WINDOW_S
}

fn window_sum(entries: &[(u64, u64)], now_s: u64) -> u64 {
    entries
        .iter()
        .filter(|(at, _)| in_window(*at, now_s))
        .map(|(_, v)| v)
        .sum()
}

impl CostMeter {
    /// An empty meter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one digest of `bytes` sent at `now_s`.
    pub fn record_digest(&mut self, now_s: u64, bytes: u64) {
        self.digests.push((now_s, bytes));
        self.digests.retain(|(at, _)| in_window(*at, now_s));
    }

    /// Record `tokens` spent by the driver at `now_s`.
    pub fn record_tokens(&mut self, now_s: u64, tokens: u64) {
        self.tokens.push((now_s, tokens));
        self.tokens.retain(|(at, _)| in_window(*at, now_s));
    }

    /// The per-hour figures as of `now_s`.
    pub fn status(&self, now_s: u64) -> Value {
        json!({
            "digest_bytes_per_hour": window_sum(&self.digests, now_s),
            "driver_tokens_per_hour": window_sum(&self.tokens, now_s),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatcher_builds_tree_from_board_data() {
        let rows = vec![
            json!({"id": "g1", "parent": null, "task": "group", "idle_s": 5}),
            json!({"id": "w1", "parent": "g1", "task": "build", "idle_s": 42}),
        ];
        let nodes = tree_from_board(&rows);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1]["id"], "w1");
        assert_eq!(nodes[1]["parent"], "g1");
        assert_eq!(nodes[1]["task"], "build");
        assert_eq!(nodes[1]["idle_s"], 42);
    }

    #[test]
    fn digest_goes_to_group_driver() {
        let has_ai = |g: &str| g == "g1";
        let driver = |_: &str| "d1".to_string();
        let digests = vec![(
            "g1".to_string(),
            vec!["a: x".to_string(), "b: y".to_string()],
        )];
        let plan = plan_delivery(&digests, &has_ai, &driver);
        assert_eq!(
            plan,
            vec![Delivery {
                to: "d1".into(),
                text: "a: x\nb: y".into(),
            }]
        );
    }

    #[test]
    fn no_ai_group_digest_goes_to_board_and_maintainer() {
        let has_ai = |_: &str| false;
        let driver = |_: &str| "d1".to_string();
        let digests = vec![("g2".to_string(), vec!["a: x".to_string()])];
        let plan = plan_delivery(&digests, &has_ai, &driver);
        assert_eq!(
            plan,
            vec![
                Delivery {
                    to: "board".into(),
                    text: "a: x".into(),
                },
                Delivery {
                    to: "maintainer".into(),
                    text: "a: x".into(),
                },
            ]
        );
    }

    #[test]
    fn empty_digest_is_not_sent() {
        let has_ai = |_: &str| true;
        let driver = |_: &str| "d1".to_string();
        let digests = vec![("g1".to_string(), Vec::<String>::new())];
        assert!(plan_delivery(&digests, &has_ai, &driver).is_empty());
    }

    #[test]
    fn subgroup_is_one_rollup_line() {
        let rows = vec![
            json!({"id": "g", "parent": null}),
            json!({"id": "s", "parent": "g"}),
            json!({"id": "w1", "parent": "s", "state": "busy"}),
            json!({"id": "w2", "parent": "s", "state": "busy"}),
            json!({"id": "w3", "parent": "s", "state": "idle"}),
        ];
        let nodes = tree_from_board(&rows);
        let lines = digest_lines(&nodes, "g");
        assert_eq!(lines, vec!["s: subgroup with 3 members".to_string()]);
    }

    #[test]
    fn driver_never_sees_two_levels_down() {
        let rows = vec![
            json!({"id": "g", "parent": null}),
            json!({"id": "s", "parent": "g"}),
            json!({"id": "w1", "parent": "s", "state": "busy"}),
            json!({"id": "w2", "parent": "s", "state": "busy"}),
            json!({"id": "w3", "parent": "s", "state": "idle"}),
        ];
        let nodes = tree_from_board(&rows);
        let lines = digest_lines(&nodes, "g");
        let has_ai = |g: &str| g == "g";
        let driver = |_: &str| "d1".to_string();
        let digests = vec![("g".to_string(), lines)];
        let plan = plan_delivery(&digests, &has_ai, &driver);
        assert_eq!(plan.len(), 1);
        for w in ["w1", "w2", "w3"] {
            assert!(!plan[0].text.contains(w), "driver saw {w}");
        }
    }

    #[test]
    fn cost_per_hour_is_reported_in_status() {
        let mut m = CostMeter::new();
        m.record_digest(1000, 100);
        m.record_digest(1000, 50);
        m.record_tokens(1000, 2000);
        assert_eq!(
            m.status(1000),
            json!({"digest_bytes_per_hour": 150, "driver_tokens_per_hour": 2000})
        );
        assert_eq!(
            m.status(1000 + 3601),
            json!({"digest_bytes_per_hour": 0, "driver_tokens_per_hour": 0})
        );
    }
}
