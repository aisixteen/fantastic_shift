<!-- template: research
field OUT
-->
# {{TITLE}}

_Task kind: research · written {{DATE}} by {{AUTHOR}}._ Answer a question from sources and write the answer as a report file.

## Rules — read first
- Answer only the question below. Read only the sources it allows.
- Every claim in the report names its source: a path and line, or a link. Say "not found" rather than guess.
- Never copy a key, token or password into the report, even one you find in a source.
- Write only the file `{{OUT}}`. Change nothing else.
- The same failure after two honest attempts: stop, and hand the task back as failed with the step, the error and your diagnosis.

## The cycle (do all of it, in order)
1. **Read** the sources the task names, in that order. Note what each says about the question, with where it says it.
2. **Write** `{{OUT}}` with your files tool, in this shape:
   - `# ` the question, as one line;
   - `## Answer` — the answer in at most five sentences;
   - `## Evidence` — one bullet per finding: what it shows, and its source;
   - `## Open` — what the sources do not answer, one bullet each (or `none`).
3. **Check** that every bullet under Evidence has a source, and that the Answer says nothing the Evidence does not support.
4. **Report** in one short paragraph: the path `{{OUT}}` and the answer in one sentence.

## The task
**Question:** <!-- WRITE: the question, as one sentence. -->

**Sources:**
<!-- WRITE: the files, folders or links to read, and any the worker must NOT use. -->

**What the answer must cover:**
<!-- WRITE: the points the answer has to address (for example: options compared, numbers to find). -->
