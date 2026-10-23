//! Group driver: wakes only inside a verb (`on_digest`, `on_escalation`),
//! answers members from rules, escalates what the rules cannot settle, and
//! keeps a small context. Rules, wake gate and outbox are traits, so the
//! driver is tested with fakes.

pub mod escalate;

use escalate::{Chain, Escalation, Target};

/// How many digest lines the driver keeps as context.
pub const CONTEXT_CAP: usize = 50;

/// What the rules decided for one digest line.
pub enum Decision {
    /// Answer the member with this text.
    Reply(String),
    /// Raise an escalation; `tried` says what was already attempted.
    Escalate {
        /// What was already attempted.
        tried: String,
    },
    /// Nothing to do.
    Idle,
}

/// Decides what to do with a digest line.
pub trait RuleSource {
    /// Decide for one line from member `key` with `text`.
    fn decide(&mut self, key: &str, text: &str) -> Decision;
}

/// Says whether the driver may act now.
pub trait WakeGate {
    /// Whether a wake is allowed at `now_s` seconds.
    fn allow(&mut self, now_s: u64) -> bool;
}

/// Where the driver sends its output.
pub trait Outbox {
    /// Send a reply to a member.
    fn to_member(&mut self, key: &str, text: &str);
    /// Send an escalation line to a level.
    fn to_level(&mut self, target: &Target, line: &str);
    /// Send text to the board.
    fn to_board(&mut self, text: &str);
}

/// The group driver. It acts only inside its verbs.
pub struct Driver<R, G, O> {
    rules: R,
    gate: G,
    out: O,
    chain: Chain,
    has_ai: bool,
    context: Vec<(String, String)>,
    seq: u64,
}

impl<R: RuleSource, G: WakeGate, O: Outbox> Driver<R, G, O> {
    /// A driver over the given rules, wake gate, outbox and chain.
    /// `has_ai` false sends every digest to the board.
    pub fn new(rules: R, gate: G, out: O, chain: Chain, has_ai: bool) -> Self {
        Self {
            rules,
            gate,
            out,
            chain,
            has_ai,
            context: Vec::new(),
            seq: 0,
        }
    }

    /// The escalation chain.
    pub fn chain(&mut self) -> &mut Chain {
        &mut self.chain
    }

    /// The last digest kept as context, at most `CONTEXT_CAP` lines.
    pub fn context(&self) -> &[(String, String)] {
        &self.context
    }

    /// Handle a digest of `(key, text)` lines.
    pub fn on_digest(&mut self, now_s: u64, lines: &[(String, String)]) {
        if !self.gate.allow(now_s) {
            self.deny(now_s, &format!("{} digest lines", lines.len()));
            return;
        }
        let start = lines.len().saturating_sub(CONTEXT_CAP);
        self.context = lines[start..].to_vec();
        if !self.has_ai {
            let text = lines
                .iter()
                .map(|(k, t)| format!("{k}: {t}"))
                .collect::<Vec<_>>()
                .join("\n");
            self.out.to_board(&text);
            return;
        }
        for (key, text) in lines {
            match self.rules.decide(key, text) {
                Decision::Reply(reply) => self.out.to_member(key, &reply),
                Decision::Escalate { tried } => {
                    let esc = Escalation {
                        what: text.clone(),
                        who: key.clone(),
                        tried,
                    };
                    self.escalate(now_s, key, esc);
                }
                Decision::Idle => {}
            }
        }
    }

    /// Handle an escalation raised by `from`.
    pub fn on_escalation(&mut self, now_s: u64, from: &str, esc: Escalation) {
        if !self.gate.allow(now_s) {
            self.deny(now_s, "1 escalation");
            return;
        }
        self.escalate(now_s, from, esc);
    }

    fn escalate(&mut self, now_s: u64, from: &str, esc: Escalation) {
        self.seq += 1;
        let id = format!("{from}/{}", self.seq);
        let line = esc.line();
        let target = self.chain.raise(&id, esc, now_s);
        self.out.to_level(&target, &line);
    }

    fn deny(&mut self, now_s: u64, detail: &str) {
        self.out
            .to_board(&format!("wake denied at {now_s}s: {detail} not handled"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Log = Rc<RefCell<Vec<String>>>;

    fn new_log() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    fn calls(log: &Log) -> Vec<String> {
        log.borrow().clone()
    }

    fn note(log: &Log, s: String) {
        log.borrow_mut().push(s);
    }

    struct FakeRules {
        log: Log,
    }

    impl RuleSource for FakeRules {
        fn decide(&mut self, key: &str, text: &str) -> Decision {
            note(&self.log, format!("rule:{key}"));
            if text.starts_with("routine") {
                Decision::Reply(format!("re:{text}"))
            } else if text.starts_with("unknown") {
                Decision::Escalate {
                    tried: "auto".to_string(),
                }
            } else {
                Decision::Idle
            }
        }
    }

    struct FakeGate {
        log: Log,
        allowed: bool,
    }

    impl WakeGate for FakeGate {
        fn allow(&mut self, _now_s: u64) -> bool {
            note(&self.log, "gate".to_string());
            self.allowed
        }
    }

    struct FakeOutbox {
        log: Log,
    }

    impl Outbox for FakeOutbox {
        fn to_member(&mut self, key: &str, text: &str) {
            note(&self.log, format!("member:{key}:{text}"));
        }
        fn to_level(&mut self, target: &Target, line: &str) {
            note(
                &self.log,
                format!("level:{}:{}:{line}", target.level, target.name),
            );
        }
        fn to_board(&mut self, text: &str) {
            note(&self.log, format!("board:{text}"));
        }
    }

    type Fake = Driver<FakeRules, FakeGate, FakeOutbox>;

    fn driver(log: &Log, allowed: bool, has_ai: bool) -> Fake {
        Driver::new(
            FakeRules { log: log.clone() },
            FakeGate {
                log: log.clone(),
                allowed,
            },
            FakeOutbox { log: log.clone() },
            Chain::new(vec!["lead".to_string(), "maintainer".to_string()], 10),
            has_ai,
        )
    }

    fn digest(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, t)| (k.to_string(), t.to_string()))
            .collect()
    }

    #[test]
    fn driver_wakes_only_on_digest_or_escalation() {
        let log = new_log();
        let mut d = driver(&log, true, true);
        assert!(calls(&log).is_empty());
        d.on_digest(1, &digest(&[("m1", "routine ask")]));
        assert_eq!(calls(&log)[0], "gate");
    }

    #[test]
    fn driver_does_not_poll() {
        let log = new_log();
        let mut d = driver(&log, true, true);
        d.on_digest(1, &digest(&[("m1", "routine ask")]));
        let after = calls(&log);
        assert_eq!(after, vec!["gate", "rule:m1", "member:m1:re:routine ask"]);
        assert_eq!(calls(&log), after);
    }

    #[test]
    fn driver_context_stays_within_cap() {
        let log = new_log();
        let mut d = driver(&log, true, true);
        for n in 1u64..=3 {
            let lines: Vec<(String, String)> = (0..80)
                .map(|i| (format!("k{i}"), format!("d{n}-{i}")))
                .collect();
            d.on_digest(n, &lines);
        }
        let ctx = d.context();
        assert_eq!(ctx.len(), 50);
        assert!(ctx.iter().all(|(_, t)| t.starts_with("d3-")));
        assert_eq!(ctx[0].1, "d3-30");
        assert_eq!(ctx[49].1, "d3-79");
    }

    #[test]
    fn routine_case_gets_a_member_reply_and_no_escalation() {
        let log = new_log();
        let mut d = driver(&log, true, true);
        d.on_digest(1, &digest(&[("m1", "routine ask")]));
        assert_eq!(
            calls(&log),
            vec!["gate", "rule:m1", "member:m1:re:routine ask"]
        );
        assert_eq!(d.chain().open_len(), 0);
    }

    #[test]
    fn unknown_case_escalates_one_level() {
        let log = new_log();
        let mut d = driver(&log, true, true);
        d.on_digest(1, &digest(&[("m2", "unknown thing")]));
        assert_eq!(
            calls(&log),
            vec!["gate", "rule:m2", "level:1:lead:unknown thing | m2 | auto"]
        );
        assert_eq!(d.chain().open_len(), 1);
    }

    #[test]
    fn no_ai_group_sends_overview_to_board() {
        let log = new_log();
        let mut d = driver(&log, true, false);
        d.on_digest(1, &digest(&[("k1", "t1"), ("k2", "t2")]));
        assert_eq!(calls(&log), vec!["gate", "board:k1: t1\nk2: t2"]);
    }

    #[test]
    fn a_denied_wake_sends_an_overview_to_the_board_only() {
        let log = new_log();
        let mut d = driver(&log, false, true);
        d.on_digest(5, &digest(&[("m1", "routine ask"), ("m2", "unknown")]));
        assert_eq!(
            calls(&log),
            vec![
                "gate",
                "board:wake denied at 5s: 2 digest lines not handled"
            ]
        );
        d.on_escalation(
            7,
            "m1",
            Escalation {
                what: "q".to_string(),
                who: "m1".to_string(),
                tried: "auto".to_string(),
            },
        );
        assert_eq!(
            calls(&log)[3],
            "board:wake denied at 7s: 1 escalation not handled"
        );
        assert_eq!(calls(&log).len(), 4);
    }
}
