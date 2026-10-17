"""Scores a voice_e2e.ps1 run: did the reference speech arrive whole?

Usage: python voice_compare.py <run dir>

The receiver writes decoded voice in arrival order, so silences the sender did not
transmit are missing and absolute timing cannot be compared. Instead the received
audio is cut into 200 ms chunks and each chunk with speech is searched for in the
reference (normalized cross-correlation). A chunk that matches nowhere was damaged
in transit (dropout, garbled frame); the report lists them.
"""

import csv
import sys
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from scipy.signal import fftconvolve, resample_poly

RATE = 48000
CHUNK = RATE // 5          # 200 ms
SPEECH_RMS = 0.01          # chunk counts as speech above this RMS
GOOD_MATCH = 0.6           # normalized correlation that counts as "arrived intact"


def load_reference(path: Path) -> np.ndarray:
    rate, data = wavfile.read(path)
    data = data.astype(np.float32)
    if data.ndim > 1:
        data = data.mean(axis=1)
    data /= max(1.0, np.abs(data).max())
    g = np.gcd(rate, RATE)
    return resample_poly(data, RATE // g, rate // g).astype(np.float32)


def best_match(chunk: np.ndarray, ref: np.ndarray, ref_energy: np.ndarray) -> float:
    num = fftconvolve(ref, chunk[::-1], mode="valid")
    den = np.sqrt(ref_energy * float(np.dot(chunk, chunk)))
    # Only offsets where the reference itself has speech; near-silent windows would
    # divide by ~0 and match anything.
    usable = ref_energy > len(chunk) * (SPEECH_RMS ** 2) * 0.25
    return float(np.max(np.abs(num[usable]) / den[usable])) if usable.any() else 0.0


def load_events(path: Path):
    """Rows of (time in ms, event, value, flag) from a LITECORD_TEST_VOICE_LOG file."""
    if not path.exists():
        return []
    return [
        (int(r["t_us"]) / 1000.0, r["event"], int(r["value"]), r["flag"] == "1")
        for r in csv.DictReader(open(path, encoding="utf-8"))
    ]


def pct(values, q):
    return float(np.percentile(values, q)) if len(values) else 0.0


def summarize(run: Path) -> None:
    sender = load_events(run / "sender.csv")
    receiver = load_events(run / "receiver.csv")

    mic_t = [t for t, e, _, _ in sender if e == "mic"]
    if len(mic_t) > 1:
        d = np.diff(mic_t)
        print(f"sender mic blocks: {len(mic_t)}, interval p50 {pct(d, 50):.1f} ms, p99 {pct(d, 99):.1f} ms, max {d.max():.0f} ms")

    tx = [(t, v == 1, f) for t, e, v, f in sender if e == "tx"]
    # Gaps between consecutive speech packets: inside a talkspurt every 20 ms frame is sent,
    # so anything well above 20 ms is audio that reached the listener late or not at all.
    gaps = [b[0] - a[0] for a, b in zip(tx, tx[1:]) if a[1] and b[1]]
    late = [g for g in gaps if g > 40]
    print(f"sender: {len(tx)} packets, {sum(1 for p in tx if p[1])} with speech, "
          f"{sum(1 for p in tx if p[2])} from the no-mic fallback")
    if gaps:
        print(f"  gap between speech packets: p50 {pct(gaps, 50):.1f} ms, p99 {pct(gaps, 99):.1f} ms, "
              f"max {max(gaps):.0f} ms, {len(late)} above 40 ms")

    rx = [v for _, e, v, _ in receiver if e == "rx"]
    reordered = sum(1 for a, b in zip(rx, rx[1:]) if 0 < (a - b) % 65536 < 1000)
    print(f"receiver: {len(rx)} packets ({len(tx) - len(rx)} fewer than sent), {reordered} out of order")


def main() -> int:
    run = Path(sys.argv[1])
    summarize(run)

    ref = load_reference(run / "reference.wav")
    rx_path = run / "received.f32"
    rx = np.fromfile(rx_path, dtype="<f4") if rx_path.exists() else np.zeros(0, np.float32)
    print(f"reference: {len(ref) / RATE:.1f} s, received audio: {len(rx) / RATE:.1f} s")
    if len(rx) < CHUNK:
        print("RESULT: FAIL (no voice received)")
        return 1

    # Received level differs from the reference (gain, codec); normalize per chunk instead.
    ref_energy = np.clip(fftconvolve(ref * ref, np.ones(CHUNK, np.float32), mode="valid"), 0.0, None)
    scores, bad = [], []
    for start in range(0, len(rx) - CHUNK + 1, CHUNK):
        chunk = rx[start:start + CHUNK]
        if np.sqrt(np.mean(chunk * chunk)) < SPEECH_RMS:
            continue
        score = best_match(chunk, ref, ref_energy)
        scores.append(score)
        if score < GOOD_MATCH:
            bad.append((start / RATE, score))

    if not scores:
        print("RESULT: FAIL (received audio has no speech)")
        return 1
    ref_speech = sum(
        1 for s in range(0, len(ref) - CHUNK + 1, CHUNK)
        if np.sqrt(np.mean(ref[s:s + CHUNK] ** 2)) >= SPEECH_RMS
    )
    print(f"speech chunks: reference {ref_speech}, received {len(scores)}")
    print(f"match score: median {np.median(scores):.2f}, worst {min(scores):.2f}")
    for at, score in bad[:20]:
        print(f"  damaged chunk at {at:6.2f} s of received audio (score {score:.2f})")
    ok = len(bad) <= max(1, len(scores) // 50) and len(scores) >= 0.9 * ref_speech
    print(f"RESULT: {'PASS' if ok else 'FAIL'} ({len(bad)} damaged of {len(scores)} speech chunks)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
