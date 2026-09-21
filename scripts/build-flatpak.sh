#!/bin/bash
# Build and install the Flatpak locally.
#
# Prerequisites, which are a large download the first time:
#
#   sudo pacman -S flatpak flatpak-builder        # or your distribution's
#   flatpak remote-add --if-not-exists flathub \
#     https://flathub.org/repo/flathub.flatpakrepo
#   flatpak install flathub org.gnome.Platform//47 org.gnome.Sdk//47 \
#     org.freedesktop.Sdk.Extension.rust-stable//24.08
set -euo pipefail
cd "$(dirname "$0")/.."

command -v flatpak-builder >/dev/null || {
  echo "flatpak-builder is not installed; see the comment at the top of this file" >&2
  exit 1
}

MANIFEST=packaging/flatpak/io.github.f360c4.f3note.yml

echo "== regenerating the crate list from Cargo.lock"
python3 scripts/cargo-sources.py

echo "== building"
flatpak-builder --force-clean --user --install-deps-from=flathub \
  --repo=target/flatpak-repo target/flatpak-build "$MANIFEST"

echo "== installing for this user"
flatpak-builder --user --install --force-clean target/flatpak-build "$MANIFEST"

cat <<'NEXT'

Installed. Run it with:

  flatpak run io.github.f360c4.f3note

Worth checking, because a sandbox fails differently from a normal build:

  - it opens a file passed on the command line
  - it picks up your desktop palette rather than falling back to the built-in
  - the font matches the rest of your desktop
  - saving works, and the file on disk really changes
NEXT
