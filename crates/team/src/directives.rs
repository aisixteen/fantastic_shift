//! The directives agents of a team run from, as files. Two ship with the binary:
//! the worker's (claim a task file in the queue, do it, hand it back, repeat) and the
//! lead's (start workers from templates, keep them going, stop the ones that go
//! wrong, clean up). A config may name another file instead. Neither names a
//! provider, a model or a kind of work: a task's own instructions say what the work is.

use std::path::Path;

/// The worker directive shipped with the binary (`directives/worker.md`).
pub const WORKER: &str = include_str!("../../../directives/worker.md");

/// The lead directive shipped with the binary (`directives/lead.md`).
pub const LEAD: &str = include_str!("../../../directives/lead.md");

/// The issue-keeper directive shipped with the binary (`directives/issue_keeper.md`).
pub const ISSUE_KEEPER: &str = include_str!("../../../directives/issue_keeper.md");

/// Read a directive file. Its text, or why it cannot be used.
pub fn load(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read directive file {}: {e}", path.display()))?;
    if text.trim().is_empty() {
        return Err(format!("directive file {} is empty", path.display()));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    const DENY: [&str; 9] = [
        "claude",
        "haiku",
        "sonnet",
        "opus",
        "qwen",
        "gpt",
        "llama",
        "anthropic",
        "openai",
    ];

    #[test]
    fn the_defaults_are_not_empty() {
        assert!(!WORKER.trim().is_empty());
        assert!(!LEAD.trim().is_empty());
        assert!(WORKER.starts_with("# The worker directive"));
        assert!(LEAD.starts_with("# The lead directive"));
    }

    #[test]
    fn the_worker_claims_a_task_file_with_if_match() {
        for s in [
            "if_match",
            "version",
            "version_mismatch",
            ".state",
            "pending",
            "taken",
            "until",
            "done",
            "failed",
            "\"taker\":\"YOU\"",
            "<type>list</type>",
            "<type>read</type>",
            "<type>write</type>",
        ] {
            assert!(WORKER.contains(s), "worker directive lacks {s}");
        }
    }

    #[test]
    fn the_lead_reads_and_guards_the_task_files() {
        for s in [
            "if_match",
            "version",
            "version_mismatch",
            ".state",
            "taken",
            "<type>list</type>",
            "<type>read</type>",
            "<type>write</type>",
            "create_agent",
            "delete_agent",
            "update_agent",
            "wait_s",
        ] {
            assert!(LEAD.contains(s), "lead directive lacks {s}");
        }
    }

    #[test]
    fn no_directive_uses_the_old_queue_agent_verbs() {
        for (name, text) in [("worker", WORKER), ("lead", LEAD)] {
            for s in [
                "<type>take</type>",
                "<type>renew</type>",
                "<type>release</type>",
                "<type>done</type>",
            ] {
                assert!(!text.contains(s), "{name} directive still uses {s}");
            }
        }
    }

    #[test]
    fn the_directives_name_no_provider_or_model() {
        for (name, text) in [("worker", WORKER), ("lead", LEAD)] {
            for d in DENY {
                assert!(
                    !text.to_lowercase().contains(d),
                    "{name} directive names {d}"
                );
            }
        }
    }

    #[test]
    fn the_worker_names_no_forge() {
        let w = words(WORKER);
        assert!(!w.iter().any(|x| x == "git" || x == "github"));
    }

    #[test]
    fn the_rules_are_in_both() {
        for text in [WORKER, LEAD] {
            assert!(text.contains("wait_s"));
            assert!(text.contains("await"));
            assert!(text.contains("<call name=\"done\"></call>"));
        }
        assert!(LEAD.contains("generating"));
        assert!(LEAD.contains("fantastic#227"));
    }

    #[test]
    fn load_reads_a_file_and_refuses_empty_or_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let d = dir.path().join("d.md");
        std::fs::write(&d, "# x\nbody\n").unwrap();
        assert_eq!(load(&d).unwrap(), "# x\nbody\n");

        let e = dir.path().join("e.md");
        std::fs::write(&e, "  \n").unwrap();
        assert_eq!(
            load(&e).unwrap_err(),
            format!("directive file {} is empty", e.display())
        );

        let none = dir.path().join("none.md");
        assert!(load(&none)
            .unwrap_err()
            .starts_with(&format!("cannot read directive file {}: ", none.display())));
    }

    #[test]
    fn issue_keeper_directive_is_listed_and_nonempty() {
        assert!(!ISSUE_KEEPER.trim().is_empty());
        assert!(ISSUE_KEEPER.starts_with("# The issue-keeper directive"));
    }

    #[test]
    fn issue_keeper_directive_names_never_rules() {
        let text = ISSUE_KEEPER.to_lowercase();
        for s in ["never merge", "never close", "never edit", "never create"] {
            assert!(text.contains(s), "issue-keeper directive lacks '{s}'");
        }
        assert!(text.contains("spec"));
    }

    #[test]
    fn driver_directive_says_clear_before_a_new_task() {
        assert!(LEAD.contains("context_clear"));
        assert!(LEAD.contains("Before handing a worker a NEW task"));
        assert!(LEAD.contains("A nudge on the same task keeps its context"));
    }
}
