# f3note

A fast, minimal tabbed text editor for Wayland. Windows Notepad's simplicity,
Notepad++'s tabs and crash recovery, in a binary that starts in ~130ms.

Built for tiling-WM users (Hyprland, Sway, river), but it depends on nothing
specific to them and runs on any desktop.

## What it does

- **Never loses your work.** Every buffer, including unnamed scratch tabs, is
  mirrored to disk continuously. Pull the power cord and everything comes back
  exactly as it was — no "do you want to save?" dialog, ever.
- **Tabs that stay out of the way.** `Ctrl+T`, `Ctrl+W`, `Alt+1..9`,
  `Ctrl+Tab` in most-recently-used order, `Ctrl+P` to jump by typing.
- **One window.** `f3note other.txt` from a terminal opens a tab in the window
  you already have open, instead of spawning a second process.
- **Follows your theme.** Reads the palette your system already defines and
  recolors live when you switch themes — no restart, no lost tabs.
- **Honest about limits.** Files with very long lines (minified CSS, single-line
  JSON dumps) would freeze the text widget, so f3note detects them, turns off
  highlighting and wrapping, and tells you it did.

## Status

Early development. Not yet usable.

## License

MIT. See [LICENSE](LICENSE).

f3note links GTK4 and GtkSourceView5, which are LGPL-2.1-or-later. Binary
bundles (AppImage) link them dynamically and ship their license texts, so you
can replace them with your own build. See [docs/LICENSING.md](docs/LICENSING.md).
