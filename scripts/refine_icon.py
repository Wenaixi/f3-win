
import os
from PIL import Image, ImageDraw

img = Image.open("assets/icon.png").convert("RGBA")
w, h = img.size

# In assets/icon.png (738x738), the inner squircle center is ~ (369, 385)
# Let's inspect along horizontal line y=385
# Outer dark background is R<20, G<20, B<20.
# Inner squircle surface is around R=40..50, G=42..54, B=46..58.

cy = 385
cx = 369

# Scan left from cx
inner_left = None
for x in range(cx - 100, 50, -1):
    r, g, b, _ = img.getpixel((x, cy))
    # Check when color drops to outer board (R < 25)
    if r < 24 and g < 24 and b < 26:
        inner_left = x + 3  # step inside edge
        break

# Scan right from cx
inner_right = None
for x in range(cx + 100, w - 50):
    r, g, b, _ = img.getpixel((x, cy))
    if r < 24 and g < 24 and b < 26:
        inner_right = x - 3
        break

# Scan up from cy
inner_top = None
for y in range(cy - 100, 50, -1):
    r, g, b, _ = img.getpixel((cx, y))
    if r < 24 and g < 24 and b < 26:
        inner_top = y + 3
        break

# Scan down from cy
inner_bottom = None
for y in range(cy + 100, h - 50):
    r, g, b, _ = img.getpixel((cx, y))
    if r < 24 and g < 24 and b < 26:
        inner_bottom = y - 3
        break

print(f"Inner tile bounds: left={inner_left}, top={inner_top}, right={inner_right}, bottom={inner_bottom}")

if inner_left and inner_right and inner_top and inner_bottom:
    tile_w = inner_right - inner_left
    tile_h = inner_bottom - inner_top
    size = min(tile_w, tile_h)
    
    # Recenter square
    center_x = (inner_left + inner_right) // 2
    center_y = (inner_top + inner_bottom) // 2
    
    box = (center_x - size // 2, center_y - size // 2, center_x + size // 2, center_y + size // 2)
    clean_tile = img.crop(box)
    tw, th = clean_tile.size
    
    # Apply super smooth rounded squircle mask
    corner_radius = int(size * 0.22)
    scale = 4
    mask_large = Image.new("L", (tw * scale, th * scale), 0)
    draw = ImageDraw.Draw(mask_large)
    draw.rounded_rectangle([(0, 0), (tw * scale - 1, th * scale - 1)], radius=corner_radius * scale, fill=255)
    mask = mask_large.resize((tw, th), Image.Resampling.LANCZOS)
    clean_tile.putalpha(mask)
    
    png_path = os.path.join("assets", "icon.png")
    clean_tile.save(png_path, "PNG")
    
    # Save standard Windows ICO
    ico_path = os.path.join("assets", "app_icon.ico")
    icon_sizes = [(256, 256), (128, 128), (64, 64), (48, 48), (32, 32), (24, 24), (16, 16)]
    clean_tile.save(ico_path, format="ICO", sizes=icon_sizes)
    print(f"Pristine icon generated! Size: {tw}x{th}")
