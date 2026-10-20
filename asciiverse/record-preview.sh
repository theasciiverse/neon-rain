#!/bin/sh
# Records the catalog preview at 100x30 (hub/RECORDING.md), ended by
# SIGTERM, then caps it at 20 fps: every frame shimmers, so the raw cast is
# large. Seed 4 has its first lightning flash at about 7 s. Needs asciinema 3
# and uv (for pyte).
#
#   asciiverse/record-preview.sh target/release/neon-rain asciiverse/neon-rain.cast [seconds]
set -eu
binary=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
out=$(pwd)/$2
seconds=${3:-25}
here=$(cd "$(dirname "$0")" && pwd)
home=$(mktemp -d)
trap 'rm -rf "$home"' EXIT
mkdir "$home/bin" && cp "$binary" "$home/bin/neon-rain"
cd "$home"  # nothing in the recording names a local path
HOME="$home" PATH="$home/bin:$PATH" TERM=xterm-256color COLORTERM=truecolor LANG=C.UTF-8 \
asciinema rec --headless --overwrite -q \
  --window-size 100x30 --idle-time-limit 2 \
  --capture-env "TERM,COLORTERM,LANG" --title "Neon Rain" \
  --command "timeout --foreground -s TERM $seconds neon-rain -s 4" \
  "$home/raw.cast"
uv run -q --with pyte python -I "$here/cast-cap-fps.py" "$home/raw.cast" "$out" 20
