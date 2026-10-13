#!/bin/sh
# Sets up voice mode for the assistant on this Mac: a Python environment with Kokoro
# (text-to-speech) and Parakeet (speech-to-text), and Kokoro's model files. Everything goes
# in the app's data folder. Run from anywhere:
#
#   sh voice/setup.sh            # voice only
#   sh voice/setup.sh --avatar   # also a stand-in Live2D avatar (Live2D's free Hiyori sample)
#
# Then: catapp config voice on   (and optionally: catapp config avatar on)
set -eu

DATA="${CATAPP_HOME:-$HOME/Library/Application Support/catapp}"
VOICE="$DATA/voice"
mkdir -p "$VOICE"

# Kokoro and MLX need Python 3.10 to 3.13 (newer releases often lack the packages for a while).
PY=""
for v in 3.12 3.13 3.11 3.10; do
  for p in "/opt/homebrew/opt/python@$v/bin/python$v" "/usr/local/opt/python@$v/bin/python$v" "$(command -v "python$v" || true)"; do
    if [ -n "$p" ] && [ -x "$p" ]; then PY="$p"; break 2; fi
  done
done
if [ -z "$PY" ]; then
  echo "Voice mode needs Python 3.10 to 3.13. Install one with: brew install python@3.12" >&2
  exit 1
fi
command -v ffmpeg >/dev/null || { echo "Voice mode needs ffmpeg: brew install ffmpeg" >&2; exit 1; }

echo "Python environment ($PY)..."
[ -x "$VOICE/venv/bin/python" ] || "$PY" -m venv "$VOICE/venv"
"$VOICE/venv/bin/pip" install -q --upgrade pip
"$VOICE/venv/bin/pip" install -q kokoro-onnx parakeet-mlx

URL=https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0
for f in kokoro-v1.0.onnx voices-v1.0.bin; do
  if [ ! -e "$VOICE/$f" ]; then
    echo "Downloading $f..."
    curl -fL --progress-bar -o "$VOICE/$f.part" "$URL/$f" && mv "$VOICE/$f.part" "$VOICE/$f"
  fi
done

echo "Loading the speech-to-text model (downloads ~2.5 GB the first time)..."
"$VOICE/venv/bin/python" -c "from parakeet_mlx import from_pretrained; from_pretrained('mlx-community/parakeet-tdt-0.6b-v2')"

if [ "${1:-}" = "--avatar" ]; then
  AV="$DATA/avatar"
  mkdir -p "$AV/Hiyori"
  echo "Downloading the Live2D runtime and the Hiyori sample model (Live2D's free licences, personal use)..."
  curl -fsSL -o "$AV/live2dcubismcore.min.js" https://cubism.live2d.com/sdk-web/cubismcore/live2dcubismcore.min.js
  SAMPLES=https://raw.githubusercontent.com/Live2D/CubismWebSamples/develop/Samples/Resources
  curl -fsSL "https://api.github.com/repos/Live2D/CubismWebSamples/git/trees/develop?recursive=1" \
    | grep -oE '"path": *"Samples/Resources/Hiyori/[^"]*\.(json|moc3|png)"' | sed 's/.*"Samples\/Resources\///; s/"$//' \
    | while read -r f; do
        mkdir -p "$AV/$(dirname "$f")"
        curl -fsSL -o "$AV/$f" "$SAMPLES/$f"
      done
fi

echo "Done. Turn it on with: catapp config voice on"
