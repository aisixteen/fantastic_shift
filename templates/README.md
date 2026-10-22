# Task templates

A task is one Markdown file a worker carries out alone, start to finish: what to read, what to change, how to check it, and how to deliver it. The worker's directive says how to claim a task and hand it back; the task says everything else. Unattended work is only as good as its task, so tasks are exact.

## The kinds

| Kind | For | Delivers |
|---|---|---|
| `code-git` | a code change in a git repository | a pushed branch, through the `git` connector |
| `code-github` | a code change for a GitHub issue | a draft pull request, through the `github` connector |
| `research` | a question answered from sources | a report file |
| `docs` | documentation written or updated | a pushed branch |
| `triage` | open issues sorted and prioritised | a report file; nothing is changed on GitHub |

Each template has three parts: **Rules** (what the worker must never do), **The cycle** (the steps, in order, from the start to the report) and **The task** (what you write).

## Making a task

```
scripts/new-task.sh --list
scripts/new-task.sh code-github "Retry a dropped connection" \
    --set REPO=owner/name --set ISSUE=42 --set BRANCH=fix/42-retry --set CHECK="cargo test"
```

This writes `tasks/retry-a-dropped-connection.md` (`--out FILE` to choose, `--print` to see it, `--force` to overwrite). Every `{{NAME}}` in the template is filled: `TITLE`, `KIND`, `DATE` and `SLUG` by the generator, `AUTHOR` from `$USER` (or `--set AUTHOR=…`), and the template's own fields from `--set`. A field with a default (`BASE=main`) may be left out; a required one may not, and the generator names what is missing.

Then fill every `<!-- WRITE: … -->` part by hand, and check:

```
scripts/new-task.sh --check tasks/retry-a-dropped-connection.md
```

It prints `…: ready`, or each line still holding a `{{…}}` or a `WRITE:`.

## Writing the task part

- **Name every file and place.** A path, and a function or a line range in it. The worker should never have to search.
- **Give the exact text.** New code, or its precise behaviour; every message a user sees, word for word.
- **Name every test.** Its name, its file, what it sets up and what it asserts, and the command that runs it.
- **One task, one change.** Two unrelated changes are two tasks.
- **Never a secret.** A key in a task ends up in logs and branches. Tests build fake keys from pieces at run time.
- **Issue numbers.** Write `For #N` in pull request text; "close", "fix" or "resolve" next to `#N` closes the issue on merge.

## Adding a kind

Add `templates/<kind>.md` starting with its header comment:

```
<!-- template: <kind>
field NAME
field NAME=default
-->
```

Every `{{NAME}}` the template uses must be a declared field or one the generator fills; `sh tests/new_task.sh` renders every template and fails on any `{{…}}` left over.
