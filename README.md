# AISixteen Fantastic Team

Program: `fantastic_team`. The long install guide is docs/install.md.

**Unattended team work on [fantastic](https://github.com/aisixteen/fantastic).** You queue tasks; a team of AI workers takes them one by one, does the work on its own, and hands back finished results for you to review. You come back to done work, not to a chat.

It is not only for code. A task is any piece of work that can be described exactly and checked: code, documentation, research, triage, data, review, translation or ports.

## Install

You need `git`, a Rust toolchain with `cargo`, and Linux or macOS (verified on Linux only). `gh` is optional.

```
git clone https://github.com/aisixteen/fantastic_team
cd fantastic_team
cargo build --release
./target/release/fantastic_team --help
```

The binary is `target/release/fantastic_team`. There are no releases yet, so build from source. On macOS the build needs `MACOSX_DEPLOYMENT_TARGET=14.0`; `.cargo/config.toml.example` sets it under `[env]`.

For the step-by-step version, with the GitHub App registration and troubleshooting, see docs/install.md.

## Configure

A team is a folder with a `team.yaml`. The file names the worker templates the team may start, the connectors it may use, the queue, and the credential store.

```yaml
templates:
  writer:
    handler_module: anthropic_backend
    model: example-model
    endpoint: https://api.example.com/v1
    credentials:
      store: credentials
      ref: EXAMPLE_KEY
    price_input_per_mtok: 1.0
    price_output_per_mtok: 5.0
first_agent:
  id: lead
  template: writer
  directive_file: lead.md
connectors:
  git:
    kind: team_git.tools
    directive_file: git.md
queue:
  id: queue
credentials:
  id: credentials
  backend: env
  refs:
    EXAMPLE_KEY: EXAMPLE_KEY
```

- `templates`: each is an agent template (model, endpoint, credential ref, prices per million tokens).
- `first_agent`: the lead, made from a template, instructed by its `directive_file`.
- `connectors`: GitHub, a plain git remote, or others, each with its own directive.
- `queue`: where tasks wait.
- `credentials`: the store the team creates when it has none. The values are never in this file; only the ref names are.

Each repo that the team serves can opt in with `.github/team.yaml`:

```yaml
task_labels: [team-ready]
paths: [src/**, docs/*.md]
build: cargo build
test: cargo test
review:
  approvals: 1
  approvers: [alice]
```

Every key is optional. An empty file gives the defaults: the label `team-ready`, any path, and one approval from any reviewer.

## Lanes

A lane is one worker template a team may start. Lanes differ in model, price, and the tools granted to them. Run several lanes at once, mix models, and compare them on the same tasks.

Tasks are exact: each says what to read, what to produce, and how to check it. The dispatcher gives the next task to the free worker with the fewest tasks first. A worker that stops early is pushed on; one that goes off the rails is stopped and its task marked failed, with the reason.

## Run

```
./target/release/fantastic_team --dir ./my-team            # headless: no port, no terminal UI
./target/release/fantastic_team --dir ./my-team --port 8123  # also serves the web UI on 127.0.0.1:8123
./target/release/fantastic_team --dir ./my-team --tui        # the terminal UI (needs a terminal)
```

`--port` and `--tui` are the only surfaces and are not combined. Ctrl-C to stop.

The build packs fantastic's canvas page with a pinned esbuild. With no vendored binary it compiles esbuild itself, which needs Go 1.21 or newer on PATH.

## Run it on another machine

The team can run on a box with a GPU while you start and watch it from a laptop over SSH. It runs detached, like a tmux session: closing the laptop, the tunnel or a terminal never stops it. Until these become `fantastic_team remote …` subcommands, they are scripts:


```
scripts/remote-add.sh gpu box.example.com:/srv/team --port 8093 --keys "ANTHROPIC_API_KEY"   # once
scripts/remote-start.sh gpu
scripts/remote-ls.sh
scripts/remote-attach.sh gpu     # Ctrl-C detaches; gpu keeps running
scripts/remote-logs.sh gpu -f
scripts/remote-stop.sh gpu       # asks first; drains (SIGTERM); --now kills
```

- **Keys only at start.** The laptop config names variables, never values. `remote-start.sh` reads their values from the laptop's environment and pipes them over SSH stdin. They are never in an argv, a file or shell history.
- **127.0.0.1 only.** The team binds the box's loopback; `remote-attach.sh` reaches it through an SSH tunnel. No public port.
- **After the box reboots** the team is down, because its keys were only in memory. `remote-ls.sh` says so.
- Every script takes `--dry-run`, which prints the commands and no secret.

## Observe

- **Cost and time per task.** Each task records its worker, model, time, tokens and cost, computed from the template's prices. Compare lanes on what a task really costs.
- **The board** (`team_board.tools`): the shared picture of the work — open issues with their queue state (pending, in work, done, failed) and the agents. Verb `board`.
- **The worker view** (`team_worker_view.tools`): one canvas per worker, with its task, files, output and chat.
- **The web UI** with `--port`, or the terminal UI with `--tui`.
- **Logs** through `scripts/remote-logs.sh` when the team runs remotely.

## Security model

- Secrets live only in fantastic's credential store (your OS keychain, or an env / file backend in a container). An agent reads a secret when it uses it and never returns, logs or stores it. Tests check that no secret appears in any reply, error, event, store row, record or log file (`crates/secrets_guard`). `scripts/check-secrets.sh` fails when a tracked or new file holds a private key or a GitHub or Anthropic token; run it before you commit.
- Workers run commands through an exec agent the team configures (`crates/team/src/worker_exec.rs`): HOME is a sandbox folder in the team (no forge login, no git credential helper, no SSH keys), the environment is a fixed list of names (no token, no key, no SSH agent), and forge CLIs (`gh`, `glab`, `hub`, `git push`) are refused before they start. That refusal is an early, clear error, not a wall. It is not an OS sandbox: a worker runs as your user, so run the team as a separate user or in a container to keep it away from files your user can read (`~/.ssh`, the login keychain).
- Workers hold no credentials. Delivering (pushing a branch, opening a pull request, posting a comment) goes through a connector that owns the credentials and the rules.
- Delivery agents enforce the rules themselves: no pushes to the default branch, no force-pushes, pull requests as drafts, an allowlist of repositories.
- You review and merge. The team merges only PRs you approved on their current head (`app-merge`).

## The lead

A team is run by **the lead**: an AI agent configured in `team.yaml` that works from a directive. It spawns worker agents when there is work, gives each a task, checks on them, and deletes workers when the queue is empty. Its model, directive and templates come from config; no provider or model is named in the code.

## Any model, any mix

Workers are fantastic AI agents: Anthropic models over the API with your own key, or local models (vLLM, llama.cpp) on your own GPU.

## Delivery: optional, pluggable

Results can stay as files in the team's folder. When work belongs somewhere else, a connector takes it there:

- **git**: a branch pushed to any remote (SSH or HTTPS).
- **GitHub** (optional): as a GitHub App you register yourself. Draft pull requests, issue comments, labels, reading a task queue from issues. Register with your own app; the private key stays in your credential store.

## What is in this repo

| Part | What it is |
|---|---|
| `fantastic_team` | The program: fantastic's terminal-shell library plus the agents below. Headless by default. |
| the lead | An AI agent configured in `team.yaml`. |
| `queue` agent | Tasks as data: add, list, take, mark done. |
| `team_puller.tools` | Reads `team-ready` issues from the GitHub connector and writes `queue/<repo>-<n>.md`. |
| connectors | `git` (`team_git.tools`) and `github` (optional, `team_github.tools`). Bound in `team.yaml`, each with its directive. |
| `team_board.tools` | Board: the shared picture of the work. Verbs `pull`, `board`, `task`, `report`, `agent`, `ask`, `audit`. It never assigns. |
| `team_worker_view.tools` | Worker view: one vertical canvas per worker. |
| `team_webhook.tools` | Optional webhook receiver (off unless enabled). |
| `team.yaml` | The lead, the worker templates, the connectors and their directives, the queue. |

## Updating fantastic

fantastic is a pinned git dependency. `scripts/update-fantastic.sh` moves all fantastic crates to the newest commit of `main` and runs the build and tests. Then commit the lock file. Use `.cargo/config.toml.example` (copied to a git-ignored `.cargo/config.toml`) to test an unreleased local fantastic checkout; never commit that copy.

## Status

Being built by fantastic's own workers, from exact tasks. Progress: the [project board](https://github.com/orgs/aisixteen/projects/2) and the [issues](https://github.com/aisixteen/fantastic_team/issues).

License: AGPL-3.0-or-later, like fantastic.
