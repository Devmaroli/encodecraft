# App icon (swap in)

The **Format E** mark is the approved shape. Colour is still being chosen — do
not embed a palette until the final files arrive.

Drop the chosen assets here, then rebuild:

| file | used for |
| --- | --- |
| `encodecraft.ico` | Windows `encodecraft.exe` and `encodecraft-cli.exe` (Explorer, shortcuts, taskbar) |
| `encodecraft-256.png` | egui/eframe window icon (title bar / taskbar). `encodecraft-64.png` is used if 256 is missing. |

`apps/encodecraft/build.rs`, `apps/encodecraft-cli/build.rs`, and
`crates/ui-egui/build.rs` pick these up automatically. If the files are absent,
the build stays icon-less (no default artwork is compiled in).
