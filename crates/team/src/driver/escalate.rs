//! Escalation chain: a case the members cannot settle climbs one level at a
//! time, and a level left unanswered for `after_min` minutes climbs again.
//! The top level is the maintainer (the board) and never overflows.

use std::collections::BTreeMap;

/// Longest line `Escalation::line` returns, in characters.
const LINE_CAP: usize = 200;

/// The top level's name: the board.
pub const TOP: &str = "maintainer";

/// A case the group could not settle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escalation {
    /// What the case is about.
    pub what: String,
    /// Who raised it.
    pub who: String,
    /// What was already tried.
    pub tried: String,
}

impl Escalation {
    /// One short line: `what | who | tried`, no newlines, at most 200 chars.
    pub fn line(&self) -> String {
        format!("{} | {} | {}", self.what, self.who, self.tried)
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .take(LINE_CAP)
            .collect()
    }
}

/// Where an escalation goes. `level` 1 is the first level up from the group;
/// `level` equal to the number of levels is the top (the maintainer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Position in the chain, 1 being the first level up.
    pub level: usize,
    /// Name of the level.
    pub name: String,
}

struct Open {
    esc: Escalation,
    level: usize,
    since_s: u64,
}

/// The escalation chain: open escalations and the levels above the group.
pub struct Chain {
    levels: Vec<String>,
    after_min: u64,
    open: BTreeMap<String, Open>,
}

impl Chain {
    /// `levels` are the levels above the group, lowest first. The chain always
    /// ends at the maintainer, which is appended when missing.
    pub fn new(mut levels: Vec<String>, after_min: u64) -> Self {
        if levels.last().map(String::as_str) != Some(TOP) {
            levels.push(TOP.to_string());
        }
        Self {
            levels,
            after_min,
            open: BTreeMap::new(),
        }
    }

    fn target(&self, level: usize) -> Target {
        Target {
            level,
            name: self.levels[level - 1].clone(),
        }
    }

    /// Opens an escalation at the first level up from the group.
    pub fn raise(&mut self, id: &str, esc: Escalation, now_s: u64) -> Target {
        self.open.insert(
            id.to_string(),
            Open {
                esc,
                level: 1,
                since_s: now_s,
            },
        );
        self.target(1)
    }

    /// Re-sends every unanswered escalation one level up once `after_min`
    /// minutes have passed since it was raised or last moved. The top stays top.
    pub fn tick(&mut self, now_s: u64) -> Vec<(String, Target)> {
        let wait = self.after_min.saturating_mul(60);
        let top = self.levels.len();
        let levels = &self.levels;
        let mut sent = Vec::new();
        for (id, o) in self.open.iter_mut() {
            if now_s.saturating_sub(o.since_s) < wait {
                continue;
            }
            o.level = (o.level + 1).min(top);
            o.since_s = now_s;
            let name = levels[o.level - 1].clone();
            sent.push((
                id.clone(),
                Target {
                    level: o.level,
                    name,
                },
            ));
        }
        sent
    }

    /// Closes an escalation: nothing more goes up for it.
    pub fn answer(&mut self, id: &str) {
        self.open.remove(id);
    }

    /// The open escalation with this id, if any.
    pub fn pending(&self, id: &str) -> Option<&Escalation> {
        self.open.get(id).map(|o| &o.esc)
    }

    /// How many escalations are open.
    pub fn open_len(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc(what: &str) -> Escalation {
        Escalation {
            what: what.to_string(),
            who: "m1".to_string(),
            tried: "auto".to_string(),
        }
    }

    fn chain() -> Chain {
        Chain::new(vec!["lead".to_string(), "maintainer".to_string()], 10)
    }

    fn tgt(level: usize, name: &str) -> Target {
        Target {
            level,
            name: name.to_string(),
        }
    }

    #[test]
    fn line_format_is_what_who_tried() {
        assert_eq!(esc("boom").line(), "boom | m1 | auto");
    }

    #[test]
    fn escalation_line_is_one_short_line() {
        let e = Escalation {
            what: "a\nb\r\nc".to_string(),
            who: "m1".to_string(),
            tried: "x".repeat(300),
        };
        let l = e.line();
        assert!(!l.contains('\n') && !l.contains('\r'));
        assert_eq!(l.chars().count(), 200);
        assert!(l.starts_with("abc | m1 | "));
    }

    #[test]
    fn unanswered_escalation_goes_up_after_n_minutes() {
        let mut c = chain();
        assert_eq!(c.raise("a", esc("q"), 0), tgt(1, "lead"));
        assert!(c.tick(599).is_empty());
        assert_eq!(c.tick(600), vec![("a".to_string(), tgt(2, "maintainer"))]);

        let mut c2 = chain();
        c2.raise("b", esc("q"), 0);
        c2.answer("b");
        assert!(c2.tick(600).is_empty());
        assert_eq!(c2.open_len(), 0);
    }

    #[test]
    fn top_level_escalation_reaches_maintainer() {
        let mut c = chain();
        c.raise("a", esc("q"), 0);
        assert_eq!(c.tick(600), vec![("a".to_string(), tgt(2, "maintainer"))]);
        assert_eq!(c.tick(1200), vec![("a".to_string(), tgt(2, "maintainer"))]);
        assert_eq!(c.pending("a").map(|e| e.what.as_str()), Some("q"));
    }

    #[test]
    fn chain_always_ends_at_maintainer() {
        let mut c = Chain::new(vec!["lead".to_string()], 1);
        c.raise("a", esc("q"), 0);
        assert_eq!(c.tick(60), vec![("a".to_string(), tgt(2, "maintainer"))]);
    }
}
