# EncodeCraft

A desktop **encode queue** in the spirit of Adobe Media Encoder, written in Rust as a companion to [EffectCraft](https://github.com/storytold/effectcraft) (the open-source After Effects clone). Same family of choices: Cargo workspace, engine crates under the UI, native **egui / eframe**, MIT OR Apache-2.0, no Adobe assets.

EncodeCraft is an independent project. It is not affiliated with Adobe or with the ArtCraft / Crafting Apps team.

```
crates/job          shared enqueue JSON (EffectCraft can depend on this crate only)
crates/presets      H.264 / HEVC / ProRes / WebM / GIF / image sequence / audio + DOOH sizes
crates/ffmpeg       discover ffmpeg/ffprobe, probe, argv, progress, encode
crates/queue        jobs, reorder, duplicate, one-source-many-sizes
crates/engine       persistence, watch folders, HTTP/JSON-lines IPC, workers
crates/ui-egui      Media Encoder-style shell
apps/encodecraft        desktop app
apps/encodecraft-cli    encode / enqueue / serve
```

EffectCraft encodes in **pure Rust** (no FFmpeg). EncodeCraft **shells out** to the FFmpeg binary on purpose: a delivery queue needs every codec FFmpeg already has, including hardware you trust. FFmpeg is never linked (GPL-safe). If it is missing, the UI shows install instructions instead of crashing.

## Features (v0.1)

- Encode **queue**: add (picker + drag-and-drop), reorder, remove, duplicate, start / pause / stop, parallel jobs with a configurable limit
- **Presets**: H.264 MP4, H.265/HEVC MP4, ProRes MOV, WebM VP9, animated GIF, PNG/JPEG sequences, AAC / WAV / MP3 — create, edit, import, export
- Per-job **container, codec, size, fps, CRF or bitrate, audio, trim**
- **DOOH screen sizes** and **one source → many sizes** (1920×1080, 1080×1920, 1200×960, 1080×864, plus a built-in pack)
- Output **naming** (`{name}_{width}x{height}`) and output folder
- **Watch folders**
- Per-job percent / ETA / speed, log, source thumbnail
- Queue, presets and settings **persist** between runs
- **EffectCraft handoff**: `POST /v1/enqueue` on `127.0.0.1:9878`, CLI `enqueue`, inbox drop if the app is not running — see [docs/effectcraft-integration.md](docs/effectcraft-integration.md)

## Requirements

- Rust 1.85+ (`rustup`)
- FFmpeg **and** ffprobe on `PATH`, or `FFMPEG_PATH`
- Linux: a working display for the GUI (X11 or Wayland). The CLI encodes without a window.

### FFmpeg on Windows

1. **winget** (recommended): `winget install Gyan.FFmpeg`
2. Or download a full build from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/), unzip, and add the `bin` folder to PATH.
3. Or set `FFMPEG_PATH` to `ffmpeg.exe` (EncodeCraft also looks in `C:\ffmpeg\bin` and Program Files).

macOS: `brew install ffmpeg`  
Linux: `sudo apt install ffmpeg` (Debian/Ubuntu) or `sudo dnf install ffmpeg`

Confirm:

```sh
ffmpeg -version
ffprobe -version
cargo run -p encodecraft-cli -- ffmpeg
```

## Run

```sh
git clone <this-repo>
cd encodecraft
cargo run --release -p encodecraft          # desktop queue
cargo run --release -p encodecraft-cli -- --help
```

Headless (CI, or when EffectCraft sends jobs with no GUI):

```sh
cargo run --release -p encodecraft-cli -- serve --port 9878 --start
```

Encode a file without the UI:

```sh
cargo run --release -p encodecraft-cli -- encode ./clip.mp4 --preset h264 --out ./out.mp4
cargo run --release -p encodecraft-cli -- encode ./clip.mp4 --preset h264 \
  --sizes 1920x1080,1080x1920,1200x960,1080x864 --out ./exports
```

Send to a running app (launches it if needed):

```sh
cargo run -p encodecraft-cli -- enqueue ./clip.mp4 --preset system.h264-mp4 --start
cargo run -p encodecraft-cli -- enqueue --project ./spot.ecproj --comp Main --preset hevc --start
```

Tests:

```sh
cargo test --workspace
```

Data dir (queue / presets / inbox): `%APPDATA%\EncodeCraft\EncodeCraft` on Windows, `~/Library/Application Support/dev.EncodeCraft.EncodeCraft` on macOS, `~/.local/share/encodecraft` on Linux. Override with `ENCODECRAFT_HOME`.

## EffectCraft

[docs/job-format.md](docs/job-format.md) — JSON schema  
[docs/effectcraft-integration.md](docs/effectcraft-integration.md) — Send to Encoder Queue  
[patches/](patches/) — drop-in command + applyable patch

## License

MIT OR Apache-2.0. See `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`.

Adobe, After Effects and Adobe Media Encoder are trademarks of Adobe Inc.
