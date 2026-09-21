# Licensing

f3note itself is MIT (see `LICENSE`).

## Linked libraries

| Library | License | How we link it |
|---|---|---|
| GTK4 | LGPL-2.1-or-later | dynamic |
| GtkSourceView5 | LGPL-2.1-or-later | dynamic |

MIT code may link LGPL libraries without becoming LGPL. The obligation the LGPL
does impose is that a user who receives a binary bundle must be able to replace
those libraries with their own modified build.

## What this means for the AppImage

The AppImage bundles GTK4 and GtkSourceView5. To stay compliant it must:

1. link them **dynamically** (never statically — GTK does not support static
   builds anyway, see `docs/PORTING.md`);
2. ship the full text of LGPL-2.1 alongside the bundled libraries;
3. document how to rebuild the AppImage against a replacement library, or ship
   the bundle in a form where the `.so` files can simply be swapped.

Native distro packages (Arch, Debian, Fedora, ...) link against the system
libraries and carry no extra obligation.

## Rust dependencies

Every crate in `Cargo.lock` must have its license reviewed before the first
public release. Some carry file-level copyleft (MPL-2.0) which is compatible
with an MIT project but still requires attribution. Run `cargo deny check
licenses` in CI.
