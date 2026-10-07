#!/usr/bin/env bash
# Smoke-test the Windows zip under Wine (64-bit). Optional; the zip is for real Windows.
#
# Usage (from repo root, after ./scripts/build-windows.sh):
#   ./scripts/smoke-windows-wine.sh
#
# Needs wine64 (Debian/Ubuntu: sudo apt install wine64).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="${DIST_DIR:-"$ROOT/dist/encodecraft-windows-x64"}"
WINEPREFIX="${WINEPREFIX:-/tmp/encodecraft-wine}"
export WINEPREFIX WINEARCH=win64 WINEDEBUG=-all
export WINEDLLOVERRIDES="mscoree,mshtml="

if [[ ! -x "$DIST/encodecraft-cli.exe" ]]; then
  echo "missing $DIST/encodecraft-cli.exe — run ./scripts/build-windows.sh first" >&2
  exit 1
fi

WINE=""
for c in /usr/lib/wine/wine64 wine64 wine; do
  if command -v "$c" >/dev/null 2>&1 || [[ -x "$c" ]]; then
    WINE="$c"
    break
  fi
done
if [[ -z "$WINE" ]]; then
  echo "wine64 not found; skip Windows-under-Linux smoke test" >&2
  exit 2
fi

log() { printf '==> %s\n' "$*"; }

log "wine: $($WINE --version 2>/dev/null | tail -1)"
mkdir -p "$WINEPREFIX"
# First boot can take a minute; ignore gecko/mono prompts.
"$WINE" wineboot --init >/tmp/encodecraft-wineboot.log 2>&1 || true

WORKDIR="$DIST"
cd "$WORKDIR"

log "CLI --help"
"$WINE" ./encodecraft-cli.exe --help | head -20

log "CLI ffmpeg (sidecar next to exe)"
"$WINE" ./encodecraft-cli.exe ffmpeg

CLIP="$WORKDIR/smoke-clip.mp4"
if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "host ffmpeg missing; cannot generate a test clip" >&2
  exit 1
fi
log "host ffmpeg → $CLIP"
ffmpeg -y -hide_banner -loglevel error \
  -f lavfi -i testsrc=duration=2:size=320x240:rate=24 \
  -f lavfi -i sine=frequency=440:duration=2 \
  -c:v libx264 -pix_fmt yuv420p -shortest "$CLIP"

OUT="$WORKDIR/smoke-out.mp4"
rm -f "$OUT"
log "CLI encode (Windows ffmpeg under Wine)"
export ENCODECRAFT_HOME="Z:$(mktemp -d /tmp/encodecraft-home.XXXXXX)"
"$WINE" ./encodecraft-cli.exe encode smoke-clip.mp4 --preset h264 --out smoke-out.mp4
ls -lh "$OUT"
ffprobe -hide_banner -loglevel error -show_entries format=duration,format_name -of default=nw=1 "$OUT"
log "wine smoke test ok"
