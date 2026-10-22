<!-- template: docs
field BRANCH
field BASE=main
field CHECK=git diff --check
field GIT=git
-->
# {{TITLE}}

_Task kind: docs · written {{DATE}} by {{AUTHOR}}._ Write or update documentation in a git repository and deliver it as a pushed branch.

## Rules — read first
- Change only the documents the task names. Never change code.
- Write plainly: short sentences, the reader's words, one idea per paragraph. Every command and path you show must be one that exists: check it.
- Never write a real key, token or password, not even as an example: use a placeholder such as `<your key>`.
- Never push to `{{BASE}}`, never force-push, never merge.
- The same failure after two honest fix attempts: stop, and hand the task back as failed with the step, the error and your diagnosis.

## The cycle (do all of it, in order)
1. **Worktree.** Ask the `{{GIT}}` connector for a worktree on a new branch `{{BRANCH}}` from `{{BASE}}` (verb `worktree_add`). Work only inside the path it returns (WT).
2. **Read** the documents and the code the task names, so what you write matches what is there.
3. **Write** — the task below.
4. **Check.** In WT run `{{CHECK}}`. It must pass. Each link to a file in the repo must name a file that exists.
5. **Commit** with the `{{GIT}}` connector (`commit`), with the message the task gives; **push** branch `{{BRANCH}}` (`push`); **clean up** (`worktree_remove`).
6. **Report** in one short paragraph: the branch, the commit, the documents changed.

## The task
<!-- WRITE: who reads these docs and what they must be able to do afterwards. -->

**Documents:**
<!-- WRITE: each document to create or change, its path relative to WT, and what goes in it (sections, examples). -->

**Facts to use:**
<!-- WRITE: the code, commands or issues the text must agree with. -->

**Commit message:** <!-- WRITE: one line. -->
