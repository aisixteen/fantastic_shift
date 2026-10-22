<!--
Thanks for the PR. Keep it focused on one concern. Only the maintainer and the
AISixteen Fantastic Team app open pull requests here (see CONTRIBUTING.md).
-->

## What & why

<!-- What does this change, and why? Link the issue ("For #N"; a closing keyword closes it). -->

## Part(s) touched

- [ ] headless binary
- [ ] workers / lead / queue
- [ ] connectors (git, GitHub)
- [ ] web UI / app
- [ ] packs
- [ ] docs only

## Security

- [ ] No key, token or private path in code, tests, docs or commit messages (`scripts/check-secrets.sh .` is clean).
- [ ] Workers still get no forge credentials and no forge CLIs.

## Checks

- [ ] `cargo fmt --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace` pass

## Legal

- [ ] I license this contribution under **AGPL-3.0-or-later** and sign off the
      [DCO](https://developercertificate.org/) (`git commit -s`).
