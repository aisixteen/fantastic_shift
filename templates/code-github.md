<!-- template: code-github
field REPO
field ISSUE
field BRANCH
field BASE=main
field CHECK
field GIT=git
field GITHUB=github
-->
# {{TITLE}}

_Task kind: code-github · written {{DATE}} by {{AUTHOR}}._ Change code in `{{REPO}}` for issue #{{ISSUE}} and deliver it as a draft pull request.

## Rules — read first
- Do only what this task says. Change only the files it names; never touch anything outside your worktree.
- Run commands with your exec tool; read and change files with your files tool. Read a file before you edit it.
- Never write a real key, token or password into any file, command or message. A test that needs one builds a fake one from pieces at run time.
- Never push to `{{BASE}}`, never force-push, never merge: the connectors refuse them anyway. Pull requests are drafts.
- In the pull request text write `For #{{ISSUE}}`. Never write "close", "fix" or "resolve" next to an issue number: GitHub would close the issue on merge.
- The same failure after two honest fix attempts: stop, and hand the task back as failed with the step, the error and your diagnosis.

## The cycle (do all of it, in order)
1. **The issue.** Ask the `{{GITHUB}}` connector for issue #{{ISSUE}} of `{{REPO}}` (verb `issue_view`) and read it whole, with its comments.
2. **Worktree.** Ask the `{{GIT}}` connector for a worktree on a new branch `{{BRANCH}}` from `{{BASE}}` (verb `worktree_add`; its `reflect` names the fields). Work only inside the path it returns (WT).
3. **Read, change, test** — the task below.
4. **Check.** In WT run `{{CHECK}}`. It must pass. Fix what fails, then run it again.
5. **Commit.** Ask the `{{GIT}}` connector to `commit` WT with the message the task gives.
6. **Push.** Ask the `{{GITHUB}}` connector to `push` branch `{{BRANCH}}` to `{{REPO}}`.
7. **Draft PR.** Ask the `{{GITHUB}}` connector to `pr_create` on `{{REPO}}`: base `{{BASE}}`, head `{{BRANCH}}`, the title the task gives, and a body that starts with `For #{{ISSUE}}.` and says what changed and how it was checked.
8. **Clean up.** Ask the `{{GIT}}` connector to `worktree_remove` WT.
9. **Report** in one short paragraph: the PR link, the commit, the files changed, the tests added, what `{{CHECK}}` said.

## The task
<!-- WRITE: why the change is needed, in one or two sentences. -->

**Read first:**
<!-- WRITE: the files to read, with paths relative to WT, and what to look for in each (a function, a line range). -->

**Change:**
<!-- WRITE: each change exactly: the file, the function or line, the new code or its precise behaviour, every message text. -->

**Tests:**
<!-- WRITE: each new test: its name, its file, what it sets up and what it asserts. -->

**Commit message:** <!-- WRITE: one line, then an optional paragraph. -->

**PR title:** <!-- WRITE: one line, ending with (#ISSUE). -->
