# Security review: permissions and threat model (#33)

## Scope and date

- Scope: the GitHub connector (`crates/github_connector`), the git connector push path (`crates/git_connector`), and the team worker sandbox (`crates/team/src/worker_exec.rs`).
- Reviewed commit: the HEAD of `team/33-security-review` at the time of the inventory, branched from `origin/main` (24dad4b).
- Method: grep inventory, then file reads of the cited sites. Findings are listed only; fixes are follow-up items.

## Permission table

Granted (per `crates/github_connector/src/readme.md:13`): contents write, pull requests write, issues write, checks read, metadata read. The spec's set adds actions read.

| permission | granted | used by (file:line) | verdict |
|---|---|---|---|
| metadata | read | `verbs.rs:203` GET `/repos/{repo}`; `app.rs:122` GET installation lookup | needed |
| contents | write | `push.rs:149` git push of a work branch (via git refspec, `push.rs:134`) | needed |
| issues | read | `verbs.rs:271, 319, 329, 383, 522, 701` (list, view, comments, events, labels) | needed |
| issues | write | `verbs.rs:435` POST comment; `492` POST label; `505` DELETE label; `750` PATCH issue | needed |
| pull requests | write | `verbs.rs:223` POST create PR | needed |
| pull requests | read | `verbs.rs:604` GET PR; `625` GET reviews | needed |
| checks | read | `verbs.rs:615` GET check-runs | needed |
| actions | read | no call found in the inventory | can drop |
| projects (org) | not granted in the readme list | `verbs.rs:795` POST `/graphql`, mutation `updateProjectV2ItemFieldValue` (`verbs.rs:790`) | undeclared: the call needs a Projects permission that the readme does not list. Either the app holds it (undocumented) or the verb fails. Verify, then declare or drop the verb. |
| app JWT | n/a | `verbs.rs:162` GET `/app` (whoami) | app-level, no repo permission |

No call in the inventory uses a PUT, a merge endpoint, a ref-delete or a force-ref update.

## Threat model

Assets: the GitHub App private key (PEM) and the installation tokens minted from it; the protected default branch; the repos on the allowlist.

Actors:
- Maintainer: configures the connector and the credential store.
- Worker model: runs in a sandboxed shell with a fixed environment; treated as untrusted.
- Untrusted issue text: issue bodies and comments reach the team's agents.
- Forge: GitHub, trusted only for the API it serves.

Trust boundaries:
- credential store to connector (the key is read here);
- connector to worker (the worker has no credential);
- worker shell to git connector (push is checked here);
- connector to GitHub API (a token is sent only to the host that minted it, `push.rs:94`).

## Claims verified

| claim | evidence | holds / does not hold |
|---|---|---|
| Keys in RAM only | `app.rs:43`: the key is read from the credential store and used as this agent; `app.rs:3`: tokens are never cached, returned, stored or logged; no `fs::write`, `println!` or logging in non-test `github_connector/src` (grep). Disk writes exist only in tests (`tests.rs:30`, `143`). | holds for the connector code. Not verified: whether `git::run` (`push.rs` auth argument) leaves the token in process arguments or the git config; see gap 2. |
| Workers hold no credentials | `worker_exec.rs:39-40`: `GH_TOKEN` and `GITHUB_TOKEN` are not in the worker env list; `worker_exec.rs:47-50`: credential files are refused; `worker_exec.rs:97`: a sandbox home holding a credential is refused; `worker_exec.rs:66`: `WORKER_ALLOWED` refuses `gh`, `glab`, `hub`, `git push`, `git credential`, `git remote set-url`, `credential.helper` and `api.github.com`. | holds for the environment and home. The command filter is a textual blocklist (`worker_exec.rs:11` notes the shell can be reached in many ways); the real boundary is that no credential is reachable. |
| Main protected | `git.rs:469-474`: force is refused; `git.rs:483-485`: the default branch, `main` and `master` are refused; `git.rs:512`: the refspec is `refs/heads/<b>:refs/heads/<b>` (no `+`); `push.rs:54`: the GitHub push calls the same `git::check_push`. | holds for the push path. Protection is by name (the record's default branch plus literal `main` and `master`), not by GitHub branch protection. |
| No merge verb | no merge endpoint in the inventory; `tests.rs:416` sends a `merge` type and expects refusal; `directives.rs:173` lists "never merge" as a directive. | holds |

## Gaps and risks, ranked

1. **Medium (high if the app holds Projects write without a declared need): undeclared Projects permission.** `verbs.rs:795` calls GraphQL `updateProjectV2ItemFieldValue`; the readme permission list does not include Projects.
   - Fix: confirm the app's granted permissions on GitHub. Declare Projects write in the readme if it is needed, or remove `project_field_set` if it is not.
   - Issue title: "Declare or remove the Projects write permission used by project_field_set".

2. **Medium: token exposure in process arguments is not verified.** `push.rs:149` passes `url.as_str()` and a refspec to `git::run`; the auth is passed separately (`push.rs` `auth`). If any path puts the token in the URL, it is visible in process listings.
   - Fix: pass the credential by an environment variable or a credential helper that is not in argv; add a test that the token never appears in the argv.
   - Issue title: "Keep the installation token out of git argv".

3. **Medium: untrusted issue text reaches a model that runs shell commands.** Issue bodies and comments (`verbs.rs:271, 319, 329, 383`) feed agents; the worker shell (`worker_exec.rs:11`) runs model commands.
   - Fix: keep the credential-free sandbox as the only shell; treat issue text as data in prompts; add a test with hostile issue text.
   - Issue title: "Treat issue text as untrusted input to worker prompts".

4. **Low: unused actions read permission.** No call found.
   - Fix: drop actions read from the app.
   - Issue title: "Drop unused actions read permission from the GitHub App".

5. **Low: branch protection is name-based.** `git.rs:483` protects only the default branch, `main` and `master`.
   - Fix: also enable GitHub branch protection on the default branch, and allow the connector to refuse any branch listed in a deny set.
   - Issue title: "Add GitHub branch protection as a second layer for main".

6. **Low (known): macOS sandbox limit.** Tracked in #8.

## Untrusted input

- Issue text, comments and PR bodies: read by `verbs.rs:271, 319, 329, 383, 604, 625` and passed to agents. Reach a model: yes. Reach a shell: through the worker, which runs model-written commands in a credential-free sandbox (`worker_exec.rs`).
- Branch names: validated in `git.rs` `check_push` (protected names, force) before the refspec is built.
- Not reviewed in this pass: prompt construction for the reviewer and qwen agents.

## Checks

- Citations come from grep on the reviewed commit; `verbs.rs:770-836` was read directly. Five spot checks were taken from the grep output and cross-checked against the surrounding code reads above.
- `scripts/check-secrets.sh .`: run by the cycle step 3.
- `git diff --stat origin/main`: expected to show only `docs/security-review.md`.
