# EncodeCraft job format

`encodecraft-job` is a tiny crate (serde + thiserror) that EffectCraft can depend on without
pulling FFmpeg or egui. Schema version **1**, also accepted as the string `"encodecraft.job/v1"`.

Default control port: **9878** on `127.0.0.1` (EffectCraft's control channel is 9877).

## HTTP

```
POST /v1/enqueue
Host: 127.0.0.1
Content-Type: application/json
X-EncodeCraft-Token: <token>

GET  /health          → {"ok":true,"product":"EncodeCraft"}   (no token)
GET  /v1/queue        → current jobs                         (token required)
POST /v1/control      → {"id":1,"method":"queue.start","params":{}}
```

The token is created on first launch as `ipc-token` inside the **ProjectDirs data dir**
(or `ENCODECRAFT_TOKEN`):

| OS | Token file |
|----|------------|
| Windows | `%APPDATA%\EncodeCraft\EncodeCraft\data\ipc-token` |
| macOS | `~/Library/Application Support/dev.EncodeCraft.EncodeCraft/ipc-token` |
| Linux | `$XDG_DATA_HOME/encodecraft/ipc-token` or `~/.local/share/encodecraft/ipc-token` |

`encodecraft_job::discover_ipc_token()` searches those paths (and a short fallback list that
includes the older Windows folder **without** `\data`). Extra JSON fields such as
`source_version` are ignored. Browser `Origin` / cross-site `Sec-Fetch-Site` requests are
rejected. There is no CORS header; bind is `127.0.0.1` only.

JSON-lines on the same port (EffectCraft-style): one `{ "id", "method", "params", "token" }` per line.

`output_dir` on an HTTP/inbox job must be the configured EncodeCraft output folder or a
subfolder of it. Naming tokens cannot escape that folder. Existing files are not overwritten
unless Settings ▸ allow overwrite is on (otherwise `-2`, `-3`, … is appended).

## Enqueue body

`schema` may be the integer `1` **or** the string `"encodecraft.job/v1"` (EffectCraft 0.4).
Unknown schema values return HTTP 400 with a readable error.

```json
{
  "schema": 1,
  "source": { "kind": "file", "path": "C:/ads/hero.mov" },
  "preset_id": "system.h264-mp4",
  "output_dir": "C:/exports",
  "naming": "{name}_{width}x{height}",
  "sizes": [
    { "name": "FHD Landscape", "width": 1920, "height": 1080 },
    { "name": "FHD Portrait", "width": 1080, "height": 1920 },
    { "name": "DOOH 1200x960", "width": 1200, "height": 960 },
    { "name": "DOOH 1080x864", "width": 1080, "height": 864 }
  ],
  "trim": { "in_sec": 0.0, "out_sec": null },
  "scale": "fit",
  "start_queue": true
}
```

`sizes` empty → one job at the preset's resolution (match source if unset).
`sizes` set → one queue item per size ("one source to many screens").
Match-source `{width}`/`{height}` come from `source.width`/`source.height` (EffectCraft comp size),
the composition raster in the `.ecproj`, or a probe of the rendered mezzanine / source file —
not the literal `srcxsrc` placeholder.

When `sizes[].name` (or the source file name / `{screen}` token) identifies a screen and
`preset_id` is H.264 / Better Res / Installation / `auto` / empty / `system.auto`, EncodeCraft assigns:

- **Better Res** (`system.better-res`) — ultra-wide landscape (`width/height > 5`),
  the name exception **The Avenues Gate**, and listed screens (1st Ring Road,
  Al Nassar Tower Vertical, Avenues Frame entrances). Reason shown on the queue:
  `ultra wide` or `listed screen`.
- **Installation** (`system.installation`) — every other named/sized screen, and the
  fallback when EffectCraft sends Match Source / `auto` with no matching screen rule
  (`installation default`).

`preset_id: "auto"` is documented for senders that do not want to pick a preset.

A **manual** pick of Better Res / Installation / other non-one-off presets **sticks** on
the next EffectCraft send of the same project + comp + screen. **One-off** presets do not:
**Approval Res** and **Sultan** (and any preset named with “Sultan”) revert to Auto.

Rules are stored in `presets.json` (`screen_rules`, `sticky_manuals`) and can be edited in Settings.

### EffectCraft composition source

Do **not** stream raw frames. EffectCraft already has a renderer. EncodeCraft asks
`effectcraft-cli` for a ProRes (or PNG) mezzanine, then encodes that with FFmpeg — the same
split After Effects / Media Encoder uses.

```json
{
  "schema": "encodecraft.job/v1",
  "source_version": 1,
  "source": {
    "kind": "effectcraft",
    "project": "/work/spot.ecproj",
    "comp": "Main",
    "mezzanine": "prores",
    "work_area": false,
    "width": 1080,
    "height": 1080
  },
  "preset_id": "system.h264-mp4",
  "start_queue": true
}
```

## Scale modes

| JSON | Behaviour |
|------|-----------|
| `fit` | Uniform scale, pad with black (letterbox) |
| `fill` | Uniform scale, crop overflow |
| `stretch` | Exact width × height, ignore aspect |
| `scale` | Uniform scale, no pad (output may be smaller) |

## Naming tokens

`{name}` `{width}` `{height}` `{preset}` `{codec}` `{ext}` `{date}` `{time}` `{job}` `{index}` `{screen}`

## Builtin preset ids

`system.h264-mp4` `system.better-res` `system.installation` `system.approval-res`
`system.sultan` `system.hevc-mp4`
`system.prores-mov` `system.webm-vp9` `system.gif`
`system.png-sequence` `system.jpeg-sequence` `system.aac` `system.wav` `system.mp3`

Studio H.264 (all High @ 25 fps CFR, match source size unless a screen size is set):

- **Better Res** — VBR 2-pass min 2 / target 6.2 / max 6.2 Mbps, no audio
- **Installation** — VBR 2-pass min 2 / target 4 / max 6 Mbps, no audio
- **Approval Res** — VBR 2-pass min 2 stored (clamped to 1) / target 1 / max 6 Mbps, AAC 48 kHz stereo 320 kbps; **one-off** (not remembered)
- **Sultan** — VBR 1-pass (AME H.264 default when encoding mode is omitted) min 2 / target 3 / max 3 Mbps, **no audio**; **one-off** (not remembered)

Canvases with any side > 4096 use H.264 level 6.2. If the frame exceeds level 6.2, the
queue item is flagged in-app instead of failing silently.

## Inbox fallback

If nothing is listening, `encodecraft-cli enqueue` writes
`$ENCODECRAFT_HOME/inbox/<timestamp>.json` (default: the ProjectDirs data dir above, plus
`/inbox`). The app drains that folder on start and every 750 ms.
