# fantastic_shift

Unattended coding shifts on [fantastic](https://github.com/aisixteen/fantastic). A **driver** hands tasks from a **queue** to AI **worker lanes**; each lane works in its own git worktree and hands back a branch ready for review. You review and merge.

- **Any git repository.** Public or private, GitHub or not. The shift only needs a git remote it can push branches to. GitHub (or another forge) is an optional agent that opens pull requests and reads issues.
- **Any model mix.** Lanes are fantastic AI agents: Anthropic (Haiku, Sonnet, Opus) over the API with your own key, or local models (vLLM / llama.cpp / Strata) on your own GPU. Mix them; compare them.
- **Built on fantastic's Rust crates.** The `fantastic_shift` binary is a fantastic host: the kernel and fantastic's bundles (AI backends, exec, files, memory, credentials) plus this repo's own bundles. The HTTP canvas is optional.

## How a shift runs

```
 queue (task specs) ──▶ dispatcher ──▶ worker lane (AI agent, own worktree)
                           ▲                 │  edit · build · test · commit
                           │                 ▼
                           └──── done ◀── forge agent: push the branch, open a draft PR
```

1. A **task** is an exact spec: files and lines to read, the change, the tests, the commands. Specs are written by a strong model or a person; lanes implement them.
2. The **dispatcher** gives the next task to whichever lane finished, the lane with the fewest tasks first. A lane is busy until the task it was handed writes its own `done`.
3. A **lane** never holds credentials. It commits locally; pushing and opening a pull request go through the **forge agent**, which enforces the policy (no default branch, no force-push, drafts only) and keeps tokens to itself.
4. Every request's tokens and cost are logged per lane and per task.

## Parts (this repo)

| Part | What it is |
|---|---|
| `fantastic_shift` binary | Headless fantastic host: kernel + fantastic bundles + the bundles below. `--canvas [port]` adds the HTTP canvas. |
| `git` agent | Worktrees, branches, commits, pushes to a plain git remote (SSH or HTTPS). Works for any repository. |
| `github` agent (optional) | A GitHub App identity: push, draft PRs, read issues, labels, comments, PR status. Key in the credential store. |
| `queue` agent | The task queue and the dispatcher, as an agent (replaces the station scripts). |
| lane templates | Records for worker lanes (Haiku, Qwen, …) and the spec template. |
| `exec` sandbox | Lanes run commands with a sandbox `HOME`: no forge credentials, no SSH agent. |

## Status

Bootstrapping. The work is tracked in this repo's issues; the code is written by fantastic's own lanes from exact specs.

License: AGPL-3.0-or-later, like fantastic.
