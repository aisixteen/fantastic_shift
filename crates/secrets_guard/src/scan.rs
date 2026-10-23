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

/// Known token prefixes (cloud keys, GitHub, Slack and `sk-` API keys).
const SECRET_PREFIXES: &[&str] = &["AKIA", "ghp_", "github_pat_", "xoxb-", "sk-"];
/// Start of a PEM block.
const PEM_BEGIN: &str = "-----BEGIN ";
/// What a PEM header must name to be a private key.
const PRIVATE_KEY_WORDS: &str = "PRIVATE KEY";
/// Fewest token characters after a prefix that still count as a token.
const MIN_TOKEN_LEN: usize = 12;

/// A PEM header that names a private key. Certificates and public keys are fine.
fn has_private_key_header(line: &str) -> bool {
    line.match_indices(PEM_BEGIN).any(|(i, _)| {
        let rest = &line[i + PEM_BEGIN.len()..];
        rest.split("-----")
            .next()
            .is_some_and(|header| header.contains(PRIVATE_KEY_WORDS))
    })
}

/// Characters a token may hold after its prefix (dashes appear in `xoxb-` and `sk-` keys).
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// True when a line holds a known token prefix (at a word start, so `task-...`
/// is not `sk-...`) followed by enough token characters, or a PEM private-key header.
pub fn line_has_secret(line: &str) -> bool {
    if has_private_key_header(line) {
        return true;
    }
    SECRET_PREFIXES.iter().any(|p| {
        line.match_indices(p).any(|(i, _)| {
            let at_word_start = match line[..i].chars().next_back() {
                Some(c) => !(c.is_ascii_alphanumeric() || c == '_'),
                None => true,
            };
            at_word_start
                && line[i + p.len()..]
                    .chars()
                    .take_while(|c| is_token_char(*c))
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
    fn a_certificate_or_public_key_block_is_not_flagged() {
        let cert = format!("{}CERTIFICATE-----", "-----BEGIN ");
        let public = format!("{}PGP PUBLIC KEY BLOCK-----", "-----BEGIN ");
        assert!(!line_has_secret(&cert));
        assert!(!line_has_secret(&public));
    }

    #[test]
    fn other_private_key_headers_are_flagged() {
        let pgp = format!("{}PGP PRIVATE KEY BLOCK-----", "-----BEGIN ");
        let ssh = format!("{}OPENSSH PRIVATE KEY-----", "-----BEGIN ");
        assert!(line_has_secret(&pgp));
        assert!(line_has_secret(&ssh));
    }

    #[test]
    fn a_dashed_slack_token_is_flagged() {
        let tok = format!("{}{}", "xoxb-", "1234567890-1234567890-abcdef");
        assert!(line_has_secret(&format!("slack = {tok}")));
    }

    #[test]
    fn an_sk_key_is_flagged_but_a_word_ending_in_sk_is_not() {
        assert!(line_has_secret(&format!("key {}", token("sk-", 24))));
        assert!(!line_has_secret(
            "see the task-management-board-planning notes"
        ));
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
