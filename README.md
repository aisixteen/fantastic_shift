# fantastic_shift

**Unattended work shifts on [fantastic](https://github.com/aisixteen/fantastic).** You queue tasks; a shift of AI workers takes them one by one, does the work on its own, and hands back finished results for you to review. You come back to done work, not to a chat.

It is not only for code. A task is any piece of work that can be described exactly and checked:

| Kind of work | What a task looks like | What comes back |
|---|---|---|
| Code | "fix this bug in these files, with these tests" | a branch or pull request, tests passing |
| Documentation | "write the setup guide for X from these sources" | a document, ready to read |
| Research | "compare these five tools on price and limits" | a report with sources |
| Triage | "read these 40 issues, label and summarize each" | labels and comments, a summary |
| Data | "clean this CSV, flag rows that break these rules" | the cleaned file and a list of flags |
| Review | "check this PR against our rules" | a review with findings |
| Translation, ports | "port this module to Python, same behaviour" | the port, with its tests |

## The shifter

A shift is run by **the shifter**: an AI agent that works from a directive. It spawns worker agents when there is work, gives each a task, checks on them, pushes a worker that stops early, stops one that goes off the rails, and deletes workers when the queue is empty. The shifter is configured, not programmed: its model, its directive, the worker templates it may spawn (model, backend, credentials ref, limits) and the connectors it may use all come from `shift.yaml`. No provider or model is named in the code.

**Connectors** (GitHub, a plain git remote, others) are bound by configuration, each with its own directive — what the shifter and the workers may do with it, and how. A shift without connectors simply leaves its results as files.

The one thing the code enforces itself is the sandbox: workers run commands with no forge credentials and cannot reach a forge from a shell. Only connectors can.

## A general-purpose shifter

The shift itself knows nothing about code, GitHub, or any kind of work. It does one thing: it takes the next task, gives the task's **instructions** to a free worker, waits until the worker says it is done, records the time and cost, and pushes or stops a worker that stalls. Everything specific to the work — what to read, what to produce, which tools to use (files, commands, git, GitHub, nothing at all), how to check the result — is in the task's instructions. A coding task is just one kind of instructions; so is a research task or a triage task.

The tools a worker may call are agents it is granted per shift: a worker on a writing shift may get only files; a worker on a coding shift gets files, commands and git; a GitHub delivery agent is added only where results go to GitHub.

## How a shift runs

```
 your tasks ──▶ queue ──▶ dispatcher ──▶ worker (an AI agent, its own workspace)
                              ▲               │  read · do the work · check it
                              │               ▼
                              └── done ◀── delivery: files · a report · a comment · a branch / PR
                                              │
                                              ▼
                                         you review
```

- **Tasks are exact.** Each says what to read, what to produce, and how to check it. A strong model or a person writes the task; workers carry it out. Exact tasks are what make unattended work reliable — and cheap: small models do them well.
- **The dispatcher keeps everyone busy.** The next task goes to whichever worker finished, the one with the fewest tasks first. A worker that stops early is pushed on; one that goes off the rails is stopped and its task marked failed, with the reason.
- **Workers hold no secrets.** They work in their own folder with no credentials. Delivering — pushing a branch, opening a pull request, posting a comment — goes through a delivery agent that owns the credentials and the rules.
- **Everything is measured.** Each task records its worker, model, time, tokens and cost, so you see what a task really costs and which model is worth it.

## Any model, any mix

Workers are fantastic AI agents: Anthropic models (Haiku, Sonnet, Opus) over the API with your own key, or local models (vLLM, llama.cpp, Strata) on your own GPU. Run several at once, mix them, and compare them on the same tasks — speed, cost and quality per task are in the log.

## Delivery: optional, pluggable

Results can stay as files in the shift's folder. When work belongs somewhere else, a delivery agent takes it there:

- **git** — a branch pushed to any remote (SSH or HTTPS), public or private, any host.
- **GitHub** (optional) — as a GitHub App: draft pull requests, issue comments, labels, reading a task queue from issues. Register your own app with the manifest in this repo; the private key stays in your credential store.
- more later (GitLab, Gitea, email, a webhook).

## Security model

- Secrets live only in fantastic's credential store (your OS keychain, or an env / file backend in a container). An agent reads a secret when it uses it and never returns, logs or stores it; tests check that no secret appears in any reply, log row or error.
- Workers run commands with a sandbox home: no forge login, no SSH agent, no access to key files. They cannot deliver anything except through the delivery agents.
- Delivery agents enforce the rules themselves: no pushes to the default branch, no force-pushes, pull requests as drafts, an allowlist of repositories.
- You review and merge. A shift never merges.

## What is in this repo

| Part | What it is |
|---|---|
| `fantastic_shift` | The program: a headless fantastic host plus the agents below. `--canvas` adds fantastic's browser canvas to watch the shift. |
| the shifter | An AI agent configured in `shift.yaml`: its model, its directive, the worker templates it may spawn. |
| `queue` agent | Tasks as data: add, list, take, mark done — the shifter's tool. |
| connectors | `git` (`shift_git.tools`: worktrees, commits, pushes to any remote; never the default branch, never a force), `github` (optional, `shift_github.tools`: a GitHub App you register yourself — push via the git connector, draft PRs, issues, comments, labels, PR status; a repo allowlist; tokens minted per call): bound in `shift.yaml`, each with its directive. |
| `shift.yaml` | The shifter, the worker templates (any backend, any model), the connectors and their directives, the task source. |
| task templates | How to write a task that a worker can carry out alone. |

## Build and run

```
cargo build --release
./target/release/fantastic_shift --dir ./my-shift
```

Ctrl-C to stop. fantastic is a pinned git dependency. On macOS the build needs `MACOSX_DEPLOYMENT_TARGET=14.0` (fantastic's Swift bridge targets macOS 14); `.cargo/config.toml.example` sets it under `[env]`.

## Updating fantastic

fantastic is a git dependency followed by branch, pinned in `Cargo.lock`. `scripts/update-fantastic.sh` moves to the newest commit and runs the build and tests. `.cargo/config.toml.example` builds against a local checkout.

## Status

Being built — by fantastic's own workers, from exact tasks. Progress: the [project board](https://github.com/orgs/aisixteen/projects/2) and the [issues](https://github.com/aisixteen/fantastic_shift/issues).

License: AGPL-3.0-or-later, like fantastic.
