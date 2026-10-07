# Send to EncodeCraft Queue (EffectCraft)

Adobe After Effects' **Composition ▸ Add to Adobe Media Encoder Queue** hands the comp to AME
instead of encoding inside AE. EncodeCraft offers the same split:

1. EffectCraft owns the composition and the renderer.
2. EncodeCraft owns delivery presets, DOOH screen packs, the queue, watch folders, FFmpeg.

**Do not stream frames.** EffectCraft already exports H.264 / ProRes / PNG / … through
`effectcraft-cli render`. Streaming a raw frame socket would duplicate that pipeline and couple
the two processes. The handoff is a small JSON job (crate `encodecraft-job`).

Two source kinds:

| `source.kind` | When to use |
|---------------|-------------|
| `file` | You already rendered a mezzanine (or the item is footage). Fastest. |
| `effectcraft` | Send the `.ecproj` + comp name. EncodeCraft calls `effectcraft-cli render --format prores` then FFmpeg. Closest to AME. |

## 1. Depend on the job crate

Clone EncodeCraft next to EffectCraft and in EffectCraft's workspace `Cargo.toml`:

```toml
[workspace.dependencies]
encodecraft-job = { path = "../encodecraft/crates/job" }
```

In `crates/engine/Cargo.toml` (or `crates/export`):

```toml
encodecraft-job = { workspace = true }
```

The crate is MIT OR Apache-2.0, serde-only — layering-safe for EffectCraft L4.

## 2. Add a command

Copy [`../patches/effectcraft-send-to-encoder.rs`](../patches/effectcraft-send-to-encoder.rs)
to `crates/engine/src/encoder_handoff.rs` and register it as `file.sendToEncodeCraft`.

Hook it from the command table the same way `renderQueue.add` is registered. Suggested
menu path: **Composition ▸ Add to EncodeCraft Queue** (shortcut `Ctrl+Alt+M` / `Cmd+Opt+M`).

Behaviour:

1. Resolve the active composition (or a `comp` param).
2. Resolve the project path (`Session`'s open file). If unsaved, error: save first.
3. Build `EnqueueRequest` with `JobSource::EffectCraft { project, comp, mezzanine: "prores", work_area }`.
4. Optionally copy current Render Settings size overrides into `sizes` (DOOH pack).
5. `POST http://127.0.0.1:9878/v1/enqueue` with `Host: 127.0.0.1`, `Content-Type: application/json`,
   and `X-EncodeCraft-Token` from `encodecraft_job::discover_ipc_token()` (the `ipc-token` file
   in EncodeCraft's data dir). If that fails, write the JSON to EncodeCraft's inbox
   (see `docs/job-format.md`) and try to spawn `encodecraft`.

## 3. CLI without patching the UI

From a terminal, with EncodeCraft running (or with inbox pickup):

```sh
# Footage / already-rendered file
encodecraft-cli enqueue ./hero.mov --preset system.h264-mp4 \
  --sizes 1920x1080,1080x1920,1200x960,1080x864 --start

# Composition (EncodeCraft will call effectcraft-cli)
encodecraft-cli enqueue --project ./spot.ecproj --comp Main \
  --preset system.h264-mp4 --start
```

Headless encoder (CI / no GUI):

```sh
encodecraft-cli serve --port 9878 --start
```

## 4. curl smoke test

```sh
TOKEN=$(cat "${ENCODECRAFT_HOME:-$HOME/.local/share/encodecraft}/ipc-token")
curl -sS -X POST http://127.0.0.1:9878/v1/enqueue \
  -H 'Content-Type: application/json' \
  -H "X-EncodeCraft-Token: $TOKEN" \
  -H 'Host: 127.0.0.1' \
  -d '{"schema":1,"source":{"kind":"file","path":"/abs/hero.mov"},"preset_id":"system.h264-mp4","start_queue":true}'
```

## 5. Patch file

[`../patches/effectcraft-send-to-encoder.patch`](../patches/effectcraft-send-to-encoder.patch)
is a ready-to-apply unified diff against EffectCraft `main` (see the header inside for the
revision it was written against). Line numbers in `commands.rs` / `menus.rs` drift; if `patch`
fails, apply the `.rs` snippet by hand using the steps above.

## 6. Why not `renderQueue.add` inside EffectCraft?

You still can — EffectCraft's own queue encodes with FilmCraft's pure-Rust codecs. Send-to-EncodeCraft
is for **delivery variants**: many DOOH sizes, H.264/HEVC/WebM/GIF from one comp, watch folders,
and FFmpeg hardware you already trust. Keep both.
