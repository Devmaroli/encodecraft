#!/usr/bin/env bash
# Cross-compile EncodeCraft for 64-bit Windows and pack a double-click zip.
#
# Usage (from the repo root, on Linux):
#   ./scripts/build-windows.sh
#
# Prefers the MSVC target via cargo-xwin with a static CRT (no VC++ redistributable
# or MinGW DLLs). If that is not available, falls back to x86_64-pc-windows-gnu
# (MinGW) and copies runtime DLLs.
#
# Environment:
#   FFMPEG_URL   Windows FFmpeg zip (default: Gyan essentials release)
#   SKIP_FFMPEG=1   pack the apps only (not useful for end users)
#   DIST_DIR     output folder (default: dist/encodecraft-windows-x64)

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
DIST_DIR="${DIST_DIR:-"$ROOT/dist/encodecraft-windows-x64"}"
ZIP_NAME="encodecraft-${VERSION}-windows-x64.zip"
FFMPEG_URL="${FFMPEG_URL:-https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip}"
CACHE="$ROOT/dist/cache"
mkdir -p "$CACHE"

log() { printf '==> %s\n' "$*"; }

have() { command -v "$1" >/dev/null 2>&1; }

ensure_xwin() {
  rustup target add x86_64-pc-windows-msvc
  if ! have cargo-xwin; then
    log "installing cargo-xwin"
    cargo install cargo-xwin --locked
  fi
}

build_msvc() {
  log "building Windows MSVC release (cargo xwin)"
  cargo xwin build --release --target x86_64-pc-windows-msvc \
    -p encodecraft -p encodecraft-cli
  TARGET=x86_64-pc-windows-msvc
}

build_gnu() {
  log "building Windows GNU release (MinGW)"
  rustup target add x86_64-pc-windows-gnu
  if ! have x86_64-w64-mingw32-gcc; then
    echo "x86_64-w64-mingw32-gcc is not installed. Install mingw-w64 or cargo-xwin." >&2
    exit 1
  fi
  cargo build --release --target x86_64-pc-windows-gnu \
    -p encodecraft -p encodecraft-cli
  TARGET=x86_64-pc-windows-gnu
}

copy_mingw_dlls() {
  local dest="$1"
  local gcc
  gcc="$(command -v x86_64-w64-mingw32-gcc)"
  local search=(
    "$(dirname "$gcc")"
    "/usr/x86_64-w64-mingw32/lib"
    "/usr/lib/gcc/x86_64-w64-mingw32"
  )
  local names=(libgcc_s_seh-1.dll libstdc++-6.dll libwinpthread-1.dll)
  local n dll
  for n in "${names[@]}"; do
    dll="$(find "${search[@]}" -name "$n" 2>/dev/null | head -1 || true)"
    if [[ -n "$dll" ]]; then
      cp -a "$dll" "$dest/"
      log "bundled $n"
    else
      log "warning: $n not found (MinGW runtime)"
    fi
  done
}

download_ffmpeg() {
  local zip="$CACHE/ffmpeg-windows-essentials.zip"
  if [[ ! -f "$zip" ]]; then
    log "downloading Windows FFmpeg essentials"
    curl -fL --retry 4 --retry-delay 4 -o "$zip.part" "$FFMPEG_URL"
    mv "$zip.part" "$zip"
  else
    log "using cached $zip"
  fi
  local unpack="$CACHE/ffmpeg-win"
  rm -rf "$unpack"
  mkdir -p "$unpack"
  unzip -q -o "$zip" -d "$unpack"
  local ff
  ff="$(find "$unpack" -name 'ffmpeg.exe' | head -1)"
  local fp
  fp="$(find "$unpack" -name 'ffprobe.exe' | head -1)"
  if [[ -z "$ff" || -z "$fp" ]]; then
    echo "ffmpeg.exe / ffprobe.exe missing from $zip" >&2
    exit 1
  fi
  cp -a "$ff" "$DIST_DIR/ffmpeg.exe"
  cp -a "$fp" "$DIST_DIR/ffprobe.exe"
  # License text shipped with the build (GPL for Gyan essentials / BtbN gpl).
  local lic
  lic="$(find "$unpack" \( -iname 'LICENSE' -o -iname 'LICENSE.txt' -o -iname 'COPYING*' -o -iname 'gpl*' \) | head -1 || true)"
  if [[ -n "$lic" ]]; then
    cp -a "$lic" "$DIST_DIR/FFmpeg-LICENSE.txt"
  else
    printf '%s\n' "FFmpeg is licensed under the GNU GPL. See https://ffmpeg.org/legal.html" > "$DIST_DIR/FFmpeg-LICENSE.txt"
  fi
  local readme
  readme="$(find "$unpack" -iname 'README.txt' | head -1 || true)"
  if [[ -n "$readme" ]]; then
    cp -a "$readme" "$DIST_DIR/FFmpeg-README.txt"
  fi
  if [[ -f "$ROOT/packaging/windows/FFmpeg-NOTICE.txt" ]]; then
    cp -a "$ROOT/packaging/windows/FFmpeg-NOTICE.txt" "$DIST_DIR/FFmpeg-NOTICE.txt"
  fi
  log "bundled $(basename "$ff") and $(basename "$fp")"
}

# --- build ---
TARGET=""
if ensure_xwin && build_msvc; then
  :
else
  log "MSVC/xwin build failed; trying MinGW GNU target"
  build_gnu
fi

BIN="$ROOT/target/$TARGET/release"
if [[ ! -f "$BIN/encodecraft.exe" || ! -f "$BIN/encodecraft-cli.exe" ]]; then
  echo "expected exes in $BIN" >&2
  exit 1
fi

log "staging $DIST_DIR"
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"
cp -a "$BIN/encodecraft.exe" "$DIST_DIR/encodecraft.exe"
cp -a "$BIN/encodecraft-cli.exe" "$DIST_DIR/encodecraft-cli.exe"
cp -a "$ROOT/packaging/windows/README-Windows.txt" "$DIST_DIR/README-Windows.txt"
cp -a "$ROOT/packaging/windows/FFmpeg-NOTICE.txt" "$DIST_DIR/FFmpeg-NOTICE.txt"
cp -a "$ROOT/LICENSE-MIT" "$DIST_DIR/LICENSE-MIT.txt"
cp -a "$ROOT/LICENSE-APACHE" "$DIST_DIR/LICENSE-APACHE.txt"
cp -a "$ROOT/NOTICE" "$DIST_DIR/NOTICE.txt"

if [[ "$TARGET" == *gnu ]]; then
  copy_mingw_dlls "$DIST_DIR"
fi

if [[ "${SKIP_FFMPEG:-}" != "1" ]]; then
  download_ffmpeg
fi

# Tiny helper so people do not have to read the long name.
printf '%s\n' 'Double-click encodecraft.exe  (see README-Windows.txt)' > "$DIST_DIR/START.txt"

log "zipping"
mkdir -p "$ROOT/dist"
(
  cd "$(dirname "$DIST_DIR")"
  rm -f "$ZIP_NAME"
  zip -r -9 "$ZIP_NAME" "$(basename "$DIST_DIR")"
)
ZIP="$ROOT/dist/$ZIP_NAME"
log "wrote $ZIP ($(du -h "$ZIP" | awk '{print $1}'))"
printf '%s\n' "$ZIP"
