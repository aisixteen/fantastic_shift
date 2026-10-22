# Security Policy

## Supported versions

fantastic_shift is **pre-1.0 (alpha)**. Security fixes land on the **latest release line only**.

## Reporting a vulnerability

**Please do not open a public issue, PR, or discussion for a security problem.**

Use GitHub's **private vulnerability reporting**: the repository's **Security** tab →
**Report a vulnerability**. It creates a private advisory visible only to the maintainers.

Helpful to include: the affected part (headless binary, a connector, the remote runner,
the web UI, the app), a version or commit, the impact and a minimal reproduction.
**Never include a real key or token**; describe it instead.

We aim to acknowledge within a few business days and agree a fix and disclosure timeline
with the reporter.

## What the shift promises (and what counts as a vulnerability)

- **Workers hold no secrets.** A worker's exec gets no forge credentials and no forge CLIs;
  delivery goes through connector agents. A way for a worker to read a key, a token or the
  GitHub App key is a vulnerability.
- **Secrets never leave the credential store.** They are read at use and never returned,
  logged, stored in rows or written to disk or git. A key or token appearing in a reply,
  a log, a record, a PR or a commit is a vulnerability.
- **Remote keys stay in memory.** On a remote box, keys arrive over SSH stdin into the
  process environment only; never on its disk, in argv or in shell history.
- **Merges need the maintainer's approval on the current head.** A way for the shift to
  merge a PR without it is a vulnerability.
- Connections between shifts, and from the app, go through our relay as it is; this repo
  adds no auth layer of its own.
