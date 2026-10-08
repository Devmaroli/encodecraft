# Speed item 3 — study only (not implemented)

Kapildev’s 6880×1032 / 10 s Better Res job spent most of its time in two places: EffectCraft writing a ProRes HQ mezzanine (~555 Mbps), then x264 `medium` pass 1 (~32 fps) and pass 2 (~21 fps) on an RTX 3070 Ti / 24-thread PC.

This note is the study he asked for before any of those changes. **Nothing here is wired into the encoder.** The shipping work on this branch is presets + min bitrate, queue selection, no auto-start, exact comp names, hidden consoles, the full-job progress bar, and the NVENC / GPU encode toggle.

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

- Full-job progress bar covers “Rendering comp” then encode pass 1/2 with one ETA.
- Optional NVIDIA NVENC (same VBR min/target/max, `multipass fullres` when the preset is 2-pass). Off by default. Quality is not identical to x264; it is opt-in.
- Hidden child processes (no CMD flashes) so the UI stays usable while measuring.

## Decision needed

Leave pipe streaming, intermediate format, adaptive parallel jobs, and x264 tuning unimplemented until he picks a lever from the table above.
