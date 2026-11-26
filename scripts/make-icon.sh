#!/bin/sh
# Regenerates assets/AppIcon.icns from assets/icon-source.png.
set -eu
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
swift scripts/make-icon.swift assets/icon-source.png "$tmp/master.png"
set_dir="$tmp/AppIcon.iconset"
mkdir "$set_dir"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$tmp/master.png" --out "$set_dir/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z "$d" "$d" "$tmp/master.png" --out "$set_dir/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$set_dir" -o assets/AppIcon.icns
printf 'Wrote assets/AppIcon.icns\n'
sips -z 256 256 assets/icon-source.png --out assets/icon-256.png >/dev/null
printf 'Wrote assets/icon-256.png\n'
