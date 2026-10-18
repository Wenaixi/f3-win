
import os
from PIL import Image, ImageDraw

src_path = "assets/icon_new.png"
img = Image.open(src_path).convert("RGBA")
w, h = img.size

# Outer corner is the canvas background
bg_color = img.getpixel((0, 0))[:3]
print(f"Canvas size: {w}x{h}, background color at (0,0): {bg_color}")

# Scan from outer edges inward toward center:
# A pixel is considered part of the squircle if its color distance from bg_color is significant
def is_different(pixel):
    r, g, b, _ = pixel
    return abs(r - bg_color[0]) > 30 or abs(g - bg_color[1]) > 30 or abs(b - bg_color[2]) > 30

# Find min_x (left boundary)
min_x = 0
for x in range(w):
    if any(is_different(img.getpixel((x, y))) for y in range(h)):
        min_x = x
        break

# Find max_x (right boundary)
max_x = w - 1
for x in range(w - 1, -1, -1):
    if any(is_different(img.getpixel((x, y))) for y in range(h)):
        max_x = x
        break

# Find min_y (top boundary)
min_y = 0
for y in range(h):
    if any(is_different(img.getpixel((x, y))) for x in range(w)):
        min_y = y
        break

# Find max_y (bottom boundary)
max_y = h - 1
for y in range(h - 1, -1, -1):
    if any(is_different(img.getpixel((x, y))) for x in range(w)):
        max_y = y
        break

print(f"Squircle tile bounding box: left={min_x}, top={min_y}, right={max_x}, bottom={max_y}")
tile_w = max_x - min_x + 1
tile_h = max_y - min_y + 1
print(f"Tile size: {tile_w}x{tile_h}")

# Symmetrical square crop
size = max(tile_w, tile_h)
center_x = (min_x + max_x) // 2
center_y = (min_y + max_y) // 2

half = size // 2
crop_box = (center_x - half, center_y - half, center_x + half, center_y + half)
tile = img.crop(crop_box)
tw, th = tile.size
print(f"Cropped square tile: {tw}x{th}")

# Apply 4x supersampled anti-aliased squircle corner mask
corner_radius = int(tw * 0.225)
scale = 4
mask_large = Image.new("L", (tw * scale, th * scale), 0)
draw = ImageDraw.Draw(mask_large)
draw.rounded_rectangle([(0, 0), (tw * scale - 1, th * scale - 1)], radius=corner_radius * scale, fill=255)
mask = mask_large.resize((tw, th), Image.Resampling.LANCZOS)
tile.putalpha(mask)

# 1. Save master icon.png
master_png = "assets/icon.png"
tile.save(master_png, "PNG")
print(f"Successfully saved {master_png} ({tw}x{th})")

# 2. Save 256x256
img_256 = tile.resize((256, 256), Image.Resampling.LANCZOS)
img_256.save("assets/icon_256.png", "PNG")

# 3. Save 64x64 and export RGBA matrix
img_64 = tile.resize((64, 64), Image.Resampling.LANCZOS)
img_64.save("assets/icon_64.png", "PNG")
with open("assets/icon_64.rgba", "wb") as f:
    f.write(img_64.tobytes())

# 4. Save 32x32 and export RGBA matrix
img_32 = tile.resize((32, 32), Image.Resampling.LANCZOS)
img_32.save("assets/icon_32.png", "PNG")
with open("assets/icon_32.rgba", "wb") as f:
    f.write(img_32.tobytes())

# 5. Generate Windows multi-size ICO
ico_path = "assets/app_icon.ico"
icon_sizes = [(256, 256), (128, 128), (64, 64), (48, 48), (32, 32), (24, 24), (16, 16)]
tile.save(ico_path, format="ICO", sizes=icon_sizes)

print(f"All assets rebuilt cleanly: icon.png, icon_256.png, icon_64.png, icon_32.png, icon_64.rgba, app_icon.ico")
