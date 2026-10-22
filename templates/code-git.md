<!-- template: code-git
field BRANCH
field BASE=main
field CHECK
field GIT=git
-->
# {{TITLE}}

_Task kind: code-git · written {{DATE}} by {{AUTHOR}}._ Change code in a git repository and deliver it as a pushed branch.

## Rules — read first
- Do only what this task says. Change only the files it names; never touch anything outside your worktree.
- Run commands with your exec tool; read and change files with your files tool. Read a file before you edit it.
- Never write a real key, token or password into any file, command or message. A test that needs one builds a fake one from pieces at run time.
- Never push to `{{BASE}}`, never force-push, never merge: the `{{GIT}}` connector refuses them anyway.
- The same failure after two honest fix attempts: stop, and hand the task back as failed with the step, the error and your diagnosis.

## The cycle (do all of it, in order)
1. **Worktree.** Ask the `{{GIT}}` connector for a worktree on a new branch `{{BRANCH}}` from `{{BASE}}` (verb `worktree_add`; its `reflect` names the fields). Work only inside the path it returns (WT).
2. **Read, change, test** — the task below.
3. **Check.** In WT run `{{CHECK}}`. It must pass. Fix what fails, then run it again.
4. **Commit.** Ask the `{{GIT}}` connector to `commit` WT with the message the task gives.
5. **Push.** Ask the `{{GIT}}` connector to `push` branch `{{BRANCH}}`.
6. **Clean up.** Ask the `{{GIT}}` connector to `worktree_remove` WT.
7. **Report** in one short paragraph: the branch, the commit, the files changed, the tests added, what `{{CHECK}}` said.

## The task
<!-- WRITE: why the change is needed, in one or two sentences. -->

**Read first:**
<!-- WRITE: the files to read, with paths relative to WT, and what to look for in each (a function, a line range). -->

**Change:**
<!-- WRITE: each change exactly: the file, the function or line, the new code or its precise behaviour, every message text. -->

**Tests:**
<!-- WRITE: each new test: its name, its file, what it sets up and what it asserts. -->

**Commit message:** <!-- WRITE: one line, then an optional paragraph. -->
