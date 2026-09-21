#!/bin/bash
# Build a self-contained f3note.AppImage.
#
# This exists for the distributions whose GTK is too old to run a natively
# built f3note — Ubuntu 22.04, Debian 12, RHEL 9 — where there is otherwise
# no way to install it at all. Everywhere else, the distribution package is
# the better answer and this is a fallback.
#
# GTK is bundled dynamically, never statically: GTK upstream does not support
# static builds, and the LGPL requires that a user receiving this bundle can
# replace those libraries with their own. See docs/LICENSING.md.
set -euo pipefail
cd "$(dirname "$0")/.."

APP=f3note
ID=io.github.f360c4.f3note
ARCH=$(uname -m)
BUILD=target/appimage
APPDIR=$BUILD/$APP.AppDir
TOOLS=$BUILD/tools

need() {
  command -v "$1" >/dev/null || { echo "missing: $1"; exit 1; }
}
need cargo
need ldd

echo "== building the binary"
cargo build --release --bin $APP

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$TOOLS"
mkdir -p "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/scalable/apps" \
         "$APPDIR/usr/share/glib-2.0/schemas"

install -m755 target/release/$APP "$APPDIR/usr/bin/$APP"
install -m644 packaging/$ID.desktop "$APPDIR/usr/share/applications/$ID.desktop"
install -m644 packaging/$APP.svg "$APPDIR/usr/share/icons/hicolor/scalable/apps/$ID.svg"
# AppImage tools look for these at the AppDir root.
cp packaging/$ID.desktop "$APPDIR/$ID.desktop"
cp packaging/$APP.svg "$APPDIR/$ID.svg"

echo "== collecting libraries"
# Everything the binary needs, minus what every glibc system already has.
# Bundling the core C library or the graphics drivers is how an AppImage
# becomes less portable rather than more: they must come from the host.
EXCLUDE='^(linux-vdso|ld-linux|libc|libm|libdl|libpthread|librt|libresolv|libgcc_s|libstdc\+\+|libGL|libEGL|libGLX|libGLdispatch|libOpenGL|libdrm|libgbm|libX11|libxcb|libwayland)'

copy_deps() {
  local target=$1
  ldd "$target" 2>/dev/null | awk '{ if ($3 ~ /^\//) print $3 }' | while read -r lib; do
    local base
    base=$(basename "$lib")
    if [[ $base =~ $EXCLUDE ]]; then continue; fi
    if [[ -f "$APPDIR/usr/lib/$base" ]]; then continue; fi
    cp -L "$lib" "$APPDIR/usr/lib/$base"
    copy_deps "$APPDIR/usr/lib/$base"
  done
}
copy_deps "$APPDIR/usr/bin/$APP"
echo "   bundled $(find "$APPDIR/usr/lib" -name '*.so*' | wc -l) libraries"

echo "== collecting GTK's own data"
# GdkPixbuf loaders and the GSettings schemas GTK reads at startup. Without
# the schemas GTK aborts; without the loaders no icon renders.
for dir in /usr/lib/gdk-pixbuf-2.0 /usr/lib64/gdk-pixbuf-2.0; do
  [[ -d $dir ]] && cp -r "$dir" "$APPDIR/usr/lib/" && break
done
if [[ -f /usr/share/glib-2.0/schemas/gschemas.compiled ]]; then
  cp /usr/share/glib-2.0/schemas/gschemas.compiled \
     "$APPDIR/usr/share/glib-2.0/schemas/"
fi
for dir in /usr/share/gtksourceview-5 /usr/share/gtksourceview-4; do
  [[ -d $dir ]] && cp -r "$dir" "$APPDIR/usr/share/" && break
done
mkdir -p "$APPDIR/usr/share/icons"
[[ -d /usr/share/icons/Adwaita ]] && cp -r /usr/share/icons/Adwaita "$APPDIR/usr/share/icons/"

echo "== writing AppRun"
cat > "$APPDIR/AppRun" <<'RUN'
#!/bin/sh
# Point GTK at the bundled data, then hand over. The bundled libraries go
# first on the path but the host's graphics drivers are deliberately not
# bundled, so they resolve from the system as they must.
HERE=$(dirname "$(readlink -f "$0")")
export LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export XDG_DATA_DIRS="$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSETTINGS_SCHEMA_DIR="$HERE/usr/share/glib-2.0/schemas"
export GDK_PIXBUF_MODULEDIR="$HERE/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders"
if [ -f "$HERE/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache" ]; then
  export GDK_PIXBUF_MODULE_FILE="$HERE/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
fi
exec "$HERE/usr/bin/f3note" "$@"
RUN
chmod +x "$APPDIR/AppRun"

echo "== fetching appimagetool"
TOOL=$TOOLS/appimagetool
if [[ ! -x $TOOL ]]; then
  URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
  if ! curl -fsSL "$URL" -o "$TOOL"; then
    echo
    echo "Could not download appimagetool. The AppDir is complete at:"
    echo "  $APPDIR"
    echo "Run appimagetool against it when you have network access."
    exit 1
  fi
  chmod +x "$TOOL"
fi

echo "== packing"
ARCH=$ARCH "$TOOL" --no-appstream "$APPDIR" "target/$APP-$ARCH.AppImage"
echo
echo "built: target/$APP-$ARCH.AppImage"
ls -lh "target/$APP-$ARCH.AppImage" | awk '{print "size:", $5}'
