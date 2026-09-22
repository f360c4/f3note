#!/bin/bash
# Produce the manifest Flathub receives, into target/flathub/.
#
# It differs from packaging/flatpak/io.github.f360c4.f3note.yml in exactly one
# way: the source is the published release tarball instead of this working
# tree. Development builds from what you just edited; a submission has to
# build from something fixed that a reviewer can fetch.
#
# This is a script rather than an instruction in a document because the edit
# is mechanical and the failure mode — a manifest whose checksum no longer
# matches what is published — is silent until a bot rejects it.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep '^pkgver=' packaging/PKGBUILD | cut -d= -f2)
SHA=$(grep -oP "sha256sums=\('\K[a-f0-9]+" packaging/PKGBUILD)
URL="https://github.com/f360c4/f3note/releases/download/v$VERSION/f3note-$VERSION.tar.gz"
OUT=target/flathub

echo "== $VERSION"
echo "   $URL"

# Checked, not assumed: the manifest is about to promise this checksum to a
# machine that will download the file and compare.
echo "== confirming the published tarball still has that checksum"
PUBLISHED=$(curl -fsSL "$URL" | sha256sum | cut -d' ' -f1)
if [ "$PUBLISHED" != "$SHA" ]; then
  echo "   published: $PUBLISHED" >&2
  echo "   PKGBUILD:  $SHA" >&2
  echo "the release does not match the recipe; refusing to write a manifest that would fail review" >&2
  exit 1
fi
echo "   matches"

rm -rf "$OUT"; mkdir -p "$OUT"
cp packaging/flatpak/cargo-sources.json "$OUT/"
cp packaging/flatpak/io.github.f360c4.f3note.metainfo.xml "$OUT/"

VERSION=$VERSION URL=$URL SHA=$SHA OUT=$OUT python3 - <<'PY'
import os
src = open('packaging/flatpak/io.github.f360c4.f3note.yml').read()
old = """    sources:
      - type: dir
        path: ../..
        skip:
          - target
          - .git
      - cargo-sources.json
"""
new = """    sources:
      # The published release, not this working tree. The checksum is over an
      # uploaded asset rather than GitHub's generated /archive/<ref>.tar.gz:
      # those are produced on demand and have changed before.
      - type: archive
        url: {url}
        sha256: {sha}
      - cargo-sources.json
""".format(url=os.environ['URL'], sha=os.environ['SHA'])
assert old in src, "the development manifest's sources block has moved"
open(os.environ['OUT'] + '/io.github.f360c4.f3note.yml', 'w').write(src.replace(old, new))
PY

echo "== wrote"
ls -1 "$OUT" | sed 's/^/   /'
