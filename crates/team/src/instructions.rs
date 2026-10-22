//! The first agent's instructions, from text or from flags. Given text and
//! rendered text concatenate, given first. The flags only become words: the
//! sentences below are templates, and the code chooses one by which flags were
//! given and fills in the counts. No topology logic lives here.

use std::path::{Path, PathBuf};

/// The sentence for `--workers N`. `{n}` is the count.
pub const WORKERS_TEMPLATE: &str = "Start {n} workers. Each one runs the worker directive: claim a task file in the queue, do it, hand it back, and repeat.";

/// The sentence for `--leads M --per-lead K`. `{m}` and `{k}` are the counts.
pub const LEADS_TEMPLATE: &str = "Start {m} leads. Each lead runs the lead directive: it starts {k} workers from templates, keeps them going, and stops the ones that go wrong.";

/// The flags that shape the first agent's instructions, as given on the command line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Flags {
    /// `--instructions <text>`: written instructions, used as given.
    pub instructions: Option<String>,
    /// `--instructions-file <path>`: a file whose contents are the instructions.
    pub instructions_file: Option<PathBuf>,
    /// `--workers <n>`: how many workers to start.
    pub workers: Option<u32>,
    /// `--leads <m>`: how many leads to start.
    pub leads: Option<u32>,
    /// `--per-lead <k>`: how many workers each lead starts.
    pub per_lead: Option<u32>,
}

/// Take the five instruction flags out of `argv`. Returns the flags and the
/// arguments that are left for the rest of the command line.
pub fn parse_flags(argv: &[String]) -> Result<(Flags, Vec<String>), String> {
    let mut f = Flags::default();
    let mut rest = Vec::new();
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            k @ ("--instructions"
            | "--instructions-file"
            | "--workers"
            | "--leads"
            | "--per-lead") => {
                let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
                match k {
                    "--instructions" => f.instructions = Some(v.clone()),
                    "--instructions-file" => f.instructions_file = Some(PathBuf::from(v)),
                    "--workers" => f.workers = Some(count(k, v)?),
                    "--leads" => f.leads = Some(count(k, v)?),
                    _ => f.per_lead = Some(count(k, v)?),
                }
            }
            _ => rest.push(a.clone()),
        }
    }
    Ok((f, rest))
}

fn count(flag: &str, v: &str) -> Result<u32, String> {
    v.parse::<u32>()
        .map_err(|_| format!("{flag} needs a whole number, got `{v}`"))
}

/// Render the instructions for `f`: the given text, the rendered topology, or
/// both. Refuses with the reason when the flags do not make sense together.
pub fn render(f: &Flags) -> Result<String, String> {
    if f.instructions.is_some() && f.instructions_file.is_some() {
        return Err("give --instructions or --instructions-file, not both".to_string());
    }
    for (flag, v) in [
        ("--workers", f.workers),
        ("--leads", f.leads),
        ("--per-lead", f.per_lead),
    ] {
        if v == Some(0) {
            return Err(format!("{flag} must be at least 1"));
        }
    }
    if f.per_lead.is_some() && f.leads.is_none() {
        return Err("--per-lead needs --leads".to_string());
    }
    if f.leads.is_some() && f.per_lead.is_none() {
        return Err("--leads needs --per-lead".to_string());
    }
    if f.workers.is_some() && f.leads.is_some() {
        return Err("--workers cannot be combined with --leads".to_string());
    }
    let given = match (&f.instructions, &f.instructions_file) {
        (Some(t), None) => Some(t.clone()),
        (None, Some(p)) => Some(read_file(p)?),
        _ => None,
    };
    match (given, topology(f)) {
        (None, None) => Err(
            "nothing given: pass --instructions, --instructions-file, or --workers, or --leads with --per-lead"
                .to_string(),
        ),
        (Some(g), None) => Ok(g),
        (None, Some(r)) => Ok(r),
        (Some(g), Some(r)) => Ok(format!("{g}\n\n{r}")),
    }
}

fn read_file(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read instructions file {}: {e}", path.display()))
}

/// The rendered topology sentence for the counts given, if any.
fn topology(f: &Flags) -> Option<String> {
    if let Some(n) = f.workers {
        return Some(WORKERS_TEMPLATE.replace("{n}", &n.to_string()));
    }
    match (f.leads, f.per_lead) {
        (Some(m), Some(k)) => Some(
            LEADS_TEMPLATE
                .replace("{m}", &m.to_string())
                .replace("{k}", &k.to_string()),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn flags(workers: Option<u32>, leads: Option<u32>, per_lead: Option<u32>) -> Flags {
        Flags {
            workers,
            leads,
            per_lead,
            ..Flags::default()
        }
    }

    #[test]
    fn renders_workers_only() {
        let out = render(&flags(Some(3), None, None)).unwrap();
        assert!(out.contains("Start 3 workers"));
    }

    #[test]
    fn renders_leads_and_per_lead() {
        let out = render(&flags(None, Some(2), Some(4))).unwrap();
        assert!(out.contains("Start 2 leads"));
        assert!(out.contains("starts 4 workers"));
    }

    #[test]
    fn given_instructions_then_rendered_text() {
        let f = Flags {
            instructions: Some("Build the parser.".to_string()),
            ..flags(Some(2), None, None)
        };
        let out = render(&f).unwrap();
        let given_at = out.find("Build the parser.").unwrap();
        let rendered_at = out.find("Start 2 workers").unwrap();
        assert!(given_at < rendered_at);
    }

    #[test]
    fn instructions_file_read_and_used() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("i.md");
        std::fs::write(&p, "From the file.").unwrap();
        let f = Flags {
            instructions_file: Some(p),
            ..Flags::default()
        };
        assert_eq!(render(&f).unwrap(), "From the file.");
    }

    #[test]
    fn refuses_both_instructions_and_file() {
        let f = Flags {
            instructions: Some("a".to_string()),
            instructions_file: Some(PathBuf::from("x")),
            ..Flags::default()
        };
        assert!(render(&f).unwrap_err().contains("not both"));
    }

    #[test]
    fn refuses_per_lead_without_leads() {
        let e = render(&flags(None, None, Some(2))).unwrap_err();
        assert_eq!(e, "--per-lead needs --leads");
    }

    #[test]
    fn refuses_leads_without_per_lead() {
        let e = render(&flags(None, Some(2), None)).unwrap_err();
        assert_eq!(e, "--leads needs --per-lead");
    }

    #[test]
    fn refuses_workers_with_leads() {
        let e = render(&flags(Some(2), Some(2), Some(2))).unwrap_err();
        assert!(e.contains("cannot be combined"));
    }

    #[test]
    fn refuses_zero_counts() {
        assert!(render(&flags(Some(0), None, None))
            .unwrap_err()
            .contains("--workers"));
        assert!(render(&flags(None, Some(0), Some(1)))
            .unwrap_err()
            .contains("--leads"));
        assert!(render(&flags(None, Some(1), Some(0)))
            .unwrap_err()
            .contains("--per-lead"));
    }

    #[test]
    fn refuses_nothing_given() {
        assert!(render(&Flags::default())
            .unwrap_err()
            .starts_with("nothing given"));
    }

    #[test]
    fn rendered_text_has_no_model_names() {
        let out = render(&flags(None, Some(2), Some(3))).unwrap();
        let low = out.to_lowercase();
        for d in [
            "claude",
            "haiku",
            "sonnet",
            "opus",
            "qwen",
            "gpt",
            "llama",
            "anthropic",
            "openai",
        ] {
            assert!(!low.contains(d), "rendered text names {d}");
        }
    }

    #[test]
    fn parse_flags_takes_the_five_and_leaves_the_rest() {
        let (f, rest) = parse_flags(&v(&["--dir", "/x", "--workers", "2", "--tui"])).unwrap();
        assert_eq!(f.workers, Some(2));
        assert_eq!(rest, v(&["--dir", "/x", "--tui"]));
        assert!(parse_flags(&v(&["--leads"])).is_err());
        assert!(parse_flags(&v(&["--workers", "x"])).is_err());
    }
}
