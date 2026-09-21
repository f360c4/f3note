#!/bin/bash
# Build a reproducible release tarball and fill the PKGBUILD in from it.
#
# The tarball is `git archive` of one named commit, gzipped with -n so no
# timestamp goes in. That is what makes it reproducible: anyone can run the
# same command on the same commit and get the same bytes, so the checksum in
# the PKGBUILD is something they can verify rather than something they have
# to believe.
#
# It must be uploaded as a release *asset*. GitHub's generated
# /archive/<ref>.tar.gz is built on demand and has changed before — the
# January 2023 change to their tar output invalidated checksums across
# Homebrew, the AUR and Nix. A stored asset cannot change under you.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${1:?usage: release.sh <version> [commit]}
COMMIT=${2:-HEAD}
COMMIT=$(git rev-parse "$COMMIT")
NAME="f3note-$VERSION"
OUT="target/$NAME.tar.gz"

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "working tree is dirty; commit first so the tarball matches a real commit" >&2
  exit 1
fi

mkdir -p target
echo "== archiving $COMMIT"
git archive --format=tar --prefix="$NAME/" "$COMMIT" | gzip -n -9 > "$OUT"

SHA=$(sha256sum "$OUT" | cut -d' ' -f1)
echo "   $OUT"
echo "   sha256 $SHA"

echo "== checking it reproduces"
SECOND=$(git archive --format=tar --prefix="$NAME/" "$COMMIT" | gzip -n -9 | sha256sum | cut -d' ' -f1)
if [ "$SHA" != "$SECOND" ]; then
  echo "the archive is not reproducible; refusing to publish a checksum nobody can verify" >&2
  exit 1
fi
echo "   same bytes twice"

echo "== filling in the PKGBUILD"
sed -i "s/^_commit=.*/_commit=$COMMIT/" packaging/PKGBUILD
sed -i "s/^pkgver=.*/pkgver=$VERSION/" packaging/PKGBUILD
sed -i "s/^sha256sums=.*/sha256sums=('$SHA')/" packaging/PKGBUILD
sed -i "s|--prefix=f3note-[0-9.]*/|--prefix=$NAME/|" packaging/PKGBUILD
grep -E '^(pkgver|_commit|sha256sums)=' packaging/PKGBUILD | sed 's/^/   /'

cat <<NEXT

Next:
  git add packaging/PKGBUILD && git commit -m "Package $VERSION"
  git tag -a v$VERSION -m "f3note $VERSION"
  git push && git push --tags
  gh release create v$VERSION $OUT target/f3note-x86_64.AppImage \\
    --title "f3note $VERSION" --notes-file CHANGELOG.md

The tarball must be attached to the release, or the PKGBUILD's source URL
will 404.
NEXT
