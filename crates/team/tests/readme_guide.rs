//! The README is a guide a third party follows. These tests keep it honest:
//! its sections are in order, the scripts it names exist, the config keys in
//! its examples are read by the code, and its examples hold no secret values.

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn readme() -> String {
    fs::read_to_string(root().join("README.md")).expect("README.md must exist")
}

#[test]
fn readme_has_the_guide_sections_in_order() {
    let text = readme();
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).collect();
    let wanted = [
        "## Install",
        "## Configure",
        "## Lanes",
        "## Run",
        "## Observe",
        "## Security model",
    ];
    let mut at = 0usize;
    for h in wanted {
        let pos = lines
            .iter()
            .position(|l| *l == h)
            .unwrap_or_else(|| panic!("README is missing the header `{h}`"));
        assert!(pos >= at, "README header `{h}` is out of order");
        at = pos;
    }
}

#[test]
fn every_script_the_readme_names_exists() {
    let text = readme();
    let mut found = 0;
    let mut rest = text.as_str();
    while let Some(i) = rest.find("scripts/") {
        rest = &rest[i + "scripts/".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .collect();
        if name.ends_with(".sh") {
            found += 1;
            let path = root().join("scripts").join(&name);
            assert!(
                path.exists(),
                "README names scripts/{name}, which does not exist"
            );
        }
    }
    assert!(found > 0, "README should name at least one script");
}

#[test]
fn every_config_key_the_readme_names_is_read_by_the_code() {
    let text = readme();
    let code = format!(
        "{}\n{}",
        fs::read_to_string(root().join("crates/team/src/config.rs")).unwrap(),
        fs::read_to_string(root().join("crates/team/src/repo_config.rs")).unwrap(),
    );
    let mut in_yaml = false;
    let mut checked = 0;
    let mut parent = String::new();
    for raw in text.lines() {
        let line = raw.trim_start();
        if line.starts_with("```") {
            in_yaml = line == "```yaml";
            parent.clear();
            continue;
        }
        if !in_yaml {
            continue;
        }
        let indent = raw.len() - line.len();
        if indent == 0 {
            parent = line.trim_end_matches(':').to_string();
        } else if indent == 2 && (parent == "templates" || parent == "connectors") {
            // a user-chosen name (template or connector id), not a config key
            continue;
        }
        let body = line.trim_start_matches("- ").trim_start();
        let key: String = body
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || *c == '_')
            .collect();
        if !key.is_empty() && body[key.len()..].starts_with(':') {
            checked += 1;
            assert!(
                code.contains(&key),
                "README example uses key `{key}`, which config.rs / repo_config.rs do not read"
            );
        }
    }
    assert!(checked > 0, "README should have yaml config examples");
}

#[test]
fn readme_examples_hold_no_secret_values() {
    let text = readme();
    for bad in ["ghp_", "-----BEGIN", "sk-"] {
        assert!(!text.contains(bad), "README must not contain `{bad}`");
    }
}

#[test]
fn readme_defaults_match_repo_config() {
    let text = readme();
    let code = format!(
        "{}\n{}",
        fs::read_to_string(root().join("crates/team/src/config.rs")).unwrap(),
        fs::read_to_string(root().join("crates/team/src/repo_config.rs")).unwrap(),
    );
    let label = code
        .lines()
        .find(|l| l.contains("const DEFAULT_LABEL"))
        .and_then(|l| l.split('"').nth(1))
        .expect("DEFAULT_LABEL must be a string literal in config.rs or repo_config.rs");
    let defaults = text
        .lines()
        .find(|l| l.starts_with("Every key is optional"))
        .expect("README must state the defaults in an `Every key is optional` line");
    assert!(
        defaults.contains(&format!("`{label}`")),
        "README default label disagrees with DEFAULT_LABEL `{label}`"
    );
    assert!(
        defaults.contains("one approval"),
        "README must say the default is one approval"
    );
    assert!(
        code.contains("cfg.review.approvals, 1"),
        "repo_config.rs must default review.approvals to 1, as the README says"
    );
}
