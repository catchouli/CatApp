"""CatApp's voice helper: Kokoro text-to-speech and Parakeet speech-to-text, kept loaded.

CatApp starts this on demand (`<voice dir>/venv/bin/python voice_server.py <voice dir>`)
and stops it when voice mode has been idle for a while. One JSON request per stdin line,
one JSON reply per stdout line, in order:

  {"op": "tts", "text": "...", "voice": "patchouli"}  -> {"wav": "<base64 24kHz mono WAV>"}
  {"op": "stt", "path": "/tmp/clip.webm"}              -> {"text": "..."}
  {"op": "ping"}                                       -> {"ok": true}

Failures reply {"error": "..."}. Logs go to stderr.
"""
import base64
import io
import json
import re
import sys
import time
import wave
from pathlib import Path

import numpy as np

# Named voices: a weighted blend of Kokoro style vectors plus a default speed. A voice can
# also be a plain Kokoro voice ("af_heart") or an even blend ("af_alloy,af_nicole"), and
# ":<speed>" on the end sets the speed ("patchouli:1.2"; 0.5 to 2, artifacts creep in
# above about 1.15).
BLENDS = {
    # Patchouli's voice, chosen by Cat in a three-round bake-off (2026-09).
    "patchouli": {"blend": {"af_alloy": 0.5, "af_nicole": 0.5}, "speed": 1.1},
    "patchouli-fast": {"blend": {"af_alloy": 0.5, "af_nicole": 0.5}, "speed": 1.15},
}

STT_MODEL = "mlx-community/parakeet-tdt-0.6b-v2"


def log(*a):
    print(*a, file=sys.stderr, flush=True)


# Old-school emoticons and markdown read badly aloud: ^_^, >_<, :), :3, **bold**, `code`.
EMOTICON = re.compile(r"(?<!\w)(?:[\^>;:=xX][_\-.~oO']?[\^<)(DPpOo3\]\[|/\\]+|<3|[\^]{2})(?!\w)")


def speakable(text: str) -> str:
    t = re.sub(r"[*_`#>]+", " ", EMOTICON.sub(" ", text))
    t = re.sub(r"https?://\S+", "a link", t)
    return re.sub(r"\s+", " ", t).strip()


class Voice:
    def __init__(self, home: Path):
        from kokoro_onnx import Kokoro

        t0 = time.time()
        self.kokoro = Kokoro(str(home / "kokoro-v1.0.onnx"), str(home / "voices-v1.0.bin"))
        log(f"kokoro loaded in {time.time() - t0:.1f}s")
        self.stt = None
        self.styles = {}

    def style(self, voice: str):
        """(style, speed) for a voice name."""
        if voice not in self.styles:
            name, _, speed = voice.partition(":")
            recipe = BLENDS.get(name)
            if recipe is None:
                names = [v.strip() for v in name.split(",") if v.strip()] or ["af_heart"]
                recipe = {"blend": {n: 1.0 for n in names}, "speed": 1.0}
            total = sum(recipe["blend"].values()) or 1.0
            style = sum(self.kokoro.get_voice_style(n) * (w / total) for n, w in recipe["blend"].items())
            speed = min(2.0, max(0.5, float(speed))) if speed else recipe["speed"]
            self.styles[voice] = (style, speed)
        return self.styles[voice]

    def tts(self, text: str, voice: str) -> dict:
        text = speakable(text)
        if not text:
            return {"wav": ""}
        style, speed = self.style(voice or "af_heart")
        audio, sr = self.kokoro.create(text, voice=style, speed=speed, lang="en-us")
        pcm = (np.clip(audio, -1.0, 1.0) * 32767).astype(np.int16)
        buf = io.BytesIO()
        with wave.open(buf, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(sr)
            w.writeframes(pcm.tobytes())
        return {"wav": base64.b64encode(buf.getvalue()).decode()}

    def load_stt(self):
        if self.stt is None:
            from parakeet_mlx import from_pretrained

            t0 = time.time()
            self.stt = from_pretrained(STT_MODEL)
            log(f"parakeet loaded in {time.time() - t0:.1f}s")
        return self.stt

    def transcribe(self, path: str) -> dict:
        return {"text": self.load_stt().transcribe(path).text.strip()}


def main() -> None:
    home = Path(sys.argv[1])
    v = Voice(home)
    v.load_stt()
    v.tts("Hi.", "patchouli")  # first render is slower; get it out of the way
    print(json.dumps({"ready": True}), flush=True)
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            req = json.loads(line)
            op = req.get("op")
            t0 = time.time()
            if op == "tts":
                out = v.tts(req.get("text", ""), req.get("voice", ""))
            elif op == "stt":
                out = v.transcribe(req["path"])
            elif op == "ping":
                out = {"ok": True}
            else:
                out = {"error": f"unknown op {op!r}"}
            log(f"{op} in {time.time() - t0:.2f}s")
        except Exception as e:  # keep serving; report the failure to CatApp
            out = {"error": f"{type(e).__name__}: {e}"[:300]}
        print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
