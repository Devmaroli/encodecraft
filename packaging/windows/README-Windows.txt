EncodeCraft for Windows
=======================

What this folder is
-------------------
A ready-to-run copy of EncodeCraft, a media encode queue (think Adobe Media Encoder).
You do not need to install Rust.

Double-click  encodecraft.exe  to open the app.

This zip already includes ffmpeg.exe and ffprobe.exe in the same folder. Leave them
there — EncodeCraft looks next to itself first, so encoding works without a separate
FFmpeg install.


First launch — “Windows protected your PC”
------------------------------------------
The download is not digitally signed (that costs a code-signing certificate). Windows
SmartScreen may block it once:

  1. If you see a blue window “Windows protected your PC”, click  More info.
  2. Click  Run anyway.

You should only do that for software you chose to download. After the first time,
Windows usually remembers and opens the app normally.

If Windows still refuses: right-click encodecraft.exe → Properties → at the bottom,
tick Unblock → Apply → OK, then double-click again.


How to use it
-------------
  • Drag video, audio, image sequences, or an EffectCraft .ecproj onto the window,
    or click  + Add. Dropping a .ecproj lets you pick which comps to queue.
  • Pick a preset on the right. Studio deliveries: Better Res (7/8 Mbps) and
    Installation (5/6 Mbps). Double-click a preset to edit those numbers.
  • Jobs that include a screen name/size pick Better Res or Installation
    automatically (ultra-wide and listed screens vs everything else). The queue
    shows which preset and why; you can override per item.
  • For several screen sizes from one file (for example 1920×1080 and 1080×1920),
    click  One → many sizes.
  • Click  Start queue.

Files are written to the output folder shown in Settings (Videos\EncodeCraft by default).

The other program, encodecraft-cli.exe, is optional. It is a command-line tool for
people who want to encode without opening the window. You can ignore it.


If encoding fails
-----------------
Keep ffmpeg.exe and ffprobe.exe in this same folder as encodecraft.exe.
Do not run the .exe files from inside the zip — unzip the whole folder first,
for example to Desktop\EncodeCraft.


Licenses
--------
EncodeCraft is MIT OR Apache-2.0 (see LICENSE-MIT and LICENSE-APACHE).
FFmpeg is a separate program under the GNU GPL (see FFmpeg-NOTICE.txt and
FFmpeg-LICENSE.txt). EncodeCraft is not affiliated with Adobe.
