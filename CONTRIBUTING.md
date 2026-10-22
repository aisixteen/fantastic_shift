# Contributing to fantastic_team

fantastic_team runs unattended team work on [fantastic](https://github.com/aisixteen/fantastic).
It is **AGPL-3.0-or-later**.

> **Outside pull requests are not accepted.** Only the maintainer and the AISixteen Fantastic Team
> GitHub App open pull requests here. **Bug reports and ideas are welcome** as issues and
> discussions; security problems go through private reporting ([`SECURITY.md`](SECURITY.md)).

> **Alpha.** Configuration (`team.yaml`), verbs and on-disk formats still change. See the
> roadmap, issue #1.

## How work happens here

- Issues carry the spec. An issue labelled `team-ready` with a `## SPEC` comment may be taken
  by a team worker; `team-pr` marks the PR it opened. `aiagent` is set by the maintainer only.
- Workers open **draft** PRs. The maintainer reviews and approves; the team merges only PRs
  approved on their current head.
- CI is run by hand (#46): units by default, the e2e for a finished milestone or a release.

## Building

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --check && cargo clippy --workspace --all-targets
sh scripts/check-secrets.sh .
```

fantastic is a dependency: semver releases by default, a git branch or a local checkout as
overrides (`.cargo/config.toml.example`, `scripts/update-fantastic.sh`).

## Rules

- No key, token or private path anywhere in the repo. Tests build fake tokens from pieces.
- No provider or model names in code; they belong in configuration.
- Workers get no forge credentials, enforced in code; never weaken it.

## Licensing of contributions (DCO)

Your contribution is licensed under **AGPL-3.0-or-later**, and you certify the
[Developer Certificate of Origin](https://developercertificate.org/). Sign off each commit with
`git commit -s`. The "Aisixteen" and "fantastic" names are trademarks of the author; the AGPL
covers the code only, so a fork must rename.
