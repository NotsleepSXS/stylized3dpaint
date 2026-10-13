#!/usr/bin/env bash
# Builds dist/Stylized3DPaint-<version>-linux-x86_64.tar.gz from target/release.
# Usage: packaging/linux/package.sh   (after `cargo build --release -p stylized3dpaint`)
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ver="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
name="Stylized3DPaint-$ver-linux-x86_64"
out="$root/dist/$name"
rm -rf "$out" && mkdir -p "$out/icons"
install -m755 "$root/target/release/stylized3dpaint" "$out/stylized3dpaint"
strip "$out/stylized3dpaint" 2>/dev/null || true
for f in install.sh uninstall.sh stylized3dpaint.desktop stylized3dpaint-mime.xml; do cp "$root/packaging/linux/$f" "$out/"; done
chmod +x "$out/install.sh" "$out/uninstall.sh"
cp "$root/assets/icon/stylized3dpaint.svg" "$out/icons/"
for s in 48 64 128 256; do
    if command -v rsvg-convert >/dev/null; then rsvg-convert -w $s -h $s "$root/assets/icon/stylized3dpaint.svg" -o "$out/icons/$s.png"
    else cp "$root/assets/icon/stylized3dpaint.png" "$out/icons/$s.png"; fi
done
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$out/"
cat > "$out/README.txt" <<TXT
Stylized3DPaint $ver — open-source 3D texture painter for stylized / hand-painted texturing.

Install for your user:   ./install.sh      (app menu entry, 'stylized3dpaint' command, .s3dp files)
Run without installing:  ./stylized3dpaint [model.fbx|.glb|.obj|project.s3dp]
Uninstall:               ./uninstall.sh

Needs a Vulkan-capable GPU driver. Wayland (with pen pressure) or X11.
Source, issues: https://github.com/NotsleepSXS/stylized3dpaint
Support the project: https://boosty.to/nosleeproom
TXT
tar -C "$root/dist" -czf "$root/dist/$name.tar.gz" "$name"
echo "$root/dist/$name.tar.gz"
