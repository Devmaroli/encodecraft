# EncodeCraft job format

`encodecraft-job` is a tiny crate (serde + thiserror) that EffectCraft can depend on without
pulling FFmpeg or egui. Schema version **1**.

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

The token is created on first launch in the data dir as `ipc-token` (or `ENCODECRAFT_TOKEN`).
`encodecraft-cli` and the EffectCraft handoff send it automatically. Browser `Origin` /
cross-site `Sec-Fetch-Site` requests are rejected. There is no CORS header; bind is
`127.0.0.1` only.

JSON-lines on the same port (EffectCraft-style): one `{ "id", "method", "params", "token" }` per line.

`output_dir` on an HTTP/inbox job must be the configured EncodeCraft output folder or a
subfolder of it. Naming tokens cannot escape that folder. Existing files are not overwritten
unless Settings ▸ allow overwrite is on (otherwise `-2`, `-3`, … is appended).

## Enqueue body

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

### EffectCraft composition source

Do **not** stream raw frames. EffectCraft already has a renderer. EncodeCraft asks
`effectcraft-cli` for a ProRes (or PNG) mezzanine, then encodes that with FFmpeg — the same
split After Effects / Media Encoder uses.

```json
{
  "schema": 1,
  "source": {
    "kind": "effectcraft",
    "project": "/work/spot.ecproj",
    "comp": "Main",
    "mezzanine": "prores",
    "work_area": false
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

`system.h264-mp4` `system.hevc-mp4` `system.prores-mov` `system.webm-vp9` `system.gif`
`system.png-sequence` `system.jpeg-sequence` `system.aac` `system.wav` `system.mp3`

## Inbox fallback

If nothing is listening, `encodecraft-cli enqueue` writes
`$ENCODECRAFT_HOME/inbox/<timestamp>.json` (default platform data dir). The app drains that
folder on start and every 750 ms.
