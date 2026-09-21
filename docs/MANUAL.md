# f3note manual

## Opening files

There is no menu bar and no toolbar, which is the point but does mean nothing
on screen tells you how to open a file. Three ways:

- **`Ctrl+O`** opens a file chooser.
- **Drop a file on the window.** From a file manager, a terminal, anywhere.
- **From a terminal:** `f3note notes.txt`. If f3note is already running, the
  file opens as a tab in the window you already have, not in a new process.
  Opening a file that is already open just focuses its tab.

`Ctrl+T` makes an empty tab. It has no file behind it until you save it, and
`Ctrl+S` on it asks where to put it.

## Tabs

`Ctrl+W` closes the current tab. Closing the last one leaves an empty tab
rather than closing the window — quitting is `Ctrl+Q`, or your compositor's
close-window shortcut.

`Alt+1` through `Alt+8` jump by position. `Alt+9` is the **last** tab, not the
ninth, which is what browsers do and what is actually useful.

`Ctrl+Tab` goes to the tab you used **before this one**, not the next one along
the bar. With a handful of tabs the difference is invisible; with thirty it is
the difference between a shortcut that works and one you stop using. Press it
again to come back.

`Ctrl+P` jumps to a tab by typing part of its name. Matching is by
subsequence, so `mn` finds `main.rs`. The list starts in most-recently-used
order, so pressing `Ctrl+P` and `Enter` goes where `Ctrl+Tab` would.

Tabs can be dragged to reorder. A tab with unsaved changes is marked
`*filename` in the theme's accent colour.

## Going back

`Ctrl+Shift+R` discards the unsaved changes in the current tab and reloads the
file from disk. It asks first, because it is destructive — and the version it
throws away goes into the history, so even that is reversible.

`Ctrl+Shift+H` lists this document's earlier versions and puts one back. This
is the part no save prompt can substitute for: the list survives saving, so
"I broke this an hour ago and saved it" is recoverable. Restoring is one undo
step, and the version being replaced is kept too, so going back is never a
one-way door.

## Sessions

`Ctrl+Shift+E` saves the open tabs under a name and switches between saved
sets. Useful when you work in bursts: a set of config files, a set of notes,
and neither cluttering the other.

Switching loses nothing. The tabs being closed are mirrored exactly as they
are on exit, so unsaved work in a session you leave is there when you return.

## Finding and replacing

`Ctrl+F` opens a strip at the bottom, not a dialog. It never covers your text
and never takes the window hostage. If you have text selected, it starts with
that.

`Enter` and `Shift+Enter` step forward and back, as do `Ctrl+K` and
`Ctrl+Shift+K`. `Escape` closes the strip and puts the cursor back where it
was.

`Ctrl+H` is the same strip with a replacement field. `Enter` there replaces the
current match; `Ctrl+Enter` replaces all of them, as one undo step — `Ctrl+Z`
puts every one of them back at once.

### Across files

`Ctrl+Shift+F` searches the open tabs and the folder the current file is in.
Open tabs are searched from what is on screen, so unsaved changes are found
too.

The folder walk is bounded so it always returns quickly: it skips `.git`,
`node_modules`, `target` and friends, ignores anything that looks binary,
caps file size and depth, and does not follow symlinks. It is not a code
search tool — `ripgrep` is better at that and f3note is not competing with it.

## Editing

| | |
|---|---|
| `Ctrl+D` | duplicate the line, or the selection |
| `Alt+Up` / `Alt+Down` | move the line, or the selection |
| `Ctrl+/` | comment or uncomment |

Commenting uses the marker for the file type, and lines up the markers at the
shallowest indentation in the block so it keeps its shape. A block that is
already fully commented gets uncommented; a mixed one gets commented. Each
command is one undo step.

## How your work is kept

This is the part worth understanding, because it is different from most
editors.

Everything you type is mirrored to disk continuously — every tab, including
untitled ones you never named. The mirror is written once your typing pauses
for about two seconds, and at least every seven seconds if you never pause. So
the most a power cut can cost you is a few seconds of typing.

**Closing is always safe.** f3note never asks whether you want to save,
because it does not need to: close the window, reopen it, and everything is
back, including untitled tabs, cursor positions and unsaved changes.

**But the file on disk is still the old one.** The mirror is f3note's copy,
not your file. If you close f3note with unsaved changes and then `cat` the
file, you see the old contents — your work is safe, but it is safe *inside
f3note*, until you press `Ctrl+S`. The `*` on the tab is what tells you the
two differ.

### Scratch tabs and things you would rather not keep

Because untitled tabs are mirrored too, a tab where you pasted a password
outlives the session. `Ctrl+Shift+W` closes a tab **and deletes everything
f3note stored about it** — the mirror and the history. Use it instead of
`Ctrl+W` when the contents should not survive.

### Version history

Every time the mirror changes, a compressed snapshot is kept, up to 200 per
document and bounded by total size. `Ctrl+Shift+H` browses them.

They live in `~/.local/state/f3note/docs/<key>/history/` as zstd-compressed
files, named by sequence number, so they are recoverable by hand too:

```sh
ls ~/.local/state/f3note/docs/*/history/
zstd -dc ~/.local/state/f3note/docs/<key>/history/00000042-<hash>.zst > recovered.txt
```

Documents larger than 8 MB keep a mirror — recovery always works — but no
history, because two hundred copies of a large file is a disk leak rather than
a feature.

## Themes

Colours come from a cascade, checked in order:

1. Omarchy's current theme, if the system has one
2. `~/.config/f3note/theme.toml`
3. A built-in palette, dark unless the desktop clearly says otherwise

Nothing requires Omarchy; it is simply first in line when present. Changing the
theme recolours the running editor — no restart, no lost tabs.

The third step defaults to dark on purpose. GTK's own
`gtk-application-prefer-dark-theme` is not a reading of what you chose — it is
an application's request for a dark variant — and on a fully dark desktop it
commonly reports false. f3note therefore also consults the GTK theme name and
the desktop's `color-scheme`, and when nothing gives a clear answer it picks
dark. Set `theme = "light"` in the config to override.

To write your own palette, use the same keys:

```toml
mode = "dark"
background = "#1a1b26"
foreground = "#a9b1d6"
accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"
red = "#f7768e"
green = "#9ece6a"
```

Anything you leave out is derived. A palette with only ANSI `color0`..`color15`
works too.

The font follows the system monospace font, which on a fontconfig system means
whatever `monospace` resolves to — so changing your terminal font changes this
too. Override with `font = "Iosevka 12"` in the config.

## Long lines

f3note handles large files well: 200 000 lines and 11 MB open in about 370ms
and scroll smoothly, because only the lines on screen are laid out.

A single very long line is the exception. GTK builds the layout for an entire
logical line at once, however little of it is visible, so finding the cursor's
column in a 36 000-character line means shaping all 36 000 characters — and
doing it again on every cursor move. Measured on the reference machine:

| Line length | Per cursor move |
|---|---|
| 500 | 2 ms |
| 2 000 | 5 ms |
| 5 000 | 12 ms |
| 8 000 | 18 ms |
| 36 000 | 44 ms |

A frame is 16ms, so past about 8 000 characters it is visible. Above the
configured limit (5 000 by default) f3note turns off highlighting and wrapping,
which roughly halves the cost, and says the file will stutter. It does not
claim to have fixed it, because it has not.

Raise `long_line_chars` if your machine is quicker than the one this was
measured on.

## Where things live

| | |
|---|---|
| `~/.config/f3note/config.toml` | your settings, optional |
| `~/.config/f3note/theme.toml` | your palette, optional |
| `~/.local/state/f3note/session.json` | which tabs are open |
| `~/.local/state/f3note/docs/<key>/mirror` | the latest contents of each tab |
| `~/.local/state/f3note/docs/<key>/history/` | compressed older versions |
| `~/.local/state/f3note/sessions/` | named sessions |
| `~/.local/state/f3note/recent` | recently opened files |
| `~/.local/share/f3note/styles/` | generated syntax colours |

State and configuration are separate on purpose: state must survive, so it is
not in a cache directory anyone considers disposable.

## Troubleshooting

**It opens slowly.** f3note pins GTK's renderer to `cairo`, because the default
Vulkan renderer cost 270-300ms to first frame against 130-150ms for cairo on
the reference machine. If your hardware starts Vulkan quickly, try
`F3NOTE_RENDERER=gl f3note` or `F3NOTE_RENDERER=vulkan f3note`. Measure it with
`F3NOTE_BENCH=1 f3note`, which prints exec-to-first-frame and exits.

**Transparency does nothing.** `opacity` only matters under a compositor blur
rule. Omarchy in particular ships with blur disabled and applies its own window
opacity compositor-side, so check there first.

**A file looks like gibberish.** f3note detects the encoding rather than
guessing from a fixed list, but a file with no clear signal can still be read
wrong. The status bar shows what it decided. If it says `(lossy)`, the bytes
could not be decoded cleanly and f3note will refuse to save over the original
rather than write replacement characters into your file.

**Two windows opened.** That should not happen; f3note holds a socket that
makes a second process hand its files to the first. If it does, please report
it with the output of `echo $XDG_STATE_HOME` and whether you have a session bus.

**Syntax highlighting is off.** It is off by default — f3note should open
looking like a notepad. Turn it on with `syntax_highlighting = true`.

**Typing is eating the text in front of the cursor.** You are in overwrite
mode — the `Insert` key toggles it, and it is easy to hit by accident. The
status bar shows `OVR` while it is on; press `Insert` again, or click `OVR`,
to go back.
