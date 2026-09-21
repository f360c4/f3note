# Security

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private reporting — the
**Security** tab, then **Report a vulnerability** — or email
f360c4@gmail.com.

Include what you did, what happened, and what you expected. A proof of
concept helps but is not required.

f3note is maintained by one person in their own time, so an answer may take a
few days. You will get one.

## What is worth reporting

f3note opens files, writes to `~/.local/state/f3note`, and talks to no
network at all. The interesting classes of problem are:

- Anything that makes f3note **write outside** its own state directory or the
  file you asked it to save, especially driven by a file's *contents* or its
  *name*.
- Anything that lets a crafted file **execute code** — through the theme
  parser, the session file, the encoding detector, or a syntax definition.
- A **path traversal** through a session name, a theme name or a recent-files
  entry.
- Leaving readable copies of a document somewhere other than the state
  directory, or leaving them behind after `Ctrl+Shift+W` (close and forget),
  which promises deletion.

## What is not a vulnerability

- **Documents are mirrored to disk in plain text** under
  `~/.local/state/f3note`. That is the whole feature: it is what makes work
  survive a power cut. Anyone who can read your home directory can read them,
  exactly as they can read the files themselves. `Ctrl+Shift+W` deletes what
  was stored for a tab when that matters.
- **Version history keeps old contents** until it is collected. Same
  reasoning.
- A file that makes the editor slow. Long lines are a documented limit with
  measurements in the README.
