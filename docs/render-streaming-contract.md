# EncodeCraft ↔ EffectCraft render streaming

Contract version **1**. Canonical copy in the EffectCraft repo: `docs/render-streaming.md` on
branch `cursor/render-stream-serve-5c0e` (PR #11). EncodeCraft builds against this file.

The encoded-file fallback `effectcraft-cli render --format prores --out FILE` is unchanged.

On Windows, spawn `effectcraft-cli` with **`CREATE_NO_WINDOW` (`0x08000000`)** and redirected
stdio. Do **not** use `DETACHED_PROCESS` if you need stdout/stderr pipes.

JSON events are one object per line, UTF-8, flushed. On the CLI they go to **stderr**; raw frames
go to **stdout** or the path in `--out`. Never mix frames and JSON on the same stream.

## Pixel formats

| `pixFmt` | Layout | Bytes / frame | FFmpeg |
|---|---|---|---|
| `yuv420p` (default) | planar BT.709 limited-range 8-bit 4:2:0, Y then U then V | `w*h + 2*ceil(w/2)*ceil(h/2)` | `-f rawvideo -pix_fmt yuv420p` |
| `rgb24` | packed RGB | `w*h*3` | `-f rawvideo -pix_fmt rgb24` |
| `rgba` | packed RGBA, straight alpha | `w*h*4` | `-f rawvideo -pix_fmt rgba` |

`yuv420p` output size is cropped to even width and height (minimum 2×2). `w` and `h` in the header
are the bytes you will receive. There is **no** length prefix; each frame is exactly `bytesPerFrame`
bytes. `fps` is a JSON number (25, 23.976, …).

`--out -` or `--out stdout` writes frames to stdout. `--out PATH` writes a file, a POSIX FIFO, or
an existing Windows named pipe (`OpenOptions` write, no truncate on non-files). Flush after every
frame.

## CLI: streaming render

```text
effectcraft-cli render --project F.ecproj [--comp NAME] --format yuv420p|rgb24|rgba|raw \
  --out -|PATH [--pix-fmt yuv420p|rgb24|rgba] [--sidecar header.json] \
  [--start S] [--end S] [--work-area] [--fps N] [--resolution full|half|third|quarter|K] \
  [--quality best|draft] [--gpu] [--audio-out FILE.wav] [--audio-only]
```

- `raw` is an alias of `yuv420p`.
- `--sidecar FILE` writes the header JSON object (same as the `header` event) before frames start.
- `--audio-out FILE` writes a complete WAV (PCM 16-bit little-endian stereo 48 kHz unless the
  output module says otherwise) for muxing (Approval Res). `--audio-out -` writes WAV to stdout;
  that cannot be combined with video on stdout.
- `--audio-only` with `--out FILE|-` writes only the WAV (no video).
- `--gpu` uses the GPU compositor when an adapter exists **and** the frame fits a 6 GiB working
  set (8 GB RTX 3070 Ti at 6880×1032 is in budget). Otherwise the CPU, with a `gpu` event.
  `--gpu` never fails the job for a missing adapter.
- Progress: `{"event":"frame","n":1,"total":250,"elapsed":0.04}` on stderr from the first written
  frame (`n` is 1-based).
- `--no-window` is accepted and ignored; the **parent** must set `CREATE_NO_WINDOW`.

### Encoded fallback (unchanged)

```text
effectcraft-cli render --project F.ecproj --comp NAME --format prores --prores hq --out FILE.mov
```

This still writes a movie file. It now also emits `header` and `frame` JSON lines on stderr.

## CLI: warm server

```text
effectcraft-cli serve --control PORT [--idle-exit SECONDS] [--gpu] [--project F.ecproj]
```

- Bind **`127.0.0.1` only**. `--control 0` picks an ephemeral port.
- On start, stderr (and only stderr) gets
  `{"event":"listening","host":"127.0.0.1","port":N,"pid":P,"gpu":bool,"idleExit":S}`.
- `--idle-exit` (default **120**; `0` = never) exits 0 when no render is running and no client has
  spoken for that many seconds. `app.quit` / `server.shutdown` exits promptly. EncodeCraft should
  send `app.quit` when it exits so the child does not linger.
- One render at a time. Projects stay loaded: `project.open` of the same path is a no-op.

### Transport

Same shape as the desktop control channel: TCP, one JSON object per line, 4 MiB max line.
Unsolicited events have `"event"` and no `"id"`. Replies echo `"id"` when the request had one.

Request: `{"id":1,"method":"render.start","params":{...}}`

Reply: `{"id":1,"ok":true,"result":{...}}` or `{"id":1,"ok":false,"error":"..."}`.

### Methods

| Method | Params | Result |
|---|---|---|
| `hello` / `server.info` | `{}` | `{version, gpuRequested, gpu, project, rendering}` |
| `project.open` | `{path}` | `{path, reopened}` — skipped if already that file |
| `project.close` | `{}` | empty project |
| `render.start` | see below | blocking; events on the **same** connection; final `result` is the `done` object |
| `render.cancel` | `{}` | `{cancelling}` — another connection may call this during `render.start` |
| `render.status` | `{}` | `{rendering, project, gpu}` |
| `app.quit` / `server.shutdown` | `{}` | `{bye}` then the process exits |

### `render.start` params

```json
{
  "project": "C:\\jobs\\spot.ecproj",
  "comp": "Main",
  "out": "-",
  "pixFmt": "yuv420p",
  "audioOut": "C:\\tmp\\audio.wav",
  "audioOnly": false,
  "start": 0.0,
  "end": 10.0,
  "workArea": false,
  "fps": 25.0,
  "resolution": "full",
  "quality": "best",
  "sidecar": "C:\\tmp\\header.json"
}
```

`out` / `audioOut` / `sidecar` are paths or `"-"`. `pixFmt` accepts `yuv420p` / `rgb24` / `rgba` /
`raw`. `format: "wav"` with `audioOnly: true` is the audio-only path. The server may also `file.open`
the `project` path when it is not already loaded.

While `render.start` runs, the same connection receives header/frame/done (or cancelled) events as
plain JSON lines, then the matching `ok` reply.

## Events (stderr and serve)

```json
{"event":"header","version":1,"width":6880,"height":1032,"fps":25.0,"frames":250,
 "pixFmt":"yuv420p","bytesPerFrame":10650240,"audio":true,"sampleRate":48000,
 "audioChannels":2,"gpu":false,"comp":"Main","out":"-","audioOut":"audio.wav"}
{"event":"frame","n":1,"total":250,"elapsed":0.04}
{"event":"gpu","used":false,"reason":"no usable GPU adapter; rendering on the CPU"}
{"event":"done","frames":250,"width":6880,"height":1032,"seconds":24.5,"bytes":2662560000,
 "audioBytes":1920000,"gpu":false,"pixFmt":"yuv420p","out":"-"}
{"event":"cancelled","reason":"pipe closed"}
{"event":"error","message":"..."}
{"event":"listening","host":"127.0.0.1","port":9878,"pid":1234,"gpu":true,"idleExit":120.0}
{"event":"idle-exit","seconds":120.0}
```

`header.audio` is whether the comp has an audible mix (EncodeCraft can skip WAV). Video streams
never contain audio.

## FFmpeg (video from the pipe)

```text
effectcraft-cli render --project F.ecproj --format yuv420p --out - --gpu --sidecar hdr.json
ffmpeg -f rawvideo -pix_fmt yuv420p -s:v 6880x1032 -r 25 -i pipe:0 ...
```

Read `width`, `height`, `fps`, `pixFmt` from the header event or sidecar **before** configuring
FFmpeg, or start FFmpeg after the header line (the first video byte follows the header event, not
stdout).

## Suggested EncodeCraft flow

1. Start once: `effectcraft-cli serve --control 0 --idle-exit 120` with `CREATE_NO_WINDOW` (`--gpu` only when Settings GPU render is on).
2. Parse `listening.port` from stderr.
3. `project.open` the `.ecproj`.
4. `render.start` with `out` a path (or a pipe FFmpeg already has open), `pixFmt: "yuv420p"`, optional `audioOut`, and **`fps` set to the preset output rate** (Better Res `25`) so the CLI renders that many frames over the same duration.
5. Show `frame.n / frame.total` from the first event. Stream into x264 **pass 1** while writing a
   cheap raw file for **pass 2** (true 2-pass; studio bitrates unchanged). Delete the raw file
   (and sidecar / WAV / leftover mezzanine) on success, failure, and cancel; sweep leftovers on startup.
6. `render.cancel` on user stop; `app.quit` when EncodeCraft exits.

Cold `render --format yuv420p --out -` is the same contract without a resident process. ProRes-to-file
is the last-resort fallback when the installed CLI is too old for `yuv420p` / `serve`.
