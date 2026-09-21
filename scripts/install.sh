#!/bin/bash
# Build f3note and install it for the current user.
#
# Checks what is missing before doing anything, and names the exact command
# for your distribution rather than leaving you to translate a build error.
# Installs under ~/.local, so no root and nothing outside your home.
#
# Read it before running it. It is short on purpose.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${PREFIX:-$HOME/.local}/bin
APPS=${XDG_DATA_HOME:-$HOME/.local/share}/applications
ICONS=${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps

say()  { printf '\033[1m%s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }

distro() {
  [ -r /etc/os-release ] || { echo unknown; return; }
  # shellcheck disable=SC1091
  . /etc/os-release
  echo "${ID_LIKE:-$ID}" | awk '{print $1}'
}

packages_for() {
  case "$(distro)" in
    arch)            echo "sudo pacman -S --needed gtk4 gtksourceview5 rustup" ;;
    debian|ubuntu)   echo "sudo apt install libgtk-4-dev libgtksourceview-5-dev" ;;
    fedora|rhel)     echo "sudo dnf install gtk4-devel gtksourceview5-devel" ;;
    suse|opensuse*)  echo "sudo zypper install gtk4-devel gtksourceview5-devel" ;;
    alpine)          echo "sudo apk add gtk4.0-dev gtksourceview5-dev" ;;
    *)               echo "install the GTK 4 and GtkSourceView 5 development packages" ;;
  esac
}

# Compare dotted versions without assuming a `sort -V` that understands them.
at_least() {
  [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -1)" = "$2" ]
}

say "Checking what is here"
missing=0

if command -v pkg-config >/dev/null; then
  for lib in "gtk4 4.12" "gtksourceview-5 5.10"; do
    set -- $lib
    if have=$(pkg-config --modversion "$1" 2>/dev/null); then
      if at_least "$have" "$2"; then
        ok "$1 $have"
      else
        bad "$1 $have — need $2 or newer"
        note "Your distribution is too old for f3note. Nothing here will fix that."
        note "See docs/PORTING.md, and try the AppImage from the releases page."
        missing=1
      fi
    else
      bad "$1 not found"
      note "$(packages_for)"
      missing=1
    fi
  done
else
  bad "pkg-config not found"
  note "$(packages_for)"
  missing=1
fi

# rustup installs into ~/.cargo/bin and adds it to your shell's rc file, which
# a non-interactive shell does not read. Someone who just ran rustup and then
# ran this would otherwise be told Rust is missing while it sits right there.
if ! command -v cargo >/dev/null && [ -x "$HOME/.cargo/bin/cargo" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
  note "using cargo from ~/.cargo/bin (not on your PATH)"
fi

if command -v cargo >/dev/null; then
  rustc_version=$(rustc --version | awk '{print $2}')
  if at_least "$rustc_version" 1.92.0; then
    ok "rust $rustc_version"
  else
    bad "rust $rustc_version — need 1.92 or newer"
    note "The gtk-rs crates require it. Your distribution's Rust may be older:"
    note "  rustup default stable     (or install rustup: https://rustup.rs)"
    missing=1
  fi
else
  bad "cargo not found"
  note "Install Rust: https://rustup.rs — or $(packages_for)"
  missing=1
fi

if [ "$missing" -ne 0 ]; then
  echo
  say "Not ready yet. Install what is marked above and run this again."
  exit 1
fi

echo
say "Building (a minute or two the first time)"
cargo build --release --bin f3note

echo
say "Installing into ${BIN%/bin}"
install -Dm755 target/release/f3note "$BIN/f3note"
install -Dm644 packaging/io.github.f360c4.f3note.desktop \
  "$APPS/io.github.f360c4.f3note.desktop"
install -Dm644 packaging/f3note.svg \
  "$ICONS/io.github.f360c4.f3note.svg"
ok "$BIN/f3note"
ok "$APPS/io.github.f360c4.f3note.desktop"
ok "$ICONS/io.github.f360c4.f3note.svg"

update-desktop-database "$APPS" 2>/dev/null || true
gtk4-update-icon-cache -f -t "${ICONS%/scalable/apps}" 2>/dev/null || true

echo
case ":$PATH:" in
  *":$BIN:"*) say "Done. Run: f3note" ;;
  *)
    say "Done, but $BIN is not on your PATH."
    note "Add this to your shell's rc file:"
    note "  export PATH=\"\$HOME/.local/bin:\$PATH\""
    note "Or run it directly: $BIN/f3note"
    ;;
esac
