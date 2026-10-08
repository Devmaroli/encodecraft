# Speed item 3 — pipe streaming (implemented)

Kapildev’s 6880×1032 / 10 s Better Res job spent most of its time in two places: EffectCraft writing a ProRes HQ mezzanine (~555 Mbps), then x264 `medium` pass 1 (~32 fps) and pass 2 (~21 fps) on an RTX 3070 Ti / 24-thread PC. Office-PC **before** wall time: **~84 s**.

**Now wired:** `yuv420p` pipe from EffectCraft into x264 pass 1 while writing a raw file for pass 2 (true 2-pass, studio bitrates unchanged — Better Res stays 2-pass 2/6.2/6.2). Warm `effectcraft-cli serve`, GPU compositor on by default with CPU fallback, at most one ultra-wide / 4K+ job at a time, ProRes mezzanine if the CLI is too old. Adaptive parallel and x264 thread/lookahead tuning are still not implemented.

The rest of this note is the original study.

## What was requested

1. Stream frames from `effectcraft-cli` straight into FFmpeg (raw video over a pipe) instead of writing then re-reading ProRes HQ. For 2-pass, either tee the stream (cheap intermediate + pass 1 at the same time) or use a cheaper intermediate than ProRes HQ.
2. Adaptive parallel jobs from cores and RAM (default Auto), keeping the two-encode live preview.
3. x264 thread / lookahead tuning for very wide frames, and a confirmed fast first pass.

NVENC is **not** part of this hold: it shipped as an opt-in (toolbar GPU encode switch + per-preset allow).

## Recommendation (when he decides)

| Lever | Likely win on 6880×1032 10 s | Quality / bitrate risk | Complexity |
| --- | --- | --- | --- |
| Pipe / tee into pass 1 | Drops the ProRes write+read (~seconds of disk, extra decode). Pass 1 can start while the comp still renders. | Low if the pipe is lossless (raw yuv420p or a fast lossless). 2-pass stats still come from the same frames. | High: EffectCraft CLI must emit a stable raw/yuv pipe, EncodeCraft must tee for pass 2 / preview / min-bitrate filler. |
| Cheaper intermediate (ProRes 422 LT or raw on fast disk) | Smaller than HQ; faster write. Weaker than a live pipe. | Low–medium. LT is still 10-bit 422; raw yuv420p matches the H.264 4:2:0 delivery. | Medium. |
| Adaptive job concurrency | Helps a *queue* of jobs, not this single 10 s encode. One 6880-wide 2-pass already saturates the CPU. | Low if RAM is respected (mezzanine + two x264 instances is large). | Medium. Live preview already supports two encodes. |
| x264 threads / lookahead / fast pass 1 | Modest. FFmpeg already passes `-preset medium` and `-pass 1` (x264’s faster analysis). `--threads` auto is usually right; very wide frames may want `sliced-threads` off and a larger `rc-lookahead`. | Must not change bitrate or picture. Easy to over-tune. | Low. |

**Best first experiment if he green-lights it:** raw `yuv420p` pipe from EffectCraft into FFmpeg pass 1, with a cheap on-disk copy only if pass 2 still needs a seekable file. Do not change target/min/max bitrates. Measure wall time on the 6880×1032 10 s Better Res case and a 1920×1080 control.

## What this branch already does for speed-adjacent work

- Full-job progress bar covers “Rendering comp” (frame N/total + duration-weighted bar from the first render tick) then encode pass 1/2 with one ETA.
- Optional NVIDIA NVENC (same VBR min/target/max, `multipass fullres` when the preset is 2-pass). Off by default. Quality is not identical to x264; it is opt-in.
- Hidden child processes (no CMD flashes) so the UI stays usable while measuring.

## Timings (6880×1032 10 s Better Res, and 1920×1080)

Office-PC **before** (this branch, ProRes HQ mezzanine then x264 medium):

| Case | EffectCraft mezzanine | x264 pass 1 | x264 pass 2 | Notes |
| --- | --- | --- | --- | --- |
| 6880×1032, 10 s, Better Res 2-pass | ProRes HQ ~555 Mbps (bar sat at 0% until this finished) | ~32 fps | ~21 fps | RTX 3070 Ti, 24 threads |
| 1920×1080 (same preset) | not measured on that PC | — | — | control not captured yet |

**After** (this branch, EncodeCraft side of EffectCraft PR #11):

| Lever | Status |
| --- | --- |
| `yuv420p` stream into x264 pass 1 + raw file for pass 2 | Wired. Better Res stays **2-pass** 2/6.2/6.2. |
| Warm `effectcraft-cli serve` | On demand, reused, `--idle-exit 120`, `app.quit` on EncodeCraft exit, `CREATE_NO_WINDOW`. |
| GPU render | On by default **when NVIDIA is present**; otherwise CPU (non-RTX adapters were slower). CLI still CPU-fallbacks if the frame does not fit. Settings checkbox; `ENCODECRAFT_GPU_RENDER=0/1`. NVENC stays a separate Off switch. This VM has no GPU and no `effectcraft-cli`, so GPU vs CPU was not measured here. |
| One ultra-wide / 4K+ job at a time | Dispatch skips a second heavy raster. Other jobs still run in parallel. |
| Old CLI | ProRes mezzanine fallback. |
| Adaptive parallel / x264 thread tune | Still not implemented. |

Office-PC **after** wall time for 6880×1032 / 10 s Better Res: **not remeasured here** (this VM has no EffectCraft project or RTX). Expected win vs **84 s** is dropping the ProRes HQ write/read and overlapping pass 1 with the render. Re-run that clip on the office installer to fill the after number.
