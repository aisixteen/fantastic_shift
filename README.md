# AISixteen Fantastic Team

Program: `fantastic_team`. Install: docs/install.md

**Unattended team work on [fantastic](https://github.com/aisixteen/fantastic).** You queue tasks; a team of AI workers takes them one by one, does the work on its own, and hands back finished results for you to review. You come back to done work, not to a chat.

**How a shift works (1 min):**
https://github.com/aisixteen/fantastic_shift/raw/main/media/how_it_works.mp4

[media/how_it_works.mp4](media/how_it_works.mp4)

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

## The lead

A team is run by **the lead**: an AI agent that works from a directive. It spawns worker agents when there is work, gives each a task, checks on them, pushes a worker that stops early, stops one that goes off the rails, and deletes workers when the queue is empty. The lead is configured, not programmed: its model, its directive, the worker templates it may spawn (model, backend, credentials ref, limits) and the connectors it may use all come from `team.yaml`. No provider or model is named in the code.

For AIs: docs/ai-readme.md

**Connectors** (GitHub, a plain git remote, others) are bound by configuration, each with its own directive — what the lead and the workers may do with it, and how. A team without connectors simply leaves its results as files.

The one thing the code enforces itself is the sandbox: workers run commands with no forge credentials and cannot reach a forge from a shell. Only connectors can.

## A general-purpose team

The team itself knows nothing about code, GitHub, or any kind of work. It does one thing: it takes the next task, gives the task's **instructions** to a free worker, waits until the worker says it is done, records the time and cost, and pushes or stops a worker that stalls. Everything specific to the work — what to read, what to produce, which tools to use (files, commands, git, GitHub, nothing at all), how to check the result — is in the task's instructions. A coding task is just one kind of instructions; so is a research task or a triage task.

The tools a worker may call are agents it is granted per team: a worker on a writing team may get only files; a worker on a coding team gets files, commands and git; a GitHub delivery agent is added only where results go to GitHub.

## How a team runs

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

Results can stay as files in the team's folder. When work belongs somewhere else, a delivery agent takes it there:

- **git** — a branch pushed to any remote (SSH or HTTPS), public or private, any host.
- **GitHub** (optional) — as a GitHub App: draft pull requests, issue comments, labels, reading a task queue from issues. Register your own app with the manifest in this repo; the private key stays in your credential store.
- more later (GitLab, Gitea, email, a webhook).

## Security model

- Secrets live only in fantastic's credential store (your OS keychain, or an env / file backend in a container). An agent reads a secret when it uses it and never returns, logs or stores it; tests check that no secret appears in any reply, error, event, store row, record or log file (`crates/secrets_guard`, used by every agent crate here). `scripts/check-secrets.sh` fails when a tracked or new file holds a private key or a GitHub / Anthropic token — run it before you commit.
- Workers run commands through an exec agent the team configures (`crates/team/src/worker_exec.rs`): HOME is a sandbox folder in the team (no forge login, no git credential helper, no SSH keys), the environment is a fixed list of names (no token, no key, no SSH agent), and forge CLIs (`gh`, `glab`, `hub`, `git push`, …) are refused before they start. That refusal is an early, clear error, not a wall — a shell can spell a command many ways; what makes a push from a worker fail is that no credential is within its reach. It is not an OS sandbox: a worker runs as your user, so run the team as a separate user or in a container to keep it away from files your user can read (`~/.ssh`, the login keychain).
- Delivery agents enforce the rules themselves: no pushes to the default branch, no force-pushes, pull requests as drafts, an allowlist of repositories.
- You review and merge. The team merges only PRs you approved on their current head (`app-merge`).

## What is in this repo

| Part | What it is |
|---|---|
| `fantastic_team` | The program: fantastic's terminal-shell library (`fantastic-tui`) plus the agents below. Fully headless by default; `--port N` serves the web UI on 127.0.0.1:N; `--tui` opens the terminal UI. |
| the lead | An AI agent configured in `team.yaml`: its model, its directive, the worker templates it may spawn. |
| `queue` agent | Tasks as data: add, list, take, mark done — the lead's tool. |
| `team_puller.tools` | Puller: `pull` reads `team-ready` issues from the github connector (only when the latest label event is by the maintainer and the `## SPEC` comment is by the maintainer or the app) and writes `queue/<repo>-<n>.md` plus a pending `.state`; `status` answers. It never overwrites a state file. |
| connectors | `git` (`team_git.tools`: worktrees, commits, pushes to any remote; never the default branch, never a force), `github` (optional, `team_github.tools`: a GitHub App you register yourself — push via the git connector, draft PRs, issues, comments, labels, PR status; a repo allowlist; tokens minted per call): bound in `team.yaml`, each with its directive. |
| `team_board.tools` | Board: the shared picture of the work. Caches every open issue of the configured repos with its queue state (pending / in_work / done / failed), knows the agents, keeps its own state in the store; verbs `pull`, `board`, `task`, `report`, `agent`, `ask`, `audit`. It never assigns. |
| `team.yaml` | The lead, the worker templates (any backend, any model), the connectors and their directives, the task source. |
| task templates | How to write a task that a worker can carry out alone. |

## Build and run

```
cargo build --release
./target/release/fantastic_team --dir ./my-team            # headless: no port, no terminal UI
./target/release/fantastic_team --dir ./my-team --port 8123  # also serves the web UI on 127.0.0.1:8123
./target/release/fantastic_team --dir ./my-team --tui        # the terminal UI (needs a terminal)
```

`--port` and `--tui` are the only surfaces and are not combined; `mcp`, `send` and `readme` as the first word run fantastic's own commands in the current directory. Ctrl-C to stop. fantastic is a pinned git dependency. On macOS the build needs `MACOSX_DEPLOYMENT_TARGET=14.0` (fantastic's Swift bridge targets macOS 14); `.cargo/config.toml.example` sets it under `[env]`.

## Updating fantastic

fantastic has three modes. Every fantastic crate is one version in every mode.

1. **semver** (target default): once fantastic publishes releases to crates.io, the git lines in the root `Cargo.toml` become `fantastic-* = "0.x.y"`. Not yet: `cargo search fantastic-kernel` finds nothing, so the build stays on mode 2.
2. **git branch `main`** (today): the git lines in `Cargo.toml`, pinned by `Cargo.lock`. `scripts/update-fantastic.sh` moves all fantastic crates to the newest commit of `main` and runs the build and tests. Then commit the lock file.
3. **local checkout**: copy `.cargo/config.toml.example` to `.cargo/config.toml`, point its `[patch]` paths at your fantastic clone, build, and delete the copy. The file is git-ignored; never commit it.

Switch from mode 2 to 1 when releases exist: replace the git lines with versions and run `cargo update`. Use mode 3 to test an unreleased fantastic change.

## Status

Being built — by fantastic's own workers, from exact tasks. Progress: the [project board](https://github.com/orgs/aisixteen/projects/2) and the [issues](https://github.com/aisixteen/fantastic_team/issues).

License: AGPL-3.0-or-later, like fantastic.

## Run it on another machine

The team can run on a box with a GPU while you start and watch it from a laptop over SSH. It runs detached, like a tmux session: closing the laptop, the tunnel or a terminal never stops it. Until these become `fantastic_team remote …` subcommands, they are scripts:

```
scripts/remote-add.sh gpu box.example.com:/srv/team --port 8093 --keys "ANTHROPIC_API_KEY"   # once
scripts/remote-start.sh gpu      # started gpu · pid 157007 · log /srv/team/logs/kernel.log
scripts/remote-ls.sh             # NAME HOST PORT ATTACHED STATE
scripts/remote-attach.sh gpu     # attached to gpu at http://127.0.0.1:8093 — Ctrl-C detaches; gpu keeps running
scripts/remote-logs.sh gpu -f
scripts/remote-stop.sh gpu       # asks first; drains (SIGTERM); --now kills
```

- **Keys only at start.** The laptop config (`~/.config/fantastic-team/remotes/<name>`) names variables, never values. `remote-start.sh` reads their values from the laptop's environment and pipes them over SSH stdin into the box's memory: never in an argv, a file or shell history. `attach`, `ls`, `logs` and `stop` need no keys.
- **127.0.0.1 only.** The team binds the box's loopback; `remote-attach.sh` reaches it through an SSH tunnel with keep-alives and re-opens the tunnel when it drops. The tunnel is the security: no public port.
- **After the box reboots** the team is down (its keys were only in memory). `remote-ls.sh` says `down — box restarted, run remote start`.
- Every script takes `--dry-run`, which prints the commands it would run and no secret. Tests: `sh tests/remote_scripts.sh`.
