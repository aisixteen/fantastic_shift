# fantastic_team: README for AIs

You are reading the text of the `fantastic_team` binary, the team's start-up. Use only what is written here. Every command below exists in this repo.

## 1. Build and check

- Build: `cargo build --workspace`. The binary is `fantastic_team`.
- Check: `fantastic_team ai-readme` prints this file. `fantastic_team --help` prints the usage text.
- Tests: `cargo test --workspace`. Secrets check: `scripts/check-secrets.sh .`

## 2. Start the team

Run it from the directory the team lives in, or pass `--dir PATH` (created if missing).

- No flags: fully headless. No port, no terminal UI. This is the default.
- `--port N`: serve the web UI on 127.0.0.1:N.
- `--tui`: the terminal UI. It needs a terminal; without one it exits with code 2. `--port` and `--tui` cannot be combined.
- `--dir PATH`: the team's directory.
- `--instructions TEXT` or `--instructions-file PATH`: the first agent's instructions. Its flags `--workers N`, `--leads N`, `--per-lead N` say how many agents run and are put into those instructions.
- `--resend-instructions`: send the first agent its instructions again, even if it already exists.
- `fantastic_team mcp`, `fantastic_team send`, `fantastic_team readme` run fantastic's own commands in this directory (`readme` is fantastic's readme, not this file).

When the team starts it reads `team.yaml` from its directory (if there is one) and creates the agent tree from it, then sends the instructions to the first agent.

## 3. team.yaml

`examples/team.yaml` is the reference. Copy it into the team's directory and edit it. Its sections:

- `templates`: one per agent kind. Each has `handler_module`, `model`, `endpoint`, `credentials` (a store and a ref), prices, `can_call` (what the agent may reach) and `extra`.
- `first_agent`: `id`, `template`, `directive_file`.
- `connectors`: for example `git` (`kind: team_git.tools`, with `repo_dir`, `workspace_root`, `remote`, `default_branch`) and a `directive` that says what its agents may do.
- `queue`: the queue agent's id.
- `credentials`: `backend: env`, and per ref the host and the NAMES of the environment variables that hold the key (for example `ANTHROPIC_API_KEY`).

Keys are never written in `team.yaml`. Set the environment variables named there in your shell, and tell the user which names to set.

## 4. Run it on a remote box

Remote scripts run from the laptop, in `scripts/`:

- `remote-add.sh <name> <host>:<dir> [--bin PATH] [--port N] [--keys "NAME …"] [--dry-run]`: once, to save the remote.
- `remote-start.sh <name> [--port N] [--dry-run]`: start it detached. The values of the `--keys` variables are read on the laptop and sent over SSH stdin. They never appear in an argument list or a file.
- `remote-ls.sh [--dry-run]`: every remote and its state.
- `remote-attach.sh <name> [--port N] [--dry-run]`: the web UI through a tunnel, at http://127.0.0.1:N. `--tui` is not supported yet.
- `remote-logs.sh <name> [-f] [-n LINES]`: the team's log.
- `remote-stop.sh <name> [--now] [--yes] [--dry-run]`: drains the team (SIGTERM). `--now` kills it. It asks first unless `--yes`.

## 5. Give it work

Work comes from GitHub issues. A maintainer adds the label `team-ready` to an issue, and a `## SPEC` comment is written by the maintainer or the app. The puller (`crates/team/src/puller.rs`) takes only issues whose latest `team-ready` event is by the maintainer. It writes `queue/<repo>-<n>.md` and a pending `.state` file. It never overwrites a state file. Read `queue/` to see what was taken.

## 6. Rules

- Never put a key, token or password in a file, an argument, a commit or a log. Keys come from the environment variables named in `team.yaml`. Run `scripts/check-secrets.sh .` before you commit.
- Never push to the default branch (`main`), and never force-push. Workers push work branches through the git connector only.
- Merging is the maintainer's decision.
- Ask before stopping a running team. `remote-stop.sh` asks unless `--yes` is given.

## 7. Troubleshooting

- `--tui needs a terminal`: run without `--tui`, or use `--port N`.
- `--port and --tui cannot be combined`: drop one of them.
- `cannot use --dir …`: the directory cannot be created or entered. Fix its path or permissions.
- `… is not set on this laptop`: export the variable named in the remote's keys, then run `remote-start.sh` again.
- `no remote \`name\``: add it first with `remote-add.sh`.
- `already running · pid …`: the team is up. Use `remote-stop.sh` before starting again.
