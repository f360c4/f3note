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

**1. Get the real checksum.** The PKGBUILD says `SKIP`, which is fine while
developing and not fine in a published package — it means nobody can tell if
the tarball was tampered with:

```sh
curl -fsSL https://github.com/f360c4/f3note/archive/refs/tags/v1.0.0.tar.gz \
  | sha256sum
```

Put that hash in `sha256sums=(...)` in `packaging/PKGBUILD`, replacing
`'SKIP'`.

**2. Clone the (empty) AUR repository:**

```sh
git clone ssh://aur@aur.archlinux.org/f3note.git ~/aur-f3note
cd ~/aur-f3note
```

**3. Copy the PKGBUILD in and generate the metadata.** `.SRCINFO` is what the
AUR website reads; a push without it is rejected:

```sh
cp ~/Documents/f3note/packaging/PKGBUILD .
makepkg --printsrcinfo > .SRCINFO
```

**4. Test the build before anyone else does:**

```sh
makepkg -si
```

This downloads the tarball, checks the hash, compiles, and installs. If it
fails here it will fail for everyone.

**5. Push:**

```sh
git add PKGBUILD .SRCINFO
git commit -m "Add f3note 1.0.0"
git push
```

It is live immediately at `https://aur.archlinux.org/packages/f3note`.

## For each new release

```sh
cd ~/aur-f3note
# update pkgver and the checksum in PKGBUILD, then
makepkg --printsrcinfo > .SRCINFO
git commit -am "Update to 1.1.0"
git push
```

## Things that will bite

**The tag has to exist first.** The PKGBUILD downloads
`archive/refs/tags/v$pkgver.tar.gz` from GitHub. Publish the release before
the AUR package, not after.

**`SKIP` as a checksum is a real problem in a published package.** It means
the build accepts whatever it downloads. Use the real hash.

**Rust 1.92.** The PKGBUILD depends on `cargo`, and Arch's is current, so
this is fine on Arch — but it is worth knowing if anyone asks why it will not
build elsewhere.

**Do not commit `pkg/`, `src/` or the tarball.** The AUR repository holds two
files: `PKGBUILD` and `.SRCINFO`. Add a `.gitignore` if you are unsure.

**You become the maintainer.** People will file comments on the package page
for build failures — including ones caused by Arch moving GTK under you.
That is the deal, and it is a real ongoing commitment rather than a one-off
publish.
