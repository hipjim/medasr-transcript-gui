#!/usr/bin/env bash
# scripts/build-icon.sh
#
# Renders the Lucide `mic` icon onto a teal squircle background and emits
# every raster the app + installers need:
#
#   assets/icon/mic-source.svg   (original Lucide source, ISC-licensed)
#   assets/icon/icon-1024.png    (master raster, 1024x1024 RGBA)
#   assets/icon/icon-256.png     (embedded into the egui binary)
#   assets/icon/icon.icns        (macOS .app bundle icon)
#   assets/icon/icon.ico         (Windows .exe icon, multi-resolution)
#
# Run from the workspace root. Idempotent — overwrites everything in
# assets/icon/. Requires: rsvg-convert, ImageMagick, iconutil, python3
# with Pillow.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/assets/icon"
mkdir -p "$OUT"

SRC_SVG="$OUT/mic-source.svg"
MASTER_PNG="$OUT/icon-1024.png"
EGUI_PNG="$OUT/icon-256.png"
ICNS_OUT="$OUT/icon.icns"
ICO_OUT="$OUT/icon.ico"

# ---------- 1. Source SVG ----------
if [[ ! -f "$SRC_SVG" ]]; then
    echo ">> fetching Lucide 'mic' SVG"
    curl -fsSL "https://raw.githubusercontent.com/lucide-icons/lucide/main/icons/mic.svg" \
        -o "$SRC_SVG"
fi

# ---------- 2. Render master 1024 PNG with squircle bg ----------
# Background: Tailwind teal-600 (#0D9488). Foreground: pure white mic at
# 60% canvas size. macOS-style squircle corners (~22% radius).

python3 - "$SRC_SVG" "$MASTER_PNG" <<'PYEOF'
import sys, subprocess, tempfile, os
from PIL import Image, ImageDraw, ImageFilter

SRC, OUT = sys.argv[1], sys.argv[2]
SIZE = 1024
BG   = (13, 148, 136, 255)        # teal-600
FG   = (255, 255, 255, 255)
ICON_FRAC = 0.60                  # how much of the canvas the icon fills
RADIUS_FRAC = 0.22                # squircle-ish corner radius

# Render the SVG as a white-on-transparent raster at the inner size.
inner = int(SIZE * ICON_FRAC)
with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
    tmp_path = tmp.name
try:
    # Lucide strokes are #000000 by default. We want #FFFFFF; rsvg-convert
    # doesn't recolour, so we rewrite the SVG with stroke="#FFFFFF" and
    # bump the stroke-width so it stays readable at small sizes.
    with open(SRC) as f:
        svg = f.read()
    svg_white = (
        svg
        .replace('stroke="currentColor"', 'stroke="#FFFFFF"')
        .replace('stroke-width="2"', 'stroke-width="2.4"')
    )
    with tempfile.NamedTemporaryFile("w", suffix=".svg", delete=False) as svgtmp:
        svgtmp.write(svg_white)
        svgtmp_path = svgtmp.name
    try:
        subprocess.run(
            ["rsvg-convert", "-w", str(inner), "-h", str(inner),
             "-o", tmp_path, svgtmp_path],
            check=True,
        )
    finally:
        os.unlink(svgtmp_path)

    icon = Image.open(tmp_path).convert("RGBA")
finally:
    if os.path.exists(tmp_path):
        os.unlink(tmp_path)

# Build the squircle background.
canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
mask = Image.new("L", (SIZE, SIZE), 0)
mdraw = ImageDraw.Draw(mask)
r = int(SIZE * RADIUS_FRAC)
mdraw.rounded_rectangle((0, 0, SIZE - 1, SIZE - 1), radius=r, fill=255)
bg_layer = Image.new("RGBA", (SIZE, SIZE), BG)
canvas.paste(bg_layer, (0, 0), mask)

# Composite the icon centered.
ix = (SIZE - inner) // 2
iy = (SIZE - inner) // 2
canvas.paste(icon, (ix, iy), icon)

canvas.save(OUT, "PNG")
print(f"wrote {OUT} ({SIZE}x{SIZE})")
PYEOF

# ---------- 3. Egui-embedded 256 PNG ----------
echo ">> resizing -> $(basename "$EGUI_PNG")"
sips -s format png -z 256 256 "$MASTER_PNG" --out "$EGUI_PNG" >/dev/null

# ---------- 4. .icns (macOS) ----------
# iconutil expects an .iconset directory with named PNGs at exact sizes.
echo ">> building $(basename "$ICNS_OUT")"
ICONSET="$OUT/icon.iconset"
rm -rf "$ICONSET"
mkdir "$ICONSET"
for spec in \
    "16:icon_16x16.png" \
    "32:icon_16x16@2x.png" \
    "32:icon_32x32.png" \
    "64:icon_32x32@2x.png" \
    "128:icon_128x128.png" \
    "256:icon_128x128@2x.png" \
    "256:icon_256x256.png" \
    "512:icon_256x256@2x.png" \
    "512:icon_512x512.png" \
    "1024:icon_512x512@2x.png" ; do
    px="${spec%%:*}"
    name="${spec##*:}"
    sips -s format png -z "$px" "$px" "$MASTER_PNG" --out "$ICONSET/$name" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$ICNS_OUT"
rm -rf "$ICONSET"

# ---------- 5. .ico (Windows) ----------
# Multi-resolution ICO. ImageMagick handles this in one shot from PNGs.
echo ">> building $(basename "$ICO_OUT")"
TMPDIR_ICO="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_ICO"' EXIT
for px in 16 32 48 64 128 256; do
    sips -s format png -z "$px" "$px" "$MASTER_PNG" --out "$TMPDIR_ICO/$px.png" >/dev/null
done
magick \
    "$TMPDIR_ICO/16.png" "$TMPDIR_ICO/32.png" "$TMPDIR_ICO/48.png" \
    "$TMPDIR_ICO/64.png" "$TMPDIR_ICO/128.png" "$TMPDIR_ICO/256.png" \
    "$ICO_OUT"

echo
echo "icon assets built:"
ls -la "$OUT"
