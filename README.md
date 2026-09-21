# f3note

A fast, minimal tabbed text editor for Wayland. Windows Notepad's simplicity,
Notepad++'s tabs and crash recovery, in a binary that opens in about 150ms.

![f3note](docs/images/editor.png)

Built for people running tiling compositors — Hyprland, Sway, river — but it
depends on nothing specific to them and runs on any desktop.

## What it does

**It does not lose your work.** Every buffer is mirrored to disk continuously,
including unnamed scratch tabs. Pull the power cord and everything comes back
exactly as it was. There is no "do you want to save?" dialog, because there is
nothing to ask about — that dialog exists for editors that cannot make this
promise.

**Tabs stay out of the way.** `Ctrl+T`, `Ctrl+W`, `Alt+1`..`Alt+9`, `Ctrl+Tab`
in most-recently-used order, and `Ctrl+P` to jump to a tab by typing part of
its name. Past a dozen tabs nobody reads a tab bar anyway.

**One window.** `f3note other.txt` from a terminal opens a tab in the window you
already have, rather than starting a second process. It works without a session
bus too, which is where most single-instance implementations quietly fail.

**It follows your theme.** Colours come from whatever palette your system
already defines, and follow it live — switch the desktop theme and the editor
recolours without restarting or losing a tab.

**It tells you the truth about its limits.** See below.

## Install

```sh
git clone https://github.com/f360c4/f3note
cd f3note
cargo build --release
install -Dm755 target/release/f3note ~/.local/bin/f3note
```

Needs GTK 4.12 or newer and GtkSourceView 5.10 or newer, both of which most
distributions have shipped since 2023. `docs/PORTING.md` lists which
distributions are covered and which need a different route.

| Distribution | Package |
|---|---|
| Arch | `pacman -S gtk4 gtksourceview5` |
| Debian 13+, Ubuntu 24.04+ | `apt install libgtk-4-dev libgtksourceview-5-dev` |
| Fedora | `dnf install gtk4-devel gtksourceview5-devel` |
| Alpine 3.20+ | `apk add gtk4.0-dev gtksourceview5-dev` |

## Keyboard

| | |
|---|---|
| `Ctrl+O` | open a file — or drop one on the window |
| `Ctrl+S` / `Ctrl+Shift+S` | save / save as |
| `Ctrl+T` / `Ctrl+W` | new tab / close tab |
| `Ctrl+Shift+W` | close and forget — also deletes what f3note stored about it |
| `Alt+1`..`Alt+8`, `Alt+9` | jump to tab by position; `Alt+9` is the last tab |
| `Ctrl+Tab` | previously used tab, not the next one along |
| `Ctrl+P` | jump to a tab by typing part of its name |
| `Ctrl+F` / `Ctrl+H` | find / find and replace |
| `Ctrl+K` / `Ctrl+Shift+K` | next / previous match |
| `Ctrl+G` | go to line |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | zoom in / out / reset — also `Ctrl+Scroll` |
| `Ctrl+Q` | quit |

Your compositor's close-window shortcut works too. f3note will not stop you
with a dialog: everything is already mirrored, so closing is always safe.

## Configuration

Entirely optional — every setting has a working default and the file does not
need to exist. See [docs/config.example.toml](docs/config.example.toml), which
explains why each default is what it is.

```toml
[appearance]
opacity = 0.9              # needs a compositor blur rule to be worth anything
syntax_highlighting = true # off by default: it should open like a notepad
```

Colours cascade: your Omarchy theme if you have one, then
`~/.config/f3note/theme.toml`, then the desktop's light/dark preference. On an
Omarchy system the palette f3note resolves is identical to the one every other
themed application resolves — verified key by key across all 22 themes.

## Honest limits

**Long lines, not large files.** 200 000 lines and 11 MB open in 370ms and
scroll smoothly, because only the lines on screen are laid out. A single
enormous line is different: GTK lays out a whole logical line at once, so
finding the cursor's column means shaping every character in it. Minified CSS
and single-line JSON make the cursor stutter, and no setting removes that.

f3note detects such files, turns off what it can, and says so rather than
pretending the problem is handled:

![Long lines](docs/images/longlines.png)

**Version history is not in the interface yet.** Snapshots are written from the
first release, so nothing is being lost while the time-travel view is built.
They are under `~/.local/state/f3note/docs/<key>/history/`, compressed with
zstd.

**No split view, no multiple cursors, no plugins, no LSP.** Deliberately. If
you want those, you want a different editor, and there are good ones.

## Building and testing

```sh
./scripts/test.sh        # everything below, in one go
cargo test               # logic tests, no display required
./target/release/uitest  # drives the real window: tabs, find, popovers
./scripts/crashtest.sh   # end-to-end: edit, SIGKILL, recover
```

`scripts/test.sh` also runs clippy, checks the palette against the system's own
resolver, measures startup against its 200ms budget, and measures how long the
main loop stalls on a line at the configured limit. The window test runs under
`G_DEBUG=fatal-criticals`, so a GTK critical fails the build rather than
scrolling past.

## License

MIT. See [LICENSE](LICENSE).

f3note links GTK4 and GtkSourceView5, which are LGPL-2.1-or-later. Binary
bundles link them dynamically and ship their licence texts, so they can be
replaced. See [docs/LICENSING.md](docs/LICENSING.md).
