"""How a fantastic_team team works, as a short manim animation.

Render (low quality, to check it runs):
    manim -ql how_it_works.py HowItWorks
Render (final 1080p60):
    manim -qh --format mp4 -o how_it_works.mp4 how_it_works.py HowItWorks
"""

from manim import *

BG = "#0b0d12"
CYAN = "#4fe0e6"
BLUE = "#5fa8e0"
VIOLET = "#7b86d6"
GREY = "#5a6170"
PANEL = "#14181f"


def box(width, height, color=VIOLET):
    return RoundedRectangle(
        corner_radius=0.12, width=width, height=height,
        stroke_color=color, fill_color=PANEL, fill_opacity=1,
    )


def label(text, size=24, color=WHITE):
    return Text(text, font_size=size, color=color)


class HowItWorks(Scene):
    def construct(self):
        self.camera.background_color = BG
        self.caption_mob = None
        self.cards = None
        self.workers = []
        self.dots = []
        self.dispatcher = None
        self.delivery = None

        self.title_beat()
        self.queue_beat()
        self.workers_beat()
        self.handout_beat()
        self.work_beat()
        self.delivery_beat()
        self.measured_beat()
        self.review_beat()
        self.end_beat()

    # helpers

    def caption(self, text):
        new = label(text, 26).to_edge(DOWN, buff=0.35)
        if self.caption_mob is None:
            self.play(FadeIn(new), run_time=0.6)
        else:
            self.play(FadeOut(self.caption_mob, run_time=0.4),
                      FadeIn(new, run_time=0.4))
        self.caption_mob = new

    def clear_all(self):
        self.play(FadeOut(*self.mobjects), run_time=0.8)
        self.caption_mob = None

    # beats

    def title_beat(self):
        title = label("fantastic_team", 64, CYAN)
        sub = label("unattended work teams", 32)
        sub.next_to(title, DOWN, buff=0.4)
        self.play(FadeIn(title), FadeIn(sub), run_time=1.5)
        self.wait(1.5)
        self.play(FadeOut(title), FadeOut(sub), run_time=1)

    def queue_beat(self):
        texts = [
            "fix bug #12",
            "write the setup guide",
            "compare 5 tools",
            "triage 40 issues",
            "clean data.csv",
        ]
        self.cards = VGroup()
        for i, txt in enumerate(texts):
            b = box(4.0, 0.8, CYAN)
            t = label(txt)
            g = VGroup(b, t).move_to([-4.9, 2.2 - i, 0])
            self.cards.add(g)
        self.play(LaggedStart(*[FadeIn(c) for c in self.cards], lag_ratio=0.2),
                  run_time=3)
        self.caption("you queue tasks — any kind of work")
        self.wait(2.5)

    def workers_beat(self):
        disp_box = box(2.6, 1.0, BLUE)
        disp_lbl = label("the lead (AI)")
        self.dispatcher = VGroup(disp_box, disp_lbl).move_to([0, 0.6, 0])

        names = ["worker · Haiku", "worker · Haiku", "worker · Qwen (local GPU)"]
        ys = [2.4, 0.6, -1.2]
        self.workers = []
        self.dots = []
        for name, y in zip(names, ys):
            wb = box(4.6, 1.4, VIOLET)
            wl = label(name).move_to([4.4, y + 0.15, 0])
            dot = Dot(radius=0.12, color=GREY).move_to([6.4, y + 0.4, 0])
            self.workers.append(VGroup(wb.move_to([4.4, y, 0]), wl))
            self.dots.append(dot)

        arrows = VGroup(*[
            Arrow(start=[1.3, 0.6, 0], end=[2.0, y, 0], buff=0, stroke_width=3)
            for y in ys
        ])
        self.play(FadeIn(self.dispatcher), run_time=1)
        self.caption("the lead, an AI agent, spawns its workers — any model")
        centre = self.dispatcher.get_center()
        for w, d in zip(self.workers, self.dots):
            self.play(GrowFromPoint(w, centre), FadeIn(d), run_time=1)
        self.play(FadeIn(arrows), run_time=0.8)
        self.wait(1.5)

    def handout_beat(self):
        self.caption("the next task goes to whichever worker is free")
        ys = [2.4, 0.6, -1.2]
        for idx in range(3):
            card = self.cards[idx]
            self.play(card.animate.scale(0.4).move_to([4.4, ys[idx], 0]),
                      run_time=1.2)
            self.play(FadeOut(card, run_time=0.3),
                      self.dots[idx].animate.set_color(CYAN),
                      run_time=0.4)
        self.wait(1)

    def work_beat(self):
        self.caption("each worker works alone, from exact instructions")
        pos = [4.4, 2.05, 0]
        for word in ["read", "do", "check"]:
            t = label(word, 24, CYAN).move_to(pos)
            self.play(FadeIn(t), run_time=0.5)
            self.wait(0.8)
            self.play(FadeOut(t), run_time=0.3)
        check = Text("✓", font_size=40, color=CYAN).move_to(pos)
        self.play(FadeIn(check), run_time=0.4)
        self.play(self.dots[0].animate.set_color(GREY), run_time=0.4)
        self.wait(1)

    def delivery_beat(self):
        self.caption("the delivery agent holds the keys — workers hold none")
        self.play(FadeOut(self.cards[3], self.cards[4]), run_time=0.6)
        bx = box(3.4, 0.9, VIOLET).move_to([0, -2.4, 0])
        body = RoundedRectangle(corner_radius=0.04, width=0.3, height=0.25,
                                stroke_color=CYAN, fill_color=CYAN,
                                fill_opacity=1).move_to([-1.3, -2.4, 0])
        shackle = Arc(radius=0.1, start_angle=0, angle=PI,
                      color=CYAN).next_to(body, UP, buff=0)
        lbl = label("delivery agent").move_to([0.35, -2.4, 0])
        self.delivery = VGroup(bx, body, shackle, lbl)
        self.play(FadeIn(self.delivery), run_time=1)

        finished = VGroup(box(3.0, 0.6, CYAN), label("fix bug #12"))
        finished.move_to([0, 0.6, 0])
        self.play(FadeIn(finished), run_time=0.6)
        self.play(finished.animate.move_to([0, -2.4, 0]).scale(0.3),
                  run_time=1.2)
        self.play(FadeOut(finished, run_time=0.3))

        outs = VGroup(*[label(t) for t in
                        ["files", "report", "comment", "branch / PR"]])
        outs.arrange(RIGHT, buff=0.35).move_to([0.2, -1.4, 0])
        self.play(LaggedStart(*[FadeIn(o) for o in outs], lag_ratio=0.5),
                  run_time=2.5)
        self.wait(1.5)

    def measured_beat(self):
        self.clear_all()
        headers = ["task", "worker", "time", "tokens", "cost"]
        rows = [
            ["fix bug #12", "Haiku", "3 min", "0.7M", "$0.03"],
            ["write setup guide", "Qwen", "6 min", "1.2M", "$0.00"],
            ["triage 40 issues", "Haiku", "4 min", "0.5M", "$0.02"],
        ]
        xs = [-4.6, -1.4, 1.2, 3.6, 5.6]
        ys = [2.4, 1.2, 0.2, -0.8]
        cells = VGroup()
        for r, row in enumerate([headers] + rows):
            for c, val in enumerate(row):
                colour = CYAN if r == 0 else WHITE
                cells.add(label(val, 24, colour).move_to([xs[c], ys[r], 0]))
        line = Line([-6.5, 1.8, 0], [6.5, 1.8, 0], color=GREY)
        self.play(FadeIn(cells), Create(line), run_time=2)
        self.caption("every task: time, tokens, cost")
        self.wait(3)

    def review_beat(self):
        self.clear_all()
        head = Circle(radius=0.45, color=CYAN).move_to([-2, 1.0, 0])
        body = RoundedRectangle(corner_radius=0.4, width=1.6, height=1.2,
                                color=CYAN).move_to([-2, -0.4, 0])
        person = VGroup(head, body)
        items = VGroup(*[
            VGroup(box(4.4, 0.6, CYAN), label(t)).move_to([3.5, y, 0])
            for t, y in [("fix bug #12 — PR draft", 1.8),
                         ("setup guide — file", 0.4),
                         ("triage — 40 labels", -1.0)]
        ])
        self.play(FadeIn(person), run_time=1)
        self.play(LaggedStart(*[FadeIn(i) for i in items], lag_ratio=0.4),
                  run_time=2)
        tick = Text("✓", font_size=56, color=CYAN).move_to([-0.1, 0.4, 0])
        self.play(FadeIn(tick), run_time=0.5)
        self.caption("you come back to done work — and review it")
        self.wait(1.5)

    def end_beat(self):
        self.clear_all()
        end = label("github.com/aisixteen/fantastic_team", 36, CYAN)
        self.play(FadeIn(end), run_time=1)
        self.wait(2.5)
        self.play(FadeOut(end), run_time=0.8)
