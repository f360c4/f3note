# Flatpak

The Flatpak exists for the distributions that cannot run f3note any other way.
Ubuntu 22.04, Debian 12 and RHEL 9 ship GTK too old to build against, and the
AppImage does not help them either — it needs a glibc newer than they have.
A Flatpak carries its own GTK from the GNOME runtime, so the host's age stops
mattering.

## Installing it

Once it is on Flathub:

```sh
flatpak install flathub io.github.f360c4.f3note
flatpak run io.github.f360c4.f3note notes.txt
```

Until then, build it yourself — see below.

If `flatpak` is new to you: it is a packaging system that ships an application
together with the libraries it needs, sandboxed from the rest of your system.
Most distributions package it as `flatpak`, and you add the main repository
once:

```sh
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
```

## Building it yourself

```sh
sudo pacman -S flatpak flatpak-builder     # or your distribution's equivalent
flatpak install flathub org.gnome.Platform//51 org.gnome.Sdk//51 \
  org.freedesktop.Sdk.Extension.rust-stable//26.08
./scripts/build-flatpak.sh
```

That takes a while the first time: the runtime and SDK are about 2 GB.

## Three things worth knowing before you change the manifest

**The runtime version is not a free choice.** f3note needs Rust 1.92 — the
gtk-rs crates require it — and each GNOME runtime pins a freedesktop base
whose `rust-stable` extension carries one fixed Rust version. GNOME 47 pairs
with base 24.08 and Rust 1.89, and the build fails outright. GNOME 51 pairs
with 26.08 and Rust 1.98. Check before changing it:

```sh
flatpak run --command=sh --devel org.gnome.Sdk//51 \
  -c '/usr/lib/sdk/rust-stable/bin/rustc --version'
```

**The build has no network.** Every crate is declared up front in
`cargo-sources.json`, regenerated from `Cargo.lock` by
`scripts/cargo-sources.py`. Run it after any dependency change, or the build
will fail on a crate it cannot fetch.

Each vendored crate also gets a `.cargo-checksum.json` carrying the hash from
the lock file. Leaving that null makes cargo refuse the crate — it reads null
as a source that cannot do checksums replacing one that can.

**The SVG icon is deliberately not exported.** Flatpak validates icons through
gdk-pixbuf, librsvg has dropped its gdk-pixbuf loader, and so an SVG is
rejected as "Format not recognized" — at the very last step, after everything
has compiled. PNGs are exported instead. GTK4 renders the SVG perfectly well,
because it uses librsvg directly, which is why a native install still uses it.

## Publishing on Flathub

Flathub is a review process, not a push. Budget days rather than minutes.

**1. Check it locally first.** A reviewer will run it, and a sandbox fails
differently from a normal build:

```sh
./scripts/build-flatpak.sh
flatpak run io.github.f360c4.f3note ~/some-file.txt
```

- does it open a file passed on the command line?
- does it pick up your desktop palette, or fall back to the built-in one?
- does the font match the rest of your desktop?
- does saving work, and does the file on disk really change?

**2. Validate the metadata.** Flathub rejects a submission with a broken
appstream file, and the error at that point is unhelpful:

```sh
flatpak run org.freedesktop.appstream-glib validate \
  packaging/flatpak/io.github.f360c4.f3note.metainfo.xml
```

**3. Generate the submission manifest.** The manifest in `packaging/` builds
from the working tree, which is right for development and wrong for Flathub:
a submission has to build from something fixed. `scripts/flathub-manifest.sh`
writes the submission copy into `target/flathub/`, with the `dir` source
replaced by the published release tarball and its checksum — the same ones
the Arch package uses:

```sh
./scripts/flathub-manifest.sh
```

It downloads the release first and compares, and refuses to write anything if
the published tarball and the recipe disagree. A manifest whose checksum no
longer matches what is published fails silently until a bot rejects it.

Then build *that* manifest, not the development one, because it is what the
reviewer's machine will build:

```sh
cd target/flathub
flatpak-builder --force-clean --repo=/tmp/f3note-repo /tmp/f3note-build \
  io.github.f360c4.f3note.yml
```

Note that the build directory and flatpak-builder's state directory have to
be on the same filesystem, or it stops before doing anything.

**A release the tarball predates is not submittable.** The 1.0.1 tarball was
cut from a commit that had no `packaging/flatpak` directory at all, so a
manifest built from it cannot find the metainfo file it installs. Whatever is
in the manifest has to exist inside the tarball named in it.

**4. Submit.** Fork `flathub/flathub`, create a branch named exactly the app
id, and open a pull request against the `new-pr` branch:

```sh
git clone https://github.com/flathub/flathub ~/flathub && cd ~/flathub
git checkout -b io.github.f360c4.f3note new-pr
mkdir io.github.f360c4.f3note
cp ~/Documents/f3note/packaging/flatpak/* io.github.f360c4.f3note/
git add io.github.f360c4.f3note
git commit -m "Add io.github.f360c4.f3note"
git push origin io.github.f360c4.f3note
```

Then open the pull request on GitHub, against `new-pr`.

A bot builds it and comments. A human reviews after that, and will ask about
anything unusual — in this manifest, that is almost certainly
`--filesystem=host`.

**The answer to that question, prepared:** f3note is opened from a terminal
with a path argument, which is most of how it is used. The file portal can
only hand over files chosen through a dialog, so a sandbox without host
filesystem access cannot open the file it was asked to open. Narrower
permissions were considered and do not cover that.

Expect to be asked to change things. That is the point of the review, and the
reviewers are usually right.
