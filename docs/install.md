# Install: zero to a first team

This guide installs `fantastic_team` from source. It covers what is in the repo on `main` today and nothing else.

## Status

- There are **no releases yet**. Build from source. Releases are tracked in #15 and #251.
- `fantastic` (the kernel) is a **git dependency** on branch `main` (see `Cargo.toml`). `Cargo.lock` pins the commit. There is no separate `fantastic_shift` binary in this repo.
- The GitHub App manifest flow is **not in the repo**. You register the app by hand (step 3).

## 1. Prerequisites

- Linux or macOS. Verified on Linux only; macOS: not yet verified.
- `git` and a Rust toolchain with `cargo`.
- `gh` (optional, for the GitHub CLI checks the team's worker uses; `gh --version` must run).

## 2. Build

    git clone https://github.com/aisixteen/fantastic_team
    cd fantastic_team
    cargo build --release

The binary is `target/release/fantastic_team`.

Check it runs:

    ./target/release/fantastic_team --help

Output (verified on this box):

    usage: fantastic_team [--dir PATH] [--port N | --tui]
      no surface flag: fully headless (no port, no terminal UI)
      --port N: serve the web UI on 127.0.0.1:N
      --tui: the terminal UI (needs a terminal)

There is no `--version` flag.

## 3. Register the GitHub App

Only needed for the GitHub side (pull requests, issue comments, labels, reading the task queue). Register your own app; the manifest flow is not in this repo.

Give the app these permissions:

- Contents: read and write
- Pull requests: read and write
- Issues: read and write
- Checks: read
- Metadata: read

Then store its `app_id` and its PEM `private_key` under one ref in fantastic's credential store, bound to `api_url` (default `https://api.github.com`). The key stays in the store. Never write it in a file, a config, or a doc.

## 4. Models and keys

Keys go in the credential store, never in a file. This repo does not define provider environment variable names, so none are listed here. The GitHub app key goes in the store as in step 3.

For remote boxes, `scripts/remote-start.sh` reads the values of the variable names passed in `--keys` on your laptop and pipes them to the remote shell over SSH stdin. They are never in argv, a file, or shell history.

## 5. Minimal config and start

The example config is `examples/team.yaml` (one master, one group, two workers). Start the team headless:

    ./target/release/fantastic_team --dir ./my-team

Serve the web UI on a port:

    ./target/release/fantastic_team --dir ./my-team --port 8123

Terminal UI:

    ./target/release/fantastic_team --dir ./my-team --tui

Run it on a remote box with the scripts (each prints its usage with `--help`; all support `--dry-run`):

- `scripts/remote-add.sh <name> <host>:<dir> [--bin PATH] [--port N] [--keys "NAME …"]`
- `scripts/remote-start.sh <name> [--port N]` (starts detached; the binary binds 127.0.0.1 only)
- `scripts/remote-attach.sh <name> [--port N] [--tui]`
- `scripts/remote-logs.sh <name> [-f] [-n LINES]`
- `scripts/remote-ls.sh`
- `scripts/remote-stop.sh <name> [--now] [--yes]`

## 6. Label a first issue, watch, review

1. In the target repo, label an issue `team-ready`.
2. Add a comment that starts with `## SPEC` and holds the task. The puller takes it only when the latest label event is by the maintainer and the `## SPEC` comment is by the maintainer or the app.
3. The team's puller (`team_puller.tools`) reads it and writes `queue/<repo>-<n>.md` plus a pending `.state`. Its `status` verb shows progress.
4. The team opens a **draft** pull request. Review it on GitHub. Merging stays human.

## 7. Troubleshooting

| Symptom | Fix |
|---|---|
| `no ## SPEC comment by the maintainer or the app` | Add the `## SPEC` comment yourself, or let the app author it; comments by others are ignored. |
| Issue is not picked up | The latest `team-ready` label event must be by the maintainer. Re-add the label as the maintainer. |
| `gh --version` fails in the worker | Install `gh` and put it on PATH. |
| Secrets check fails on commit | `scripts/check-secrets.sh .` found a key or token. Remove it and store it in the credential store. |
| Remote start leaves no keys | Name the variables in `--keys` on `remote-add.sh`; `remote-start.sh` reads them on your laptop. |

## Tested on

Linux, this box: built with `cargo build -p fantastic-team --bin fantastic_team` (exit 0), ran `--help` (above), and ran `--help` on each `scripts/remote-*.sh`. Not run: `--port`, `--tui`, a live GitHub app, real keys. macOS: not yet verified.
