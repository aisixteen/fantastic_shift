//! Kernel-free scanner for secret-looking text: known token prefixes and PEM
//! private-key headers. Findings name the rule and the line, never the text.

/// One line that looks like it holds a secret. It never carries the matched text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFinding {
    /// Which rule fired. Always `secret` for now.
    pub rule: &'static str,
    /// 1-based line number.
    pub line: usize,
}

/// Known token prefixes (cloud keys, GitHub and Slack tokens).
const SECRET_PREFIXES: &[&str] = &["AKIA", "ghp_", "github_pat_", "xoxb-"];
/// Header of a PEM private key.
const PRIVATE_KEY_MARK: &str = "-----BEGIN ";
/// Fewest token characters after a prefix that still count as a token.
const MIN_TOKEN_LEN: usize = 12;

/// True when a line holds a known token prefix followed by enough token
/// characters, or a PEM private-key header.
pub fn line_has_secret(line: &str) -> bool {
    if line.contains(PRIVATE_KEY_MARK) {
        return true;
    }
    SECRET_PREFIXES.iter().any(|p| {
        line.match_indices(p).any(|(i, _)| {
            line[i + p.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .count()
                >= MIN_TOKEN_LEN
        })
    })
}

/// Scan text line by line. One finding per line that looks like a secret.
pub fn scan_text(text: &str) -> Vec<TextFinding> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line_has_secret(line))
        .map(|(idx, _)| TextFinding {
            rule: "secret",
            line: idx + 1,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prefix followed by `n` token characters. Built at run time so the
    /// source holds no secret-shaped literal.
    fn token(prefix: &str, n: usize) -> String {
        format!("{prefix}{}", "a".repeat(n))
    }

    #[test]
    fn a_known_prefix_with_a_long_token_is_flagged() {
        let found = scan_text(&format!("key = {}\n", token("AKIA", 16)));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule, "secret");
        assert_eq!(found[0].line, 1);
    }

    #[test]
    fn a_prefix_with_a_short_token_is_not_flagged() {
        assert!(scan_text(&format!("x {}\n", token("ghp_", 4))).is_empty());
    }

    #[test]
    fn a_bare_prefix_is_not_flagged() {
        assert!(scan_text("the AKIA prefix marks a key id\n").is_empty());
    }

    #[test]
    fn a_pem_private_key_header_is_flagged() {
        let header = format!("{}RSA PRIVATE KEY-----", "-----BEGIN ");
        assert!(line_has_secret(&header));
    }

    #[test]
    fn ordinary_text_is_not_flagged() {
        assert!(scan_text("Review the diff. Keep notes short.\n").is_empty());
    }

    #[test]
    fn line_numbers_count_from_one() {
        let text = format!("ok\nok\n{}\n", token("xoxb-", 20));
        let found = scan_text(&text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 3);
    }

    #[test]
    fn a_finding_never_carries_the_text() {
        let secret = token("github_pat_", 20);
        let found = scan_text(&format!("{secret}\n"));
        assert!(!format!("{found:?}").contains(&secret));
    }
}
