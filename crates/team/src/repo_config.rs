//! Per-repo configuration, `.github/team.yaml`. A repo opts in by committing
//! this file. The text is untrusted: it is parsed and validated here, never
//! executed. Pure: no I/O.

use serde::Deserialize;

/// The file a served repo commits to opt in.
pub const FILE: &str = ".github/team.yaml";
/// The label that marks a task when the file does not name its own.
pub const DEFAULT_LABEL: &str = "team-ready";

/// Who must review a change before it is merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    /// Required approving reviews, at least 1.
    pub approvals: u32,
    /// Logins whose approval counts. Empty means any reviewer.
    pub approvers: Vec<String>,
}

/// The validated configuration of one repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoConfig {
    /// Labels that mark an issue as a task. Never empty.
    pub task_labels: Vec<String>,
    /// Relative globs a worker may touch. Empty means all paths.
    pub paths: Vec<String>,
    /// Build command, if the repo names one.
    pub build: Option<String>,
    /// Test command, if the repo names one.
    pub test: Option<String>,
    /// Review rules.
    pub review: Review,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default)]
    task_labels: Option<Vec<String>>,
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    build: Option<String>,
    #[serde(default)]
    test: Option<String>,
    #[serde(default)]
    review: Option<RawReview>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReview {
    #[serde(default)]
    approvals: Option<u32>,
    #[serde(default)]
    approvers: Option<Vec<String>>,
}

/// Parse and validate the text of `.github/team.yaml`. An empty file gives the
/// defaults. Errors name the offending key or value.
pub fn parse(text: &str) -> Result<RepoConfig, String> {
    let raw: Raw = if text.trim().is_empty() {
        Raw::default()
    } else {
        serde_yaml::from_str(text).map_err(|e| format!("team.yaml: {e}"))?
    };

    let task_labels = raw
        .task_labels
        .unwrap_or_else(|| vec![DEFAULT_LABEL.to_string()]);
    if task_labels.is_empty() || task_labels.iter().any(|l| l.trim().is_empty()) {
        return Err("team.yaml: task_labels must be a non-empty list of label names".into());
    }

    let paths = raw.paths.unwrap_or_default();
    for p in &paths {
        let escapes = p.is_empty()
            || p.starts_with('/')
            || p.contains('\\')
            || p.split('/').any(|seg| seg == "..");
        if escapes {
            return Err(format!(
                "team.yaml: path `{p}` must be relative, with no `..` segment"
            ));
        }
    }

    let (approvals, approvers) = match raw.review {
        None => (1, Vec::new()),
        Some(r) => (r.approvals.unwrap_or(1), r.approvers.unwrap_or_default()),
    };
    if approvals == 0 {
        return Err("team.yaml: review.approvals must be at least 1".into());
    }

    Ok(RepoConfig {
        task_labels,
        paths,
        build: raw.build,
        test: raw.test,
        review: Review {
            approvals,
            approvers,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let cfg = parse(
            "task_labels: [do-it, team-ready]\n\
             paths: [src/**, docs/*.md]\n\
             build: cargo build\n\
             test: cargo test\n\
             review:\n  approvals: 2\n  approvers: [alice, bob]\n",
        )
        .unwrap();
        assert_eq!(cfg.task_labels, vec!["do-it", "team-ready"]);
        assert_eq!(cfg.paths, vec!["src/**", "docs/*.md"]);
        assert_eq!(cfg.build.as_deref(), Some("cargo build"));
        assert_eq!(cfg.test.as_deref(), Some("cargo test"));
        assert_eq!(cfg.review.approvals, 2);
        assert_eq!(cfg.review.approvers, vec!["alice", "bob"]);
    }

    #[test]
    fn empty_file_gets_defaults() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg.task_labels, vec![DEFAULT_LABEL]);
        assert!(cfg.paths.is_empty());
        assert_eq!(cfg.review.approvals, 1);
        assert!(cfg.build.is_none() && cfg.test.is_none());
    }

    #[test]
    fn rejects_unknown_key() {
        let err = parse("buidl: make\n").unwrap_err();
        assert!(err.contains("buidl"), "{err}");
    }

    #[test]
    fn rejects_parent_dir_in_paths() {
        let err = parse("paths: [\"../x\"]\n").unwrap_err();
        assert!(err.contains("../x"), "{err}");
    }

    #[test]
    fn rejects_zero_approvals() {
        let err = parse("review:\n  approvals: 0\n").unwrap_err();
        assert!(err.contains("approvals"), "{err}");
    }
}
