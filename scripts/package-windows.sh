#!/usr/bin/env bash
# Cross-compile (or reuse) EncodeCraft for 64-bit Windows and pack:
#   dist/EncodeCraft-Setup-x64.exe     per-user NSIS installer (no admin)
#   dist/EncodeCraft-Portable-x64.zip  portable folder + portable.txt
#   dist/SHA256SUMS
#
# Usage (repo root):
#   ./scripts/package-windows.sh
#   SKIP_BUILD=1 ./scripts/package-windows.sh   # use already-built .exe
#   SKIP_FFMPEG=1 ./scripts/package-windows.sh  # apps only (not for users)
#
# Linux: cargo-xwin + makensis (apt install nsis).
# Windows (GitHub Actions): native cargo + NSIS from Chocolatey.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
NSIS_VER="$VERSION"
if [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  NSIS_VER="${VERSION}.0"
fi

STAGE="${STAGE_DIR:-"$ROOT/dist/encodecraft-windows-x64"}"
PORTABLE_DIR="${PORTABLE_DIR:-"$ROOT/dist/EncodeCraft-Portable-x64"}"
SETUP_NAME="EncodeCraft-Setup-x64.exe"
PORTABLE_ZIP="EncodeCraft-Portable-x64.zip"
FFMPEG_URL="${FFMPEG_URL:-https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip}"
CACHE="$ROOT/dist/cache"
mkdir -p "$CACHE"

log() { printf '==> %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

run_python() {
  if have python3; then
    python3 "$@"
  elif have python; then
    python "$@"
  else
    echo "python3/python not found" >&2
    exit 1
  fi
}

# GitHub-hosted windows-latest + Git Bash: NSIS wants D:\... not /d/...
native_path() {
  if is_windows && have cygpath; then
    cygpath -w "$1"
  else
    printf '%s' "$1"
  fi
}

is_windows() {
  case "$(uname -s 2>/dev/null || true)" in
    MINGW*|MSYS*|CYGWIN*|Windows_NT) return 0 ;;
  esac
  [[ "${OS:-}" == "Windows_NT" ]]
}

ensure_xwin() {
  rustup target add x86_64-pc-windows-msvc
  if ! have cargo-xwin; then
    log "installing cargo-xwin"
    cargo install cargo-xwin --locked
  fi
}

build_msvc_linux() {
  log "building Windows MSVC release (cargo xwin)"
  cargo xwin build --release --target x86_64-pc-windows-msvc \
    -p encodecraft -p encodecraft-cli
  echo x86_64-pc-windows-msvc
}

build_native_windows() {
  log "building Windows release (native cargo)"
  cargo build --release -p encodecraft -p encodecraft-cli
  echo native
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
  echo x86_64-pc-windows-gnu
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

find_built_exes() {
  local candidates=(
    "$ROOT/target/release"
    "$ROOT/target/x86_64-pc-windows-msvc/release"
    "$ROOT/target/x86_64-pc-windows-gnu/release"
  )
  local d
  for d in "${candidates[@]}"; do
    if [[ -f "$d/encodecraft.exe" && -f "$d/encodecraft-cli.exe" ]]; then
      printf '%s\n' "$d"
      return 0
    fi
  done
  return 1
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
  if have unzip; then
    unzip -q -o "$zip" -d "$unpack"
  else
    run_python - "$zip" "$unpack" <<'PY'
import sys, zipfile
zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])
PY
  fi
  local ff fp
  ff="$(find "$unpack" -name 'ffmpeg.exe' | head -1)"
  fp="$(find "$unpack" -name 'ffprobe.exe' | head -1)"
  if [[ -z "$ff" || -z "$fp" ]]; then
    echo "ffmpeg.exe / ffprobe.exe missing from $zip" >&2
    exit 1
  fi
  cp -a "$ff" "$STAGE/ffmpeg.exe"
  cp -a "$fp" "$STAGE/ffprobe.exe"
  local lic
  lic="$(find "$unpack" \( -iname 'LICENSE' -o -iname 'LICENSE.txt' -o -iname 'COPYING*' -o -iname 'gpl*' \) | head -1 || true)"
  if [[ -n "$lic" ]]; then
    cp -a "$lic" "$STAGE/FFmpeg-LICENSE.txt"
  else
    printf '%s\n' "FFmpeg is licensed under the GNU GPL. See https://ffmpeg.org/legal.html" > "$STAGE/FFmpeg-LICENSE.txt"
  fi
  local readme
  readme="$(find "$unpack" -iname 'README.txt' | head -1 || true)"
  if [[ -n "$readme" ]]; then
    cp -a "$readme" "$STAGE/FFmpeg-README.txt"
  fi
  if [[ -f "$ROOT/packaging/windows/FFmpeg-NOTICE.txt" ]]; then
    cp -a "$ROOT/packaging/windows/FFmpeg-NOTICE.txt" "$STAGE/FFmpeg-NOTICE.txt"
  fi
  log "bundled $(basename "$ff") and $(basename "$fp")"
}

stage_payload() {
  local bin="$1"
  local gnu="${2:-}"
  log "staging $STAGE"
  rm -rf "$STAGE"
  mkdir -p "$STAGE"
  cp -a "$bin/encodecraft.exe" "$STAGE/encodecraft.exe"
  cp -a "$bin/encodecraft-cli.exe" "$STAGE/encodecraft-cli.exe"
  cp -a "$ROOT/packaging/windows/README-Windows.txt" "$STAGE/README-Windows.txt"
  cp -a "$ROOT/packaging/windows/FFmpeg-NOTICE.txt" "$STAGE/FFmpeg-NOTICE.txt"
  cp -a "$ROOT/LICENSE-MIT" "$STAGE/LICENSE-MIT.txt"
  cp -a "$ROOT/LICENSE-APACHE" "$STAGE/LICENSE-APACHE.txt"
  cp -a "$ROOT/NOTICE" "$STAGE/NOTICE.txt"
  printf '%s\n' "Double-click encodecraft.exe  (see README-Windows.txt)" > "$STAGE/START.txt"
  if [[ "$gnu" == *gnu ]]; then
    copy_mingw_dlls "$STAGE"
  fi
  if [[ "${SKIP_FFMPEG:-}" != "1" ]]; then
    download_ffmpeg
  fi
}

find_makensis() {
  if have makensis; then
    command -v makensis
    return
  fi
  local c
  for c in \
    "/c/Program Files (x86)/NSIS/makensis.exe" \
    "/c/Program Files/NSIS/makensis.exe" \
    "/usr/bin/makensis"
  do
    if [[ -x "$c" ]]; then
      printf '%s\n' "$c"
      return
    fi
  done
  return 1
}

build_installer() {
  local nsis
  nsis="$(find_makensis)" || {
    echo "makensis not found. On Linux: sudo apt install nsis. On Windows: choco install nsis." >&2
    exit 1
  }
  local out="$ROOT/dist/$SETUP_NAME"
  rm -f "$out"
  local glob
  if is_windows; then
    # Windows NSIS does not treat "dir/*" as a wildcard. It needs "dir\*.*".
    glob="$(native_path "$STAGE")\\*.*"
  else
    glob="${STAGE}/*"
  fi
  log "NSIS $nsis → $out"
  log "payload $(find "$STAGE" -type f | wc -l | tr -d ' ') files, glob $glob"
  # Debian makensis wants -D; Windows makensis accepts /D.
  # On GitHub windows-latest, convert Git Bash paths so File /r finds the payload.
  "$nsis" \
    -DPRODUCT_VERSION="$VERSION" \
    -DPRODUCT_VERSION_NSIS="$NSIS_VER" \
    "-DOUTFILE=$(native_path "$out")" \
    "-DPAYLOAD=$(native_path "$STAGE")" \
    "-DPAYLOAD_GLOB=${glob}" \
    "-DICO=$(native_path "$ROOT/assets/app-icon/encodecraft.ico")" \
    "$(native_path "$ROOT/packaging/windows/encodecraft.nsi")"
  if [[ ! -f "$out" ]]; then
    echo "NSIS did not write $out" >&2
    exit 1
  fi
}

build_portable() {
  log "portable zip $PORTABLE_ZIP"
  rm -rf "$PORTABLE_DIR"
  mkdir -p "$PORTABLE_DIR"
  cp -a "$STAGE"/. "$PORTABLE_DIR/"
  cp -a "$ROOT/packaging/windows/portable.txt" "$PORTABLE_DIR/portable.txt"
  mkdir -p "$ROOT/dist"
  (
    cd "$ROOT/dist"
    rm -f "$PORTABLE_ZIP"
    if have zip; then
      zip -r -9 "$PORTABLE_ZIP" "$(basename "$PORTABLE_DIR")"
    else
      run_python - "$PORTABLE_ZIP" "$(basename "$PORTABLE_DIR")" <<'PY'
import sys, zipfile, pathlib
out, folder = sys.argv[1], pathlib.Path(sys.argv[2])
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    for p in folder.rglob("*"):
        if p.is_file():
            z.write(p, p.as_posix())
PY
    fi
  )
}

write_checksums() {
  local setup="$ROOT/dist/$SETUP_NAME"
  local zipf="$ROOT/dist/$PORTABLE_ZIP"
  local sums="$ROOT/dist/SHA256SUMS"
  if have sha256sum; then
    (cd "$ROOT/dist" && sha256sum "$SETUP_NAME" "$PORTABLE_ZIP" > SHA256SUMS)
  else
    run_python - "$setup" "$zipf" "$sums" <<'PY'
import hashlib, pathlib, sys
lines = []
for p in sys.argv[1:3]:
    data = pathlib.Path(p).read_bytes()
    lines.append(f"{hashlib.sha256(data).hexdigest()}  {pathlib.Path(p).name}")
pathlib.Path(sys.argv[3]).write_text("\n".join(lines) + "\n")
PY
  fi
  log "checksums"
  cat "$sums"
}

# --- build ---
TARGET_KIND=""
if [[ "${SKIP_BUILD:-}" == "1" ]]; then
  log "SKIP_BUILD=1 — using existing binaries"
else
  if is_windows; then
    TARGET_KIND="$(build_native_windows)"
  elif ensure_xwin && TARGET_KIND="$(build_msvc_linux)"; then
    :
  else
    log "MSVC/xwin build failed; trying MinGW GNU target"
    TARGET_KIND="$(build_gnu)"
  fi
fi

BIN="$(find_built_exes)" || {
  echo "expected encodecraft.exe and encodecraft-cli.exe under target/" >&2
  exit 1
}
log "binaries in $BIN"

stage_payload "$BIN" "$TARGET_KIND"
mkdir -p "$ROOT/dist"
build_installer
build_portable
write_checksums

log "wrote $ROOT/dist/$SETUP_NAME ($(du -h "$ROOT/dist/$SETUP_NAME" | awk '{print $1}'))"
log "wrote $ROOT/dist/$PORTABLE_ZIP ($(du -h "$ROOT/dist/$PORTABLE_ZIP" | awk '{print $1}'))"
printf '%s\n' "$ROOT/dist/$SETUP_NAME" "$ROOT/dist/$PORTABLE_ZIP"
