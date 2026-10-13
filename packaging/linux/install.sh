#!/usr/bin/env bash
# Installs Stylized3DPaint for the current user (no root needed):
#   ~/.local/share/stylized3dpaint/stylized3dpaint   the program
#   ~/.local/bin/stylized3dpaint                     launcher symlink
#   app menu entry, icons, and the .s3dp project file type.
# Run from the unpacked release folder:  ./install.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
dest="$data/stylized3dpaint"
mkdir -p "$dest" "$HOME/.local/bin" "$data/applications" "$data/mime/packages"
install -m755 "$here/stylized3dpaint" "$dest/stylized3dpaint"
ln -sfn "$dest/stylized3dpaint" "$HOME/.local/bin/stylized3dpaint"
for s in 48 64 128 256; do
    mkdir -p "$data/icons/hicolor/${s}x${s}/apps"
    install -m644 "$here/icons/${s}.png" "$data/icons/hicolor/${s}x${s}/apps/stylized3dpaint.png"
done
mkdir -p "$data/icons/hicolor/scalable/apps"
install -m644 "$here/icons/stylized3dpaint.svg" "$data/icons/hicolor/scalable/apps/stylized3dpaint.svg"
install -m644 "$here/stylized3dpaint-mime.xml" "$data/mime/packages/stylized3dpaint.xml"
sed "s|@EXEC@|$dest/stylized3dpaint|" "$here/stylized3dpaint.desktop" > "$data/applications/stylized3dpaint.desktop"
update-mime-database "$data/mime" >/dev/null 2>&1 || true
update-desktop-database "$data/applications" >/dev/null 2>&1 || true
gtk-update-icon-cache -q "$data/icons/hicolor" >/dev/null 2>&1 || true
command -v xdg-mime >/dev/null && xdg-mime default stylized3dpaint.desktop application/x-stylized3dpaint-project || true
echo "Stylized3DPaint installed. Start it from the app menu or run: stylized3dpaint"
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "(add ~/.local/bin to PATH to use the command)";; esac
