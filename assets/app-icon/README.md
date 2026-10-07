# App icon (swap in)

**Format E** (isometric `E` on a rounded square) is the approved shape. Colour is
still being chosen, so it is **not** compiled into the Windows exe yet.

The teal candidate lives in [`format-e/`](format-e/) (`.ico`, PNGs 16–1024, SVG
masters). Do not copy those files to the names below until the final colour
arrives.

To embed, drop the chosen colour here and rebuild:

| file | used for |
| --- | --- |
| `encodecraft.ico` | Windows `encodecraft.exe` and `encodecraft-cli.exe` (Explorer, shortcuts, taskbar) |
| `encodecraft-256.png` | egui/eframe window icon (title bar / taskbar). `encodecraft-64.png` is used if 256 is missing. |

`apps/encodecraft/build.rs`, `apps/encodecraft-cli/build.rs`, and
`crates/ui-egui/build.rs` pick those two filenames up automatically. If they are
absent, the build stays icon-less.
