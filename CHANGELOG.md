# Changelog

## 1.0.0

First release.

### Your work

- Every buffer is mirrored to disk continuously, including untitled tabs.
  Losing power costs at most a few seconds of typing, and nothing is asked on
  closing because there is nothing to ask about.
- Earlier versions of each document are kept and can be restored
  (`Ctrl+Shift+H`), which covers the case a save prompt cannot: realising an
  hour later that something was broken and already saved.
- Unsaved changes can be discarded and the file reloaded (`Ctrl+Shift+R`).
- `Ctrl+Shift+W` closes a tab and deletes everything stored about it, for
  scratch tabs whose contents should not outlive them.

### Tabs and files

- Tabs with `Ctrl+T`/`Ctrl+W`, `Alt+1`..`Alt+9`, `Ctrl+Tab` in
  most-recently-used order, and `Ctrl+P` to jump by typing — which also
  reopens recent files.
- Named sessions (`Ctrl+Shift+E`): save sets of tabs and switch between them
  without losing unsaved work in either.
- One window. Opening a file from a terminal lands it as a tab in the window
  you already have, with or without a session bus.
- Files can be dropped on the window.
- Encoding is detected rather than guessed from a fixed list, and line endings
  are preserved. Both are shown in the status bar and can be changed there.

### Editing

- Inline find and replace, go to line, and search across the open tabs and the
  current folder (`Ctrl+Shift+F`).
- Duplicate (`Ctrl+D`), move (`Alt+Up`/`Alt+Down`) and comment (`Ctrl+/`)
  lines. Each is one undo step.
- Zoom with `Ctrl+Scroll`.

### Appearance

- Follows the system palette, live: changing the desktop theme recolours the
  editor without restarting or losing a tab. Falls back to dark rather than
  light when the desktop gives no clear answer.
- Optional transparency for compositors with a blur rule.
- Syntax highlighting, off by default.

### Known limits

- A single very long line — minified CSS, one-line JSON — makes the cursor
  stutter, because GTK lays out a whole logical line at once. Such files open
  with wrapping and highlighting off and say so. Line *count* is not a
  problem: 200 000 lines open in 370ms.
- No split view, multiple cursors, plugins or LSP, by choice.
- Linux and Wayland only.
- Needs GTK 4.12, GtkSourceView 5.10 and Rust 1.92. Ubuntu 22.04, Debian 12
  and RHEL 9 are too old and are not supported; a Flatpak would cover them and
  does not exist yet.
