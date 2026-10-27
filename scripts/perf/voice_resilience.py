"""Scores a voice_resilience.ps1 run: for each phase and direction, the share of
seconds in which one side was sending its tone and the other side actually played
something, the longest stretch it played nothing, and how long reception took to
come back after each join.

    python scripts/perf/voice_resilience.py <run folder>
"""
import csv
import sys
from collections import defaultdict
from pathlib import Path


def load(path):
    rows = list(csv.DictReader(open(path, encoding="utf-8")))
    t0 = {r["event"]: int(r["value"]) for r in rows if r["event"].startswith("t0_unix")}
    base = t0["t0_unix_s"] * 1000 + t0["t0_unix_ms"]
    return [(base + int(r["t_us"]) / 1000.0, r["event"], int(r["value"])) for r in rows
            if not r["event"].startswith("t0_unix")]


def seconds(events, name, pick=lambda v: True):
    """Count of `name` events per whole unix second."""
    out = defaultdict(int)
    for t, e, v in events:
        if e == name and pick(v):
            out[int(t // 1000)] += 1
    return out


def main(run):
    run = Path(run)
    marks = [(int(r["unix_ms"]), r["phase"], r["event"]) for r in csv.DictReader(open(run / "phases.csv"))]
    logs = {p.stem: load(p) for p in sorted(run.glob("*.csv")) if p.name != "phases.csv"}
    side = {"sender": [], "receiver": []}
    for name, events in logs.items():
        side[name.split("-")[0]].extend(events)
    for events in side.values():
        events.sort()

    # Phase windows: from each phase's first mark to its last.
    windows = {}
    for t, phase, _ in marks:
        start, end = windows.get(phase, (t, t))
        windows[phase] = (min(start, t), max(end, t))

    # Seconds when one side was out of the call on purpose (from "leave i" to "joined i"):
    # nothing can be heard then.
    away = set()
    left = {}
    for t, phase, event in marks:
        if event.startswith("leave"):
            left[(phase, event.split()[-1])] = t
        elif event.startswith("joined") and (phase, event.split()[-1]) in left:
            away.update(range(left[(phase, event.split()[-1])] // 1000, t // 1000 + 1))

    ok = True
    for src, dst in (("sender", "receiver"), ("receiver", "sender")):
        speech = seconds(side[src], "tx", lambda v: v == 1)
        played = seconds(side[dst], "play")
        decoded = seconds(side[dst], "dec")
        fails = seconds(side[dst], "dave_fail")
        print(f"\n== {src} -> {dst}")
        print(f"{'phase':<14}{'speech s':>9}{'heard':>8}{'longest silence':>17}{'decoded s':>10}{'dave fails':>11}")
        for phase, (start, end) in windows.items():
            secs = [s for s in range(start // 1000, end // 1000 + 1) if speech.get(s, 0) >= 10 and s not in away]
            heard = [s for s in secs if played.get(s, 0) or played.get(s + 1, 0)]
            longest = run_len = 0
            for s in secs:
                run_len = 0 if (played.get(s, 0) or played.get(s + 1, 0)) else run_len + 1
                longest = max(longest, run_len)
            dec = sum(1 for s in secs if decoded.get(s, 0))
            nf = sum(fails.get(s, 0) for s in range(start // 1000, end // 1000 + 1))
            share = 100.0 * len(heard) / len(secs) if secs else 0.0
            print(f"{phase:<14}{len(secs):>9}{share:>7.1f}%{longest:>15} s{dec:>10}{nf:>11}")
            if phase != "output-break" and secs and (share < 95.0 or longest > 3):
                ok = False
        # Recovery after every join in the call: first second with sound again.
        for t, phase, event in marks:
            if "joined" in event:
                after = [s for s in range(t // 1000, t // 1000 + 30) if played.get(s, 0)]
                took = f"{after[0] - t // 1000} s" if after else "nothing in 30 s"
                print(f"  {phase} {event}: sound again after {took}")
                if not after:
                    ok = False
    print("\nRESULT:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
