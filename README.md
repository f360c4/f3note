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
- **Honest about limits.** Line *count* is not a problem: 200 000 lines and
  11 MB open in 370ms and scroll smoothly. A single enormous line is — GTK lays
  out a whole logical line at once — so minified CSS and single-line JSON make
  the cursor stutter. f3note detects those, turns off what it can, and says so
  plainly instead of pretending it fixed it.

## Keyboard

| | |
|---|---|
| `Ctrl+T` / `Ctrl+W` | new tab / close tab |
| `Ctrl+Shift+W` | close and forget — also deletes what f3note stored about it |
| `Alt+1`..`Alt+8`, `Alt+9` | jump to tab by position; `Alt+9` is the last tab |
| `Ctrl+Tab` | previously used tab, not the next one along |
| `Ctrl+P` | jump to a tab by typing part of its name |
| `Ctrl+O` / `Ctrl+S` / `Ctrl+Shift+S` | open / save / save as |
| `Ctrl+F` / `Ctrl+H` / `Ctrl+G` | find / replace / go to line |
| `Ctrl+K` / `Ctrl+Shift+K` | next / previous match |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | zoom in / out / reset, also `Ctrl+Scroll` |
| `Ctrl+Q` | quit |

`Super+W`, or whatever your compositor uses to close a window, works too.
f3note will not stop you with a dialog: everything is already mirrored, so
closing is always safe.

## Configuration

Optional. See [docs/config.example.toml](docs/config.example.toml) — every
setting has a working default, and the file does not need to exist.

Colors come from the system. On Omarchy the current theme is picked up
automatically and follows `omarchy theme set` live. Elsewhere, write
`~/.config/f3note/theme.toml` using the same keys, or let it follow the
desktop's light/dark preference.

## Building

```sh
cargo build --release
```

Needs GTK 4.12 or newer and GtkSourceView 5.10 or newer. See
[docs/PORTING.md](docs/PORTING.md) for which distributions that covers, and
which need the AppImage instead.

```sh
./scripts/test.sh       # everything below, in one go
cargo test              # logic tests, no display required
./target/release/uitest # drives the real window: tabs, switching, closing
./scripts/crashtest.sh  # end-to-end: edit, SIGKILL, recover
```

## Status

Early development, but the core works: tabs, files, find and replace, theming,
and crash recovery are all implemented and tested. Not yet released.

## License

MIT. See [LICENSE](LICENSE).

f3note links GTK4 and GtkSourceView5, which are LGPL-2.1-or-later. Binary
bundles (AppImage) link them dynamically and ship their license texts, so you
can replace them with your own build. See [docs/LICENSING.md](docs/LICENSING.md).
