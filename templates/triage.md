<!-- template: triage
field REPO
field LABEL
field OUT
field GITHUB=github
-->
# {{TITLE}}

_Task kind: triage · written {{DATE}} by {{AUTHOR}}._ Sort the open issues of `{{REPO}}` labelled `{{LABEL}}` and write a triage report. Propose; do not act.

## Rules — read first
- Read issues only through the `{{GITHUB}}` connector (`issues`, `issue_view`). Do not comment, label, close or assign anything: the report proposes, a person decides.
- Judge each issue only by its text and its comments. Say "unclear" rather than guess.
- Never copy a key, token or password into the report, even one you find in an issue.
- Write only the file `{{OUT}}`. Change nothing else.
- The same failure after two honest attempts: stop, and hand the task back as failed with the step, the error and your diagnosis.

## The cycle (do all of it, in order)
1. **List** the open issues of `{{REPO}}` with the label `{{LABEL}}` (verb `issues`).
2. **Read** each one whole with its comments (verb `issue_view`).
3. **Sort** each issue into one of: `bug`, `feature`, `question`, `duplicate of #N`, `needs info`, `out of scope` — by the rules of the task below.
4. **Write** `{{OUT}}` with your files tool: a Markdown table with the columns `#`, `title`, `kind`, `priority (P1–P3)`, `next step`, `why` (one line each), one row per issue in number order, then `## Notes` for anything that does not fit a row (or `none`).
5. **Check** that every listed issue has exactly one row, and every `duplicate of #N` names an issue that exists.
6. **Report** in one short paragraph: the path `{{OUT}}`, how many issues of each kind, and the P1s.

## The task
**How to sort:** <!-- WRITE: what makes an issue a bug, a feature, out of scope here; how to pick a priority. -->

**What to leave alone:** <!-- WRITE: issues or labels to skip, if any; otherwise write: nothing. -->
