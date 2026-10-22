# How a shift works: the animation

A one-minute manim animation of how a fantastic_shift shift runs.

- **Title:** fantastic_shift, unattended work shifts.
- **The queue:** you queue tasks, of any kind of work.
- **The workers:** a dispatcher and a mix of AI workers (Haiku, Qwen on a local GPU).
- **Hand-out:** the next task goes to whichever worker is free.
- **The work:** each worker works alone, from exact instructions (read, do, check).
- **Delivery:** the delivery agent holds the keys; workers hold none. Results come back as files, a report, a comment, or a branch / PR.
- **Measured:** every task records time, tokens and cost.
- **You review:** you come back to done work and review it.
- **End:** github.com/aisixteen/fantastic_shift.

## Re-render

Source: `how_it_works.py`, scene `HowItWorks`. It needs ffmpeg, cairo and pango:

```
brew install ffmpeg cairo pango
```

Low quality, to check it runs:

```
manim -ql how_it_works.py HowItWorks
```

Final 1080p60 mp4:

```
manim -qh --format mp4 -o how_it_works.mp4 how_it_works.py HowItWorks
```
