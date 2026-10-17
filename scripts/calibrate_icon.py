
import os
from PIL import Image, ImageDraw

img = Image.open("f3_brand_icons_minimalist.png").convert("RGBA")

# Let's crop the entire top-left quadrant region containing Concept 01 squircle tile
# In f3_brand_icons_minimalist.png (2048x2048):
# Concept 01 tile:
# Looking at the layout, Concept 01 is located around x: 200..900, y: 250..950.
# Let's find all pixels in the top-left quadrant where the tile exists.
# The squircle tile has dark grey color #25282d ~ #383c44 (R,G,B in 32..80).
# The board background is #121316 (R<25, G<25, B<25).
# The symbol is white/cyan (R>150 or G>150 or B>150).

# Let's find the exact tile bounding box by checking where pixel is NOT the board background:
# Board background in top-left is R <= 25, G <= 25, B <= 25.
tile_pixels = []
for y in range(250, 950):
    for x in range(180, 920):
        r, g, b, _ = img.getpixel((x, y))
        # Any pixel that is part of the tile (either tile surface or white/cyan symbol)
        if r > 28 or g > 28 or b > 30:
            tile_pixels.append((x, y))

min_x = min(p[0] for p in tile_pixels)
max_x = max(p[0] for p in tile_pixels)
min_y = min(p[1] for p in tile_pixels)
max_y = max(p[1] for p in tile_pixels)

print(f"True tile bounds: x=[{min_x}, {max_x}], y=[{min_y}, {max_y}]")
w = max_x - min_x
h = max_y - min_y
print(f"True tile size: {w}x{h}")

# The tile is a squircle. Let's make it an exact square centered at the tile center:
cx = (min_x + max_x) // 2
cy = (min_y + max_y) // 2
size = max(w, h)

# Crop with clean square bounds
crop_box = (cx - size // 2, cy - size // 2, cx + size // 2, cy + size // 2)
tile = img.crop(crop_box)
tw, th = tile.size

# Apply smooth rounded squircle mask with anti-aliasing
corner_radius = int(size * 0.225)
scale = 4
mask_large = Image.new("L", (tw * scale, th * scale), 0)
draw = ImageDraw.Draw(mask_large)
draw.rounded_rectangle([(0, 0), (tw * scale - 1, th * scale - 1)], radius=corner_radius * scale, fill=255)
mask = mask_large.resize((tw, th), Image.Resampling.LANCZOS)

tile.putalpha(mask)

os.makedirs("assets", exist_ok=True)
png_path = os.path.join("assets", "icon.png")
tile.save(png_path, "PNG")

# Generate standard ICO containing all 7 Windows sizes
ico_path = os.path.join("assets", "app_icon.ico")
icon_sizes = [(256, 256), (128, 128), (64, 64), (48, 48), (32, 32), (24, 24), (16, 16)]
tile.save(ico_path, format="ICO", sizes=icon_sizes)

print(f"Calibrated! Saved {png_path} ({tw}x{th}) and {ico_path}")
