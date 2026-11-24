#!/usr/bin/env python3
"""Exports the Beat This! "small0" beat tracker to ONNX for the iOS app.

Beat This! (Foscarin, Schlueter, Widmer, ISMIR 2024) is MIT licensed, code
and weights: https://github.com/CPJKU/beat_this. The app bundles the result
as `ios/Coverwave/Resources/BeatThis-small.onnx` (git-ignored, like every
model file); `crates/cw-analysis/src/rhythm.rs` runs it.

Needs a throwaway environment (Python 3.12; PyTorch does not ship for 3.14
yet):

    python3.12 -m venv /tmp/bt && /tmp/bt/bin/pip install torch torchaudio \
        onnx onnxruntime onnxscript einops soxr rotary-embedding-torch \
        "git+https://github.com/CPJKU/beat_this.git"
    /tmp/bt/bin/python ios/tools/export_beat_model.py ios/Coverwave/Resources/BeatThis-small.onnx

With `--reference DIR` it also writes a test clip and PyTorch's features
and logits for it, which `cargo run -p cw-analysis --example beat_check`
compares against the Rust port.
"""

import argparse
import pathlib

import numpy as np
import torch

from beat_this.inference import load_model
from beat_this.preprocessing import LogMelSpect

CHUNK = 1500  # frames: 30 seconds at 50 fps


class Logits(torch.nn.Module):
    """The model with plain tensor outputs: (beat, downbeat) logits."""

    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, spect):
        out = self.model(spect)
        return out["beat"], out["downbeat"]


def drums(sr=22050, secs=30.0, bpm=126.0, offset=0.37):
    """Kick on 1 and 3, snare-ish noise on 2 and 4, hats on eighths."""
    rng = np.random.default_rng(7)
    n = int(secs * sr)
    out = np.zeros(n, dtype=np.float32)
    beat = 60.0 / bpm
    t = np.arange(int(0.25 * sr)) / sr
    kick = np.sin(2 * np.pi * (50 + 80 * np.exp(-t * 30)) * t) * np.exp(-t * 12)
    snare = rng.standard_normal(t.size) * np.exp(-t * 25) * 0.5
    hat = rng.standard_normal(t.size) * np.exp(-t * 80) * 0.15
    k = 0
    while offset + k * beat / 2 < secs:
        start = int((offset + k * beat / 2) * sr)
        parts = [hat]
        if k % 4 == 0:
            parts.append(kick)
        if k % 4 == 2:
            parts.append(snare)
        for p in parts:
            end = min(n, start + p.size)
            out[start:end] += p[: end - start]
        k += 1
    return (out / np.abs(out).max() * 0.8).astype(np.float32)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out", type=pathlib.Path)
    ap.add_argument("--checkpoint", default="small0")
    ap.add_argument("--reference", type=pathlib.Path)
    args = ap.parse_args()

    model = load_model(args.checkpoint, device="cpu").eval()
    wrapped = Logits(model).eval()
    dummy = torch.zeros(1, CHUNK, 128)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(
        wrapped,
        (dummy,),
        str(args.out),
        input_names=["spect"],
        output_names=["beat", "downbeat"],
        opset_version=17,
        dynamo=False,
    )

    # Check the export against PyTorch on a real input.
    import onnxruntime as ort

    audio = drums()
    spect = LogMelSpect()(torch.from_numpy(audio))
    x = torch.zeros(1, CHUNK, 128)
    used = min(CHUNK, spect.shape[0])
    x[0, :used] = spect[:used]
    with torch.no_grad():
        beat, down = wrapped(x)
    sess = ort.InferenceSession(str(args.out))
    ob, od = sess.run(None, {"spect": x.numpy()})
    diff = max(np.abs(ob - beat.numpy()).max(), np.abs(od - down.numpy()).max())
    print(f"wrote {args.out} ({args.out.stat().st_size / 1e6:.1f} MB); max |onnx - torch| = {diff:.2e}")
    assert diff < 1e-3, "ONNX export does not match PyTorch"

    if args.reference:
        r = args.reference
        r.mkdir(parents=True, exist_ok=True)
        audio.tofile(r / "clip_22050.f32")
        spect.numpy().astype(np.float32).tofile(r / "logmel.f32")
        beat.numpy().astype(np.float32).tofile(r / "beat.f32")
        down.numpy().astype(np.float32).tofile(r / "downbeat.f32")
        print(f"reference in {r}: {spect.shape[0]} frames")


if __name__ == "__main__":
    main()
