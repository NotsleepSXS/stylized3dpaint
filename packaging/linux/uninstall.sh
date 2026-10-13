#!/usr/bin/env bash
# Removes what install.sh installed (settings in ~/.config/stylized3dpaint are kept).
set -euo pipefail
data="${XDG_DATA_HOME:-$HOME/.local/share}"
rm -rf "$data/stylized3dpaint"
rm -f "$HOME/.local/bin/stylized3dpaint" "$data/applications/stylized3dpaint.desktop" "$data/mime/packages/stylized3dpaint.xml"
rm -f "$data"/icons/hicolor/*/apps/stylized3dpaint.png "$data/icons/hicolor/scalable/apps/stylized3dpaint.svg"
update-mime-database "$data/mime" >/dev/null 2>&1 || true
update-desktop-database "$data/applications" >/dev/null 2>&1 || true
echo "Stylized3DPaint removed."
