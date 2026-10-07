# EncodeCraft security review

Date: 2026-10-07  
Scope: local desktop encode queue (egui GUI + CLI), loopback control server, FFmpeg as an **external** binary, Windows zip distribution.  
Method: attacker model (local process, malicious webpage, planted PATH, watch-folder tricks) plus project-manager bar (“would I trust this with a client’s master?”).

This document is the review **and** the record of fixes landed in the same change. Residual risk is called out as **recommended**, not done.

## Threat model (what we assume)

| Actor | Can they? |
|-------|-----------|
| The person sitting at the PC | Yes — full control of queue, output folder, FFmpeg path. This is a local tool, not a multi-user service. |
| Another process on the same machine | Can open `127.0.0.1:9878`, drop files in the inbox/watch folder, or plant `ffmpeg` on `PATH`. |
| A webpage the user visits | Historically could `POST` to localhost (DNS rebinding / CSRF). Browsers are tightening this; we still treat it as in-scope. |
| Someone on the LAN | Should not reach the control port if we only bind loopback. |
| The Windows zip | Unsigned; SmartScreen will scare non-developers. Bundled FFmpeg is GPL (Gyan essentials). |

We do **not** claim: sandboxing, seccomp, Windows ACLs on the output folder, code signing, or an auto-updater.

---

## Findings

| Sev | What | Impact | Fix |
|-----|------|--------|-----|
| **Critical** | Control server on `127.0.0.1:9878` accepted unauthenticated `POST /v1/enqueue` and `/v1/control`. Response included `Access-Control-Allow-Origin: *`. | Any local process — and a web page via DNS rebinding / cross-origin POST — could enqueue jobs, pick **output paths**, and run `effectcraft-cli` on an arbitrary `.ecproj`. That is arbitrary file overwrite plus “work the user didn’t ask for”. | **Done.** Per-install token in the data dir (`ipc-token`, 256-bit hex, `0600` on Unix), also `ENCODECRAFT_TOKEN`. HTTP requires `X-EncodeCraft-Token` (or `Authorization: Bearer`). JSON-lines requires `"token"`. `GET /health` stays open (no secrets). No CORS headers. |
| **Critical** | No `Host` / `Origin` / `Sec-Fetch-Site` checks. | DNS rebinding (`evil.com` → `127.0.0.1`) and browser CSRF. | **Done.** `Host` must be `127.0.0.1` / `localhost` / `::1`. Non-loopback `Origin` (including `null`) rejected. `Sec-Fetch-Site: cross-site` rejected. Bind remains IPv4 loopback only. |
| **High** | `EnqueueRequest.output_dir` and naming templates could write anywhere the user can write. `{name}` sanitised `/` but `output_dir: /etc` or `C:\Windows` was honoured. Silent overwrite of existing files. | Clobber client masters, overwrite random files if CSRF succeeded. | **Done** for HTTP/inbox: output dir must be the configured folder or a subfolder (lexical `..` normalisation). Filenames strip separators/control chars, Windows reserved names (`CON`, `NUL`, …), leading dashes, and overlong stems. Existing files get `-2`, `-3`, … unless Settings `allow_overwrite`. UI/watch folders keep the user-chosen directory (trusted). |
| **High** | Encode wrote straight to the final path with FFmpeg `-y`. Crash/kill mid-encode left a truncated file that could be mistaken for a finished deliverable. Persist reset `Encoding` → `Waiting` but did not delete the stub. | Client gets a 2-second “final” MP4. | **Done.** Encode to `.{name}.encodecraft-partial`, then rename. Cancel/fail/stall deletes the partial. Job is `Done` only after rename. |
| **High** | FFmpeg/`effectcraft-cli` discovery walked `PATH` including `.` and relative entries. Windows `CreateProcess` also searches the cwd. Relative `FFMPEG_PATH` would resolve against cwd. | A file named `ffmpeg.exe` next to a media clip could be executed. | **Done.** Sidecar next to *this* exe still wins. Env/override/PATH entries must be **absolute**. `.` and relative `PATH` components skipped. Resolved paths are canonicalised. Spawn is always `Command` argv (never a shell) with stdin closed. |
| **High** | Source/output paths starting with `-` were passed as FFmpeg args after `-i` / as the last argument. | FFmpeg can treat `-ss.mp4` as an option. | **Done.** `argv_path` prefixes relative dash-names with `./`. Sanitize also prefixes `_` on leading `-`. |
| **Medium** | Watch folder encoded files the moment they appeared, followed directory symlinks, and would re-ingest anything written into a watched output folder. Inbox followed JSON symlinks and had no size cap. | Partial copies encode; symlink to a huge/sensitive file; infinite encode loop. | **Done.** Watch: skip symlinks, skip output/work dirs and `*encodecraft-partial*`, skip 0-byte and >64 GiB, require size+mtime stable across scans. Inbox: skip symlinks, cap 256 KiB, wait until stable, quarantine bad JSON. |
| **Medium** | HTTP body cap was 4 MiB; no header-count limit; JSON-lines unbounded. One thread per connection. | Local DoS (slowloris / header bomb / connection flood). | **Done.** Body ≤ 256 KiB, ≤64 headers, 8 KiB/line, 10 s I/O timeout, ≤16 concurrent connections, schema version still enforced (`1`). |
| **Medium** | Settings/queue used tmp+rename (fails to replace on Windows). Presets wrote in place. Corrupt JSON could wipe the queue without a backup. | Lost queue on a crash during save; Windows users more exposed. | **Done.** Atomic write + replace; corrupt files renamed `*.json.corrupt-*` then defaults. |
| **Medium** | Hung FFmpeg (broken decoder, network source) ran forever. Worker panics tore down the thread with the job stuck `Encoding`. App exit did not always kill children. Logs unbounded except a soft trim. | Stuck queue, orphan ffmpeg.exe, disk filled with logs. | **Done.** Stall timeout (default 120 s, no progress → kill). `catch_unwind` on the encode thread. `ChildGuard` kills on drop. `stop_all` on GUI/CLI shutdown. Job log trimmed; session log capped. Disk-full sniffed from FFmpeg stderr. Missing/symlink sources fail the job instead of calling FFmpeg. |
| **Medium** | `effectcraft-cli` still runs on whatever project path a **token-holding** client sends. | Stolen token (or malware already on the box) can render any `.ecproj` the user can read. | **Accepted** for a local companion app. Token file is user-private. **Recommended:** optional allow-list of project roots; never run EffectCraft projects from the inbox without a prompt. |
| **Low** | `GET /health` unauthenticated. | Port scan confirmation. | **Left open** on purpose so EffectCraft can detect a live encoder. No queue contents. |
| **Low** | JSON-lines and HTTP share one port. | Confused-deputy if a client sends the wrong framing. | Peek on `POST`/`GET`; otherwise JSON-lines. Token required on both. |
| **Low** | `ttf-parser` 0.25.1 is unmaintained (`RUSTSEC-2026-0192`, via egui). `cargo audit` found **no** known exploitable advisories in `Cargo.lock`. | Supply-chain hygiene. | **Recommended:** follow egui; add `cargo deny` / `cargo audit` to CI. Lockfile is committed. |
| **Low** | Windows zip is **unsigned**. Bundled FFmpeg is **GPL** (Gyan essentials 9.0.2, libx264/libx265). | SmartScreen “Windows protected your PC”; distributing the zip means shipping GPL binaries (notice is in the zip). | **Done** in the zip: `README-Windows.txt` (More info → Run anyway), `FFmpeg-LICENSE.txt` (GPL-3), `FFmpeg-NOTICE.txt`. **Recommended:** Authenticode cert before any “download from the web” marketing. No auto-updater (good — that would be new attack surface). Updates = replace the unzipped folder. |
| **Info** | Control port is not bound on `[::1]`. | IPv6-only localhost clients must use `127.0.0.1`. | Intentional (smaller bind surface). |
| **Info** | Tests share a process-global data dir override; they take a lock so they don’t clobber each other. | Test-only. | `paths::lock_home()` in tests. |

---

## What we did **not** fix (on purpose or later)

- **No OS sandbox** around FFmpeg (job objects / AppContainer / Landlock). A malicious media file can still exercise FFmpeg bugs — that is inherent in “shell out to FFmpeg”.
- **No max encode duration / max output bytes** beyond stall detection and watch-folder size skip.
- **Inbox is still a local control channel** (any process that can write the data dir can enqueue). The data dir is as trusted as the user account.
- **No code signing, notarisation, or Microsoft Store** packaging.
- **No `cargo deny` CI job yet** (audit was run once; ttf-parser unmaintained warning only).
- **EffectCraft patch** now sends the token header; older copies of the snippet will get `401` until updated.

---

## Project manager

### Biggest remaining risks

1. **FFmpeg is a huge native attack surface.** We never link it, but we *run* it on whatever file hits the queue. Treat untrusted media (downloads, client USB sticks) as untrusted code.
2. **The token is a file, not a prompt.** Malware that can already read `%APPDATA%\EncodeCraft` can enqueue. That is the same bar as “malware can already read your documents”.
3. **Unsigned Windows zip.** Non-developers will hit SmartScreen. Some will bounce; some will click Run anyway on *anything* with that name. Branding and a signed build matter more than another feature.
4. **GPL FFmpeg in the zip.** Fine for a personal tool; a studio redistributing EncodeCraft+FFmpeg must keep the GPL notices (we ship them) and should talk to counsel before a commercial installer.

### Before anyone relies on this for **client** work

Must-have:

- [x] Loopback + token + no CORS on the control port  
- [x] Outputs stay in the chosen folder; no silent overwrite  
- [x] Temp-then-rename so a crash isn’t a “finished” file  
- [x] Sidecar FFmpeg, not a planted cwd binary  
- [ ] **Operator practice:** set the output folder to a dedicated `EncodeCraft exports` directory, not the client’s master tree  
- [ ] Spot-check one encode (play the file, check duration) before sending to a client  
- [ ] Keep `ffmpeg.exe` / `ffprobe.exe` beside the app; don’t mix with a random PATH install  

Should-have before a v1.0 you would sell:

- Authenticode signature  
- CI `cargo audit` + `cargo deny`  
- In-UI “FFmpeg missing / token path / last error” that a non-developer can screenshot  
- Optional confirm dialog the first time EffectCraft (or any HTTP client) enqueues  
- A real installer (Start Menu shortcut) so people don’t run from Downloads  

Nice-to-have:

- Allow-list of source/output roots  
- Per-job max runtime and max output size  
- Windows Job Object so killing the app always kills FFmpeg  
- LGPL FFmpeg build if you want to avoid GPL in the zip (no x264/x265 — you’d lose the presets people actually need)

### Suggested roadmap

1. **Now (0.1.x)** — this hardening + the Windows zip. Use it yourself; don’t promise SLA to clients yet.  
2. **0.2** — signed zip, CI audit, first-run confirm for HTTP enqueue, export-folder picker that’s hard to set to “Desktop”.  
3. **0.3** — watch-folder UI that shows “skipped symlink / too large / output loop”, and a log export for when a job fails.  
4. **1.0** — installer, signing, documented support window, and a written “we shell out to FFmpeg” disclaimer in the README a client can live with.

### Distribution notes (Windows zip)

- Rebuild: `./scripts/build-windows.sh`  
- Smoke (optional): `./scripts/smoke-windows-wine.sh`  
- FFmpeg in the zip is Gyan **essentials** (GPL). EncodeCraft itself stays MIT OR Apache-2.0 and does not link FFmpeg.  
- Updates: unzip a newer folder; there is no auto-updater.

`cargo audit` on this tree (2026-10-07): **no yanked/vulnerable crates**; one unmaintained warning (`ttf-parser`, transitive via egui).
