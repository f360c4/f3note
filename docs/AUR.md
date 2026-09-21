# Publishing on the AUR

The `PKGBUILD` in `packaging/` is ready. This is how to get it onto the AUR.
Do it once; after that a release is two commands.

## Once, to set up

**1. Create an AUR account** at https://aur.archlinux.org/register. The
username is separate from your GitHub one.

**2. Give it an SSH key.** The AUR only accepts SSH, never a password:

```sh
ssh-keygen -t ed25519 -C "aur" -f ~/.ssh/aur
cat ~/.ssh/aur.pub
```

Paste that public key into **My Account → SSH Public Key** on the AUR site.

**3. Tell SSH to use it** — add to `~/.ssh/config`:

```
Host aur.archlinux.org
  User aur
  IdentityFile ~/.ssh/aur
  IdentitiesOnly yes
```

**4. Check it works:**

```sh
ssh aur@aur.archlinux.org help
```

It should greet you by name. If it asks for a password, the key is not
registered yet.

## Publishing

**1. Build the release and fill in the recipe:**

```sh
./scripts/release.sh 1.0.1
```

That archives one named commit with `gzip -n`, checks it produces the same
bytes twice, and writes the commit hash and the checksum into the PKGBUILD.

Two choices in there are deliberate, and a reviewer already caught the
alternatives in another package of mine:

**The source is a commit, not a tag.** A Git tag is a mutable pointer —
whoever controls the repository can move it — so a recipe that fetches
`#tag=v1.0.0` can compile something other than what was reviewed. A commit
hash cannot be moved.

**The source is an uploaded release asset, not GitHub's
`/archive/<ref>.tar.gz`.** Those are generated on demand, and GitHub has
never promised them to be byte-identical over time. When they changed their
tar output in January 2023 it invalidated checksums across Homebrew, the AUR
and Nix at once. A stored asset cannot change under you.

**`sha256sums` is never `SKIP`.** `SKIP` means the build accepts whatever it
downloads, which is the whole point of the checksum gone.

**2. Publish the release with the tarball attached.** If it is missing, the
recipe's source URL 404s:

```sh
git add packaging/PKGBUILD && git commit -m "Package 1.0.1"
git tag -a v1.0.1 -m "f3note 1.0.1"
git push && git push --tags
gh release create v1.0.1 target/f3note-1.0.1.tar.gz target/f3note-x86_64.AppImage \
  --title "f3note 1.0.1" --notes-file CHANGELOG.md
```

**3. Check that someone else can verify it**, from a clean clone, the way a
reviewer would:

```sh
git clone https://github.com/f360c4/f3note /tmp/verify && cd /tmp/verify
COMMIT=$(grep '^_commit=' packaging/PKGBUILD | cut -d= -f2)
git archive --format=tar --prefix=f3note-1.0.1/ $COMMIT | gzip -n -9 | sha256sum
curl -fsSL https://github.com/f360c4/f3note/releases/download/v1.0.1/f3note-1.0.1.tar.gz | sha256sum
```

Both must match `sha256sums` in the PKGBUILD. If they do not, do not publish:
a checksum nobody can reproduce is not much better than `SKIP`.

**4. Push to the AUR:**

```sh
git clone ssh://aur@aur.archlinux.org/f3note.git ~/aur-f3note
cd ~/aur-f3note
cp ~/Documents/f3note/packaging/PKGBUILD .
makepkg --printsrcinfo > .SRCINFO   # required; a push without it is rejected
makepkg -si                          # build it yourself before anyone else does
git add PKGBUILD .SRCINFO
git commit -m "Add f3note 1.0.1"
git push
```

## For each new release

```sh
cd ~/Documents/f3note
./scripts/release.sh 1.1.0          # rebuilds the tarball, rewrites the hash
# commit, tag, push, gh release create — as above

cd ~/aur-f3note
cp ~/Documents/f3note/packaging/PKGBUILD .
makepkg --printsrcinfo > .SRCINFO
git commit -am "Update to 1.1.0"
git push
```

## Things that will bite

**The tag has to exist first.** The PKGBUILD downloads
`archive/refs/tags/v$pkgver.tar.gz` from GitHub. Publish the release before
the AUR package, not after.

**Never let `SKIP` back in.** It means the build accepts whatever it
downloads. `scripts/release.sh` writes a real hash; if you edit the PKGBUILD
by hand, run the verification in step 3 again.

**Rust 1.92.** The PKGBUILD depends on `cargo`, and Arch's is current, so
this is fine on Arch — but it is worth knowing if anyone asks why it will not
build elsewhere.

**Do not commit `pkg/`, `src/` or the tarball.** The AUR repository holds two
files: `PKGBUILD` and `.SRCINFO`. Add a `.gitignore` if you are unsure.

**You become the maintainer.** People will file comments on the package page
for build failures — including ones caused by Arch moving GTK under you.
That is the deal, and it is a real ongoing commitment rather than a one-off
publish.
