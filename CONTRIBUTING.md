# Contributing

## Before a large change

Open an issue first. f3note has a deliberately narrow scope, and the most
likely reason for a patch to be refused is that it would widen it — not that
the code is wrong. Saving you that work is worth a short conversation.

Things that are out of scope on purpose: split view, multiple cursors,
plugins, LSP, terminal emulation, project trees. Not because they are bad,
but because there are good editors that do them and none of them start in
150ms.

## What a change has to pass

```sh
./scripts/test.sh
```

That runs formatting, the borrow-guard check, clippy as errors, the unit
tests, the window test, the crash-recovery test, and the startup and
long-line measurements. CI runs the same thing.

If you change behaviour, the test suite should say so. If you fix a bug, add
the case that was broken — every bug found by a user in this codebase has a
test named after what went wrong, and that is deliberate.

## Two rules that come from real crashes

**Never hold a RefCell borrow across a call.**

```rust
// No. The guard lives for the whole block, and the call re-enters.
if let Some(x) = self.cell.borrow_mut().take() { self.something(); }

// Yes.
let x = self.cell.borrow_mut().take();
if let Some(x) = x { self.something(); }
```

`scripts/check-borrows.sh` enforces this and fails the build. Three separate
crashes here came from that shape, all found by a user rather than a test.

**A GTK critical is a failure.** The window test runs under
`G_DEBUG=fatal-criticals`. A critical means GTK was handed something it
should never have been handed; letting them print is how a broken popover
teardown reached a user.

## Style

Comments explain *why*, not *what*. If the reason is a measurement, put the
number in. If it is a bug that happened, say what it was — the next person to
touch that code needs to know it was deliberate.

Commit messages describe what changed and why in prose. No trailers, no
attribution lines, no emoji.
