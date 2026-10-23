//! Pack evals: sample tasks with expected outcomes and a check. A pack's
//! `evals/*.yaml` files are parsed here and run against an abstract
//! [`EvalExecutor`], so the runner needs no real model: the caller supplies the
//! executor that hands each task to a worker.

use std::time::{Duration, Instant};

use async_trait::async_trait;

/// How an eval decides whether its outcome is a pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Check {
    /// The outcome text contains this string.
    Contains(String),
    /// The outcome text matches this regular expression.
    Regex(String),
    /// The outcome lists this path among the files it produced.
    FileExists(String),
    /// The outcome's exit code equals this number.
    ExitCode(i32),
}

/// One sample task with its expected outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Eval {
    /// Stable id within the pack.
    pub id: String,
    /// The task text handed to the executor.
    pub task: String,
    /// The check applied to the outcome.
    pub check: Check,
}

/// What an executor reports for one task.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    /// The text the task produced.
    pub text: String,
    /// Paths of the files the task produced.
    pub files: Vec<String>,
    /// The exit code of the task.
    pub exit_code: i32,
    /// The cost as the executor reports it, if it reports one.
    pub cost: Option<f64>,

}

/// Runs one task. An `Err` becomes an eval failure with that reason.
#[async_trait]
pub trait EvalExecutor: Send + Sync {
    /// Run `task` and report its outcome.
    async fn run(&self, task: &str) -> Result<Outcome, String>;
}

/// The result of one eval.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalResult {
    /// The eval's id.
    pub id: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Why it failed; `None` when it passed.
    pub reason: Option<String>,
    /// Time the eval took.
    pub elapsed: Duration,
    /// Cost the executor reported, if any.
    pub cost: Option<f64>,
}

/// The result of a whole run.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalSummary {
    /// How many evals passed.
    pub passed: usize,
    /// How many evals ran.
    pub total: usize,
    /// passed / total; 0.0 when there are no evals.
    pub pass_rate: f64,
    /// Sum of the eval times.
    pub total_time: Duration,
    /// Sum of the reported costs; `None` when no eval reported one.
    pub total_cost: Option<f64>,
    /// One result per eval, in order.
    pub per_eval: Vec<EvalResult>,
}

/// Parse the eval files of a pack. Each item is `(file name, yaml text)`; each
/// file is a YAML list of evals. An unknown check kind is refused.
pub fn parse_evals<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<Vec<Eval>, String> {
    let mut evals = Vec::new();
    for (name, text) in files {
        let doc: serde_yaml::Value =
            serde_yaml::from_str(text).map_err(|e| format!("{name}: {e}"))?;
        let items = doc
            .as_sequence()
            .ok_or_else(|| format!("{name}: expected a list of evals"))?;
        for item in items {
            evals.push(parse_one(name, item)?);
        }
    }
    Ok(evals)
}

fn parse_one(name: &str, v: &serde_yaml::Value) -> Result<Eval, String> {
    let field = |key: &str| -> Result<String, String> {
        v.get(key)
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("{name}: an eval needs a string `{key}`"))
    };
    let id = field("id")?;
    let task = field("task")?;
    let raw = v
        .get("check")
        .ok_or_else(|| format!("{name}: eval `{id}` has no check"))?;
    let check = parse_check(name, &id, raw)?;
    Ok(Eval { id, task, check })
}

fn parse_check(name: &str, id: &str, v: &serde_yaml::Value) -> Result<Check, String> {
    let bad = |why: &str| format!("{name}: eval `{id}`: {why}");
    let map = v
        .as_mapping()
        .filter(|m| m.len() == 1)
        .ok_or_else(|| bad("check must be a single `kind: value` pair"))?;
    let (key, val) = map.iter().next().ok_or_else(|| bad("empty check"))?;
    let kind = key
        .as_str()
        .ok_or_else(|| bad("check kind must be a word"))?;
    let text = || {
        val.as_str()
            .map(str::to_string)
            .ok_or_else(|| bad(&format!("`{kind}` needs a string")))
    };
    match kind {
        "contains" => Ok(Check::Contains(text()?)),
        "regex" => {
            let pat = text()?;
            regex::Regex::new(&pat).map_err(|e| bad(&format!("bad regex: {e}")))?;
            Ok(Check::Regex(pat))
        }
        "file_exists" => Ok(Check::FileExists(text()?)),
        "exit_code" => val
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .map(Check::ExitCode)
            .ok_or_else(|| bad("`exit_code` needs a whole number")),
        other => Err(bad(&format!("unknown check kind `{other}`"))),
    }
}

fn check_outcome(check: &Check, out: &Outcome) -> Result<(), String> {
    match check {
        Check::Contains(s) => {
            if out.text.contains(s.as_str()) {
                Ok(())
            } else {
                Err(format!("output does not contain {s:?}"))
            }
        }
        Check::Regex(r) => {
            let re = regex::Regex::new(r).map_err(|e| format!("bad regex: {e}"))?;
            if re.is_match(&out.text) {
                Ok(())
            } else {
                Err(format!("output does not match /{r}/"))
            }
        }
        Check::FileExists(p) => {
            if out.files.iter().any(|f| f == p) {
                Ok(())
            } else {
                Err(format!("file {p} was not produced"))
            }
        }
        Check::ExitCode(n) => {
            if out.exit_code == *n {
                Ok(())
            } else {
                Err(format!("exit code {}, expected {n}", out.exit_code))
            }
        }
    }
}

fn fail(id: &str, reason: String, elapsed: Duration) -> EvalResult {
    EvalResult {
        id: id.to_string(),
        passed: false,
        reason: Some(reason),
        elapsed,
        cost: None,
    }
}

/// Run every eval through `exec`, each bounded by `timeout`. A timeout or an
/// executor error is a fail with a reason; nothing panics.
pub async fn run_evals(evals: &[Eval], exec: &dyn EvalExecutor, timeout: Duration) -> EvalSummary {
    let mut per_eval = Vec::with_capacity(evals.len());
    for e in evals {
        let started = Instant::now();
        let res = match tokio::time::timeout(timeout, exec.run(&e.task)).await {
            Err(_) => fail(
                &e.id,
                format!("timed out after {}s", timeout.as_secs_f64()),
                started.elapsed(),
            ),
            Ok(Err(err)) => fail(&e.id, format!("executor error: {err}"), started.elapsed()),
            Ok(Ok(out)) => {
                let verdict = check_outcome(&e.check, &out);
                EvalResult {
                    id: e.id.clone(),
                    passed: verdict.is_ok(),
                    reason: verdict.err(),
                    elapsed: started.elapsed(),
                    cost: out.cost,
                }
            }
        };
        per_eval.push(res);
    }
    summarize(per_eval)
}

fn summarize(per_eval: Vec<EvalResult>) -> EvalSummary {
    let total = per_eval.len();
    let passed = per_eval.iter().filter(|r| r.passed).count();
    let pass_rate = if total == 0 {
        0.0
    } else {
        passed as f64 / total as f64
    };
    let total_time = per_eval.iter().map(|r| r.elapsed).sum::<Duration>();
    let costs: Vec<f64> = per_eval.iter().filter_map(|r| r.cost).collect();
    let total_cost = if costs.is_empty() {
        None
    } else {
        Some(costs.iter().sum())
    };
    EvalSummary {
        passed,
        total,
        pass_rate,
        total_time,
        total_cost,
        per_eval,
    }
}

/// One line for the UI or the CLI: pass rate, time and cost.
pub fn summary_text(s: &EvalSummary) -> String {
    let cost = match s.total_cost {
        Some(c) => format!("${c:.4}"),
        None => "n/a".to_string(),
    };
    format!(
        "evals: {}/{} passed, pass rate {:.0}%, time {:.1}s, cost {}",
        s.passed,
        s.total,
        s.pass_rate * 100.0,
        s.total_time.as_secs_f64(),
        cost
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    fn out(text: &str, exit: i32, cost: Option<f64>) -> Outcome {
        Outcome {
            text: text.to_string(),
            files: Vec::new(),
            exit_code: exit,
            cost,
        }
    }

    #[async_trait]
    impl EvalExecutor for Fake {
        async fn run(&self, task: &str) -> Result<Outcome, String> {
            match task {
                "hang" => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok(out("", 0, None))
                }
                "pass" => Ok(out("hello world", 0, Some(0.01))),
                "fail" => Ok(out("nope", 1, Some(0.02))),
                "boom" => Err("exec broke".to_string()),
                _ => Ok(out("", 0, None)),
            }
        }
    }

    fn ev(id: &str, task: &str, check: Check) -> Eval {
        Eval {
            id: id.to_string(),
            task: task.to_string(),
            check,
        }
    }

    const YAML: &str = "
- id: a
  task: t
  check: {contains: hi}
- id: b
  task: t
  check: {regex: \"^h.*$\"}
- id: c
  task: t
  check: {file_exists: out.txt}
- id: d
  task: t
  check: {exit_code: 0}
";

    #[test]
    fn evals_parse_each_check_kind() {
        let evals = parse_evals(vec![("evals/a.yaml", YAML)]).unwrap();
        assert_eq!(evals.len(), 4);
        assert_eq!(evals[0].check, Check::Contains("hi".to_string()));
        assert_eq!(evals[1].check, Check::Regex("^h.*$".to_string()));
        assert_eq!(evals[2].check, Check::FileExists("out.txt".to_string()));
        assert_eq!(evals[3].check, Check::ExitCode(0));
    }

    #[test]
    fn an_unknown_check_kind_is_refused() {
        let bad = "- id: a\n  task: t\n  check: {bogus: x}\n";
        let err = parse_evals(vec![("evals/bad.yaml", bad)]).unwrap_err();
        assert!(err.contains("unknown check kind"), "{err}");
    }

    #[tokio::test]
    async fn contains_passes_only_when_the_text_is_present() {
        // "pass" outputs "hello world"; "fail" outputs "nope".
        let evals = vec![
            ev("hit", "pass", Check::Contains("hello".to_string())),
            ev("miss", "fail", Check::Contains("hello".to_string())),
            ev("absent", "pass", Check::Contains("zzz".to_string())),
        ];
        let s = run_evals(&evals, &Fake, Duration::from_secs(2)).await;
        assert!(s.per_eval[0].passed, "present text must pass");
        assert!(!s.per_eval[1].passed, "absent text must fail");
        assert!(!s.per_eval[2].passed, "absent text must fail");
        assert_eq!(s.passed, 1);
    }

    #[tokio::test]
    async fn all_passing_gives_rate_one() {
        let evals = vec![
            ev("a", "pass", Check::Contains("hello".to_string())),
            ev("b", "pass", Check::ExitCode(0)),
        ];
        let s = run_evals(&evals, &Fake, Duration::from_secs(2)).await;
        assert_eq!(s.passed, 2);
        assert_eq!(s.total, 2);
        assert!((s.pass_rate - 1.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn one_of_two_failing_gives_half() {
        let evals = vec![
            ev("a", "pass", Check::Contains("hello".to_string())),
            ev("b", "fail", Check::ExitCode(0)),
        ];
        let s = run_evals(&evals, &Fake, Duration::from_secs(2)).await;
        assert_eq!(s.passed, 1);
        assert!((s.pass_rate - 0.5).abs() < 1e-9);
        assert!(s.per_eval[1]
            .reason
            .as_deref()
            .unwrap()
            .contains("exit code"));
    }

    #[tokio::test]
    async fn an_executor_error_is_a_fail_with_reason() {
        let evals = vec![ev("a", "boom", Check::ExitCode(0))];
        let s = run_evals(&evals, &Fake, Duration::from_secs(2)).await;
        assert!(!s.per_eval[0].passed);
        assert!(s.per_eval[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("exec broke"));
    }

    #[tokio::test]
    async fn a_timeout_is_a_fail() {
        let evals = vec![ev("a", "hang", Check::ExitCode(0))];
        let s = run_evals(&evals, &Fake, Duration::from_millis(50)).await;
        assert!(!s.per_eval[0].passed);
        assert!(s.per_eval[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("timed out"));
    }

    #[tokio::test]
    async fn cost_sums_when_reported_and_is_none_when_not() {
        let with = vec![
            ev("a", "pass", Check::ExitCode(0)),
            ev("b", "fail", Check::ExitCode(1)),
        ];
        let s = run_evals(&with, &Fake, Duration::from_secs(2)).await;
        assert!((s.total_cost.unwrap() - 0.03).abs() < 1e-9);

        let without = vec![ev("c", "silent", Check::ExitCode(0))];
        let s = run_evals(&without, &Fake, Duration::from_secs(2)).await;
        assert!(s.total_cost.is_none());
    }

    #[tokio::test]
    async fn summary_text_names_rate_time_and_cost() {
        let evals = vec![ev("a", "pass", Check::ExitCode(0))];
        let s = run_evals(&evals, &Fake, Duration::from_secs(2)).await;
        let line = summary_text(&s);
        assert!(line.contains("pass rate 100%"), "{line}");
        assert!(line.contains("time"), "{line}");
        assert!(line.contains("cost $"), "{line}");
    }
}
