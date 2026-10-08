# EncodeCraft

A desktop **encode queue** in the spirit of Adobe Media Encoder, written in Rust as a companion to [EffectCraft](https://github.com/storytold/effectcraft) (the open-source After Effects clone). Same family of choices: Cargo workspace, engine crates under the UI, native **egui / eframe**, MIT OR Apache-2.0, no Adobe assets. Current release: **v0.2.0**.

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

## Features (v0.2)

- Encode **queue**: add (picker + drag-and-drop), drag rows to change render order (ghost + insertion line, multi-select, source groups stay together, encoding/done rows locked), Up/Down or Alt+↑↓, Undo (Ctrl+Z), start / pause / stop, **Render N at once**
- **Presets**: H.264 MP4, studio **Better Res** (7/8 Mbps), **Installation** (5/6 Mbps), **Approval Res** (1/1.2 Mbps with AAC), **Sultan** (2/3 Mbps, no audio); one-off presets are not remembered as a manual choice. Also H.265/HEVC MP4, ProRes MOV, WebM VP9, animated GIF, PNG/JPEG sequences, AAC / WAV / MP3 — create, edit, import, export
- Per-job **container, codec, size, fps, CRF / bitrate / 2-pass VBR, audio, trim**, plus per-item preset override
- **Preset selection** like Media Encoder: blue Format/Preset dropdowns per output, Auto/Manual/Default tags, pinned Studio group, drag from the Preset Browser onto multi-selected rows (Undo), and a form-based Auto-assign rules editor
- **DOOH / EffectCraft screen sizes**: automatic Better Res vs Installation from screen name/size (ultra-wide, listed screens including Eye of Kuwait, Al Salam Sync, Piccadilly; editable in the rules window). EffectCraft can still send several sizes in one enqueue.
- Tall canvases (any side > 4096, e.g. 1008×5184) use H.264 **level 6.2**; frames H.264 cannot encode are flagged on the queue item
- Output **naming** (`{name}_{width}x{height}`). Default destination is an `Output` folder next to the EffectCraft project or dropped file (Settings can use a fixed folder instead)
- **Watch folders**
- Per-job percent / ETA / speed, log, source thumbnail. The UI thread never waits on FFmpeg; lists are virtualized for 500+ outputs.
- Queue, presets and settings **persist** between runs
- **EffectCraft handoff**: `POST /v1/enqueue` on `127.0.0.1:9878`, CLI `enqueue`, inbox drop if the app is not running — see [docs/effectcraft-integration.md](docs/effectcraft-integration.md)

## Requirements

- Rust 1.85+ (`rustup`)
- FFmpeg **and** ffprobe on `PATH`, or `FFMPEG_PATH`
- Linux: a working display for the GUI (X11 or Wayland). The CLI encodes without a window.

### Windows (no Rust)

Office-style installs, no administrator rights:

- **`EncodeCraft-Setup-x64.exe`** — per-user installer to `%LOCALAPPDATA%\Programs\Craft\EncodeCraft`, Start menu + desktop shortcuts, Frame E Cobalt icon, uninstaller, bundled FFmpeg/ffprobe. Settings stay in `%APPDATA%\EncodeCraft\EncodeCraft\data`.
- **`EncodeCraft-Portable-x64.zip`** — unzip anywhere and run `encodecraft.exe`. A `portable.txt` marker next to the exe stores queue/presets/settings in a `data` folder beside it (not `%APPDATA%`).

Both include `ffmpeg.exe` / `ffprobe.exe`. SmartScreen: More info → Run anyway (the download is not code-signed).

To rebuild from Linux: `./scripts/package-windows.sh` (needs `nsis` / `makensis`) or `cargo xtask windows`. Optional Wine check: `./scripts/smoke-windows-wine.sh`.

A version tag (`v0.2.0`) on the GitHub mirror runs [`.github/workflows/release.yml`](.github/workflows/release.yml) and attaches both files plus `SHA256SUMS`.

The Windows `.exe` and window icon is **Frame E in Electric Cobalt** (`assets/app-icon/encodecraft.ico` and `encodecraft-256.png`). Source pack: [`assets/app-icon/frame-e-electric-cobalt/`](assets/app-icon/frame-e-electric-cobalt/).

### FFmpeg on Windows (if you are compiling yourself)

EncodeCraft looks for `ffmpeg.exe` **next to its own .exe first**, then `FFMPEG_PATH`, then PATH.

1. **winget**: `winget install Gyan.FFmpeg`
2. Or a build from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/)
3. Or set `FFMPEG_PATH` to `ffmpeg.exe`

macOS: `brew install ffmpeg`  
Linux: `sudo apt install ffmpeg` (Debian/Ubuntu) or `sudo dnf install ffmpeg`

Confirm:

```sh
ffmpeg -version
ffprobe -version
cargo run -p encodecraft-cli -- ffmpeg
```

## Performance

EncodeCraft keeps the UI thread free of FFmpeg, ffprobe, watch-folder scans, preset/queue disk writes, and IPC. Queue edits apply on the same frame; size/duration fill in afterwards with a per-row spinner. The Queue, Preset Browser, and Log only draw visible rows.

- Progress while encoding repaints about 8 times a second. Idle (nothing encoding, no toast) does not keep the window spinning. Queue/preset/settings JSON is written on a worker thread, debounced to a couple of times a second.
- `ENCODECRAFT_PERF=1` shows a frame-time overlay and logs frames over 16 ms and slow I/O.
- `ENCODECRAFT_BENCH=1` seeds 1,000 outputs, times ~90 frames (after a short warmup), prints `avg` / `p95` / `max`, and exits. `ENCODECRAFT_BENCH=encode2` does the same with two rows marked encoding. Optional: `ENCODECRAFT_BENCH_ROWS`, `ENCODECRAFT_BENCH_FRAMES`.

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

Data dir (queue / presets / inbox / `ipc-token`): `%APPDATA%\EncodeCraft\EncodeCraft\data` on Windows, `~/Library/Application Support/dev.EncodeCraft.EncodeCraft` on macOS, `~/.local/share/encodecraft` on Linux. A `portable.txt` next to the executable switches that to `<exe folder>/data`. Override with `ENCODECRAFT_HOME`.

## EffectCraft

[docs/job-format.md](docs/job-format.md) — JSON schema  
[docs/effectcraft-integration.md](docs/effectcraft-integration.md) — Send to Encoder Queue  
[patches/](patches/) — drop-in command + applyable patch

## Security

The encode queue listens on `127.0.0.1:9878` with a per-install token. See
[docs/SECURITY-REVIEW.md](docs/SECURITY-REVIEW.md) for the threat model, what we hardened,
and what is still out of scope before using this on client work.

## License

MIT OR Apache-2.0. See `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`.

Adobe, After Effects and Adobe Media Encoder are trademarks of Adobe Inc.
