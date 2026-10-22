//! The PR footer: a lane's usage record rendered as one markdown block that
//! `pr_create` appends to a pull request body. Pure: the same record always gives
//! the same text. It carries no credentials, tokens or paths.

use serde_json::Value;

/// The first line of the footer, so a reader can detect it.
pub(crate) const MARKER: &str = "<!-- fantastic-lane-usage -->";

fn num(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// Wall time as `Hh Mm Ss`.
pub(crate) fn hms(secs: u64) -> String {
    format!(
        "{}h {:02}m {:02}s",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Cache hit rate: cache reads over all input tokens, or `n/a` when there are none.
pub(crate) fn cache_rate(input: u64, cache_read: u64) -> String {
    let total = input + cache_read;
    if total == 0 {
        "n/a".into()
    } else {
        format!("{:.0}%", cache_read as f64 * 100.0 / total as f64)
    }
}

/// The footer text for one usage record.
pub(crate) fn footer(usage: &Value) -> String {
    let cost = usage
        .get("cost_usd")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0);
    format!(
        "{MARKER}\n---\nLane `{}` · model `{}` · wall {} · requests {} · input {} tokens · output {} tokens · cache hit {} · cost ${:.2}",
        text(usage, "lane"),
        text(usage, "model"),
        hms(num(usage, "wall_secs")),
        num(usage, "requests"),
        num(usage, "input_tokens"),
        num(usage, "output_tokens"),
        cache_rate(num(usage, "input_tokens"), num(usage, "cache_read_tokens")),
        cost
    )
}

/// The body with the footer appended after a blank line, or unchanged without usage.
pub(crate) fn with_footer(body: &str, usage: Option<&Value>) -> String {
    match usage.filter(|u| !u.is_null()) {
        Some(u) => format!("{body}\n\n{}", footer(u)),
        None => body.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    use crate::tests::{fx, mock_app, requests_to};

    fn rec() -> Value {
        json!({
            "lane": "team10",
            "model": "haiku-x",
            "wall_secs": 3723,
            "requests": 12,
            "input_tokens": 5000,
            "output_tokens": 800,
            "cache_read_tokens": 15000,
            "cost_usd": 0.4567
        })
    }

    #[test]
    fn footer_from_fixed_usage_record() {
        assert_eq!(
            footer(&rec()),
            "<!-- fantastic-lane-usage -->\n---\nLane `team10` · model `haiku-x` · wall 1h 02m 03s · requests 12 · input 5000 tokens · output 800 tokens · cache hit 75% · cost $0.46"
        );
    }

    #[test]
    fn footer_cache_rate_zero_input_is_na() {
        assert_eq!(cache_rate(0, 0), "n/a");
        let zero = json!({"lane": "l", "model": "m", "input_tokens": 0, "cache_read_tokens": 0});
        assert!(footer(&zero).contains("cache hit n/a"));
    }

    #[test]
    fn footer_formats_wall_time_hms() {
        assert_eq!(hms(0), "0h 00m 00s");
        assert_eq!(hms(59), "0h 00m 59s");
        assert_eq!(hms(3723), "1h 02m 03s");
    }

    #[test]
    fn footer_cost_two_decimals() {
        let v = json!({"cost_usd": 2.5});
        assert!(footer(&v).ends_with("cost $2.50"));
    }

    #[test]
    fn with_footer_appends_when_usage_given() {
        let out = with_footer("body text", Some(&rec()));
        assert_eq!(out, format!("body text\n\n{}", footer(&rec())));
        assert!(out.ends_with(&footer(&rec())));
    }

    #[test]
    fn with_footer_body_unchanged_without_usage() {
        assert_eq!(with_footer("body text", None), "body text");
        assert_eq!(with_footer("body text", Some(&Value::Null)), "body text");
    }

    async fn mount_repo_and_pulls(gh: &wiremock::MockServer) {
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"default_branch": "main"})),
            )
            .mount(gh)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/widgets/pulls"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "number": 7, "html_url": "https://example.test/pr/7", "draft": true
            })))
            .mount(gh)
            .await;
    }

    #[tokio::test]
    async fn pr_create_appends_footer_when_usage_given() {
        let f = fx(json!({})).await;
        mock_app(&f.gh).await;
        mount_repo_and_pulls(&f.gh).await;
        let reply = f
            .call(json!({
                "type": "pr_create", "repo": "acme/widgets", "branch": "feature",
                "title": "T", "body": "Hello", "usage": rec()
            }))
            .await;
        assert_eq!(reply["number"], json!(7), "{reply}");
        let reqs = requests_to(&f.gh, "/repos/acme/widgets/pulls").await;
        assert_eq!(reqs.len(), 1);
        let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(sent["body"], json!(format!("Hello\n\n{}", footer(&rec()))));
    }

    #[tokio::test]
    async fn pr_create_body_unchanged_without_usage() {
        let f = fx(json!({})).await;
        mock_app(&f.gh).await;
        mount_repo_and_pulls(&f.gh).await;
        let reply = f
            .call(json!({
                "type": "pr_create", "repo": "acme/widgets", "branch": "feature",
                "title": "T", "body": "Hello"
            }))
            .await;
        assert_eq!(reply["number"], json!(7), "{reply}");
        let reqs = requests_to(&f.gh, "/repos/acme/widgets/pulls").await;
        let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(sent["body"], json!("Hello"));
    }

    #[test]
    fn footer_contains_no_secret_like_values() {
        let f = footer(&rec());
        assert!(!f.contains('/'));
        assert!(!f.contains("ghp_"));
        assert!(!f.contains("github_pat_"));
        assert!(!f.contains("BEGIN"));
        assert!(!f.to_lowercase().contains("token\"") && !f.contains("secret"));
    }
}
