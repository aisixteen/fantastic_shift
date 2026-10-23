//! Group driver rulebook: one rule per line, matched against a digest line or a
//! worker report. Each (case key, rule) pair has a retry counter; the first
//! `retries` matches reply, after that the case escalates.

use std::collections::HashMap;

const DEFAULT_RETRIES: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    trigger: String,
    reply: String,
    retries: u32,
}

/// Outcome of matching a case against the rulebook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A rule matched and its retry budget is not spent: send this reply.
    Reply(String),
    /// Escalate the case.
    Escalate {
        /// The matched trigger, if any rule matched.
        rule: Option<String>,
        /// Number of replies already sent for the matched rule.
        tries: u32,
    },
    /// The rulebook has no rules at all.
    NoRule,
}

/// A parsed set of routine-reply rules plus their per-case retry counters.
#[derive(Debug, Default)]
pub struct Rulebook {
    rules: Vec<Rule>,
    counters: HashMap<(String, usize), u32>,
}

impl Rulebook {
    /// Parse `<trigger> => <reply> | retries=<n>` lines. `#` comments and blank
    /// lines are skipped. A bad line yields an error naming its line number.
    pub fn parse(src: &str) -> Result<Rulebook, String> {
        let mut rules = Vec::new();
        for (i, raw) in src.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let rule = parse_line(line).map_err(|e| format!("line {}: {e}", i + 1))?;
            rules.push(rule);
        }
        Ok(Rulebook {
            rules,
            counters: HashMap::new(),
        })
    }

    /// Decide the reply for case `key` given `text`. The first matching rule in
    /// file order wins. Its counter advances on each reply; once it reaches the
    /// rule's `retries` limit, every further match escalates.
    pub fn decide(&mut self, key: &str, text: &str) -> Verdict {
        if self.rules.is_empty() {
            return Verdict::NoRule;
        }
        let hay = text.to_lowercase();
        let Some(idx) = self.rules.iter().position(|r| hay.contains(&r.trigger)) else {
            return Verdict::Escalate {
                rule: None,
                tries: 0,
            };
        };
        let rule = &self.rules[idx];
        let slot = self.counters.entry((key.to_string(), idx)).or_insert(0);
        if *slot < rule.retries {
            *slot += 1;
            Verdict::Reply(rule.reply.clone())
        } else {
            Verdict::Escalate {
                rule: Some(rule.trigger.clone()),
                tries: *slot,
            }
        }
    }

    /// Clear every retry counter belonging to `key`.
    pub fn reset(&mut self, key: &str) {
        self.counters.retain(|(k, _), _| k != key);
    }
}

fn parse_line(line: &str) -> Result<Rule, String> {
    let (trigger, rest) = line.split_once("=>").ok_or("missing '=>'")?;
    let trigger = trigger.trim();
    if trigger.is_empty() {
        return Err("empty trigger".into());
    }
    let (reply, retries) = match rest.rsplit_once('|') {
        Some((reply, opt)) => {
            let opt = opt.trim();
            let value = opt
                .strip_prefix("retries=")
                .ok_or_else(|| format!("unknown option '{opt}'"))?;
            let n = value
                .trim()
                .parse::<u32>()
                .map_err(|_| format!("bad retries '{}'", value.trim()))?;
            (reply, n)
        }
        None => (rest, DEFAULT_RETRIES),
    };
    let reply = reply.trim();
    if reply.is_empty() {
        return Err("empty reply".into());
    }
    Ok(Rule {
        trigger: trigger.to_lowercase(),
        reply: reply.to_string(),
        retries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(src: &str) -> Rulebook {
        Rulebook::parse(src).unwrap()
    }

    #[test]
    fn half_done_worker_gets_finish_reply() {
        let mut b = book("stopped half-done => finish the task");
        assert_eq!(
            b.decide("w1/step", "worker w1 stopped half-done"),
            Verdict::Reply("finish the task".into())
        );
    }

    #[test]
    fn known_build_race_is_retried() {
        let mut b = book("build race => retry the step");
        assert_eq!(
            b.decide("w1/build", "build race detected"),
            Verdict::Reply("retry the step".into())
        );
    }

    #[test]
    fn tool_path_mixup_gets_hint() {
        let mut b = book("wrong tool path => use files.read with a relative path");
        assert_eq!(
            b.decide("w2/read", "error: wrong tool path /abs"),
            Verdict::Reply("use files.read with a relative path".into())
        );
    }

    #[test]
    fn rule_past_retry_limit_escalates() {
        let mut b = book("build race => retry the step | retries=2");
        let k = "w1/build";
        assert_eq!(
            b.decide(k, "build race"),
            Verdict::Reply("retry the step".into())
        );
        assert_eq!(
            b.decide(k, "build race"),
            Verdict::Reply("retry the step".into())
        );
        assert_eq!(
            b.decide(k, "build race"),
            Verdict::Escalate {
                rule: Some("build race".into()),
                tries: 2
            }
        );
        assert_eq!(
            b.decide(k, "build race"),
            Verdict::Escalate {
                rule: Some("build race".into()),
                tries: 2
            }
        );
    }

    #[test]
    fn unknown_case_does_not_match_any_rule() {
        let mut b = book("build race => retry the step");
        assert_eq!(
            b.decide("w1", "something else entirely"),
            Verdict::Escalate {
                rule: None,
                tries: 0
            }
        );
    }

    #[test]
    fn counters_are_per_key() {
        let mut b = book("race => retry | retries=1");
        assert_eq!(b.decide("w1", "race"), Verdict::Reply("retry".into()));
        assert_eq!(
            b.decide("w1", "race"),
            Verdict::Escalate {
                rule: Some("race".into()),
                tries: 1
            }
        );
        assert_eq!(b.decide("w2", "race"), Verdict::Reply("retry".into()));
    }

    #[test]
    fn reset_clears_the_counter() {
        let mut b = book("race => retry | retries=1");
        assert_eq!(b.decide("w1", "race"), Verdict::Reply("retry".into()));
        assert_eq!(
            b.decide("w1", "race"),
            Verdict::Escalate {
                rule: Some("race".into()),
                tries: 1
            }
        );
        b.reset("w1");
        assert_eq!(b.decide("w1", "race"), Verdict::Reply("retry".into()));
    }

    #[test]
    fn retries_zero_escalates_at_once() {
        let mut b = book("x => y | retries=0");
        assert_eq!(
            b.decide("k", "x"),
            Verdict::Escalate {
                rule: Some("x".into()),
                tries: 0
            }
        );
    }

    #[test]
    fn matching_is_case_insensitive() {
        let mut b = book("Build Race => retry the step");
        assert_eq!(
            b.decide("k", "BUILD RACE now"),
            Verdict::Reply("retry the step".into())
        );
    }

    #[test]
    fn first_rule_in_file_order_wins() {
        let mut b = book("race => A\nbuild race => B");
        assert_eq!(b.decide("k", "build race"), Verdict::Reply("A".into()));
    }

    #[test]
    fn a_malformed_line_names_its_line_number() {
        let err = Rulebook::parse("ok => fine\nbroken line").unwrap_err();
        assert!(err.contains("line 2"), "wrong error: {err}");
        let err = Rulebook::parse("x => y | retries=abc").unwrap_err();
        assert!(err.contains("line 1"), "wrong error: {err}");
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let mut b = book("# a comment\n\n   \nbuild race => retry the step\n# tail");
        assert_eq!(
            b.decide("k", "build race"),
            Verdict::Reply("retry the step".into())
        );
    }

    #[test]
    fn an_empty_rulebook_yields_no_rule() {
        let mut b = book("");
        assert_eq!(b.decide("k", "anything"), Verdict::NoRule);
        let mut c = book("# only a comment");
        assert_eq!(c.decide("k", "anything"), Verdict::NoRule);
    }
}
