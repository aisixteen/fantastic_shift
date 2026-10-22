# shift_queue — tasks as data

A task is data: an id, a title, instructions (the text, or `instructions_file`, a path the worker reads), a status, the taker who holds it, a lease, and a result. The queue stores and hands out tasks; it never reads the instructions and never interprets the work. The order is "oldest pending first" and nothing more.

| verb | args | does |
|---|---|---|
| `add` | `title`, `instructions` or `instructions_file`, `id`?, `data`? | appends a pending task |
| `list` | `status`? | tasks in insertion order |
| `get` | `id` | one task |
| `take` | `taker`, `lease_s`? | the oldest pending task becomes taken under a lease |
| `renew` | `id`, `taker`, `lease_s`? | extends the lease |
| `done` | `id`, `taker`, `status` (done or failed), `result` | finishes a task the taker holds |
| `release` | `id`, `taker` | puts a held task back to pending |
| `status` | — | counts per status |
| `reflect` | — | what the queue is, and its counts |

## Leases

A taken task returns to `pending` when its lease ends, or when its taker is no longer an agent on this kernel. Both are checked at every call; there is no timer.

## Persistence

Tasks persist in the `tasks.json` sidecar through the root loader (`kernel_state.tools`). With no store wired, every verb except `reflect` answers the loader's error.
