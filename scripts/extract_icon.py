
import os
import sys
from PIL import Image, ImageDraw

src_path = "f3_brand_icons_minimalist.png"
if not os.path.exists(src_path):
    print("Source image not found: " + src_path)
    sys.exit(1)

img = Image.open(src_path).convert("RGBA")
width, height = img.size
print(f"Loaded image {width}x{height}")

# Let's inspect the top-left quadrant [0:1024, 0:1024]
# Find the squircle tile bounding box.
# The background of the presentation board is ~#121316 (RGB approx 18, 19, 22).
# The squircle tile background is darker grey/charcoal, but distinctly lighter than board, around 35-50.
# The 'F' is stark white (250, 250, 250) and '3' is cyan (0, 200, 240).

# Let's find bounding box by scanning inside the top-left quadrant (x: 100..900, y: 150..950)
# First find the center of mass of the bright symbol (white/cyan pixels where R>150 or G>180 or B>180)
bright_pixels = []
for y in range(200, 900, 4):
    for x in range(150, 850, 4):
        r, g, b, a = img.getpixel((x, y))
        if (r > 180 and g > 180 and b > 180) or (b > 180 and g > 150):
            bright_pixels.append((x, y))

if not bright_pixels:
    print("Could not find bright symbol pixels!")
    sys.exit(1)

min_bx = min(p[0] for p in bright_pixels)
max_bx = max(p[0] for p in bright_pixels)
min_by = min(p[1] for p in bright_pixels)
max_by = max(p[1] for p in bright_pixels)

center_x = (min_bx + max_bx) // 2
center_y = (min_by + max_by) // 2
symbol_w = max_bx - min_bx
symbol_h = max_by - min_by

print(f"Symbol center: ({center_x}, {center_y}), bounding box: {symbol_w}x{symbol_h} ([{min_bx}, {min_by}] to [{max_bx}, {max_by}])")

# Now from center_x, center_y, find the squircle tile boundaries in all 4 directions
# Moving left from center_x at y=center_y:
tile_left = None
for x in range(center_x, 50, -1):
    r, g, b, _ = img.getpixel((x, center_y))
    # Outer board is ~ (18, 19, 22), tile edge drops to outer board
    if r < 24 and g < 25 and b < 28:
        tile_left = x
        break

# Moving right from center_x at y=center_y:
tile_right = None
for x in range(center_x, 1000):
    r, g, b, _ = img.getpixel((x, center_y))
    if r < 24 and g < 25 and b < 28:
        tile_right = x
        break

# Moving up from center_y at x=center_x:
tile_top = None
for y in range(center_y, 50, -1):
    r, g, b, _ = img.getpixel((center_x, y))
    if r < 24 and g < 25 and b < 28:
        tile_top = y
        break

# Moving down from center_y at x=center_x:
tile_bottom = None
for y in range(center_y, 1000):
    r, g, b, _ = img.getpixel((center_x, y))
    if r < 24 and g < 25 and b < 28:
        tile_bottom = y
        break

print(f"Detected tile boundaries: left={tile_left}, top={tile_top}, right={tile_right}, bottom={tile_bottom}")

# If we have clean boundaries:
if tile_left and tile_right and tile_top and tile_bottom:
    w = tile_right - tile_left
    h = tile_bottom - tile_top
    print(f"Tile size: {w}x{h}")
    
    # Make it square by using the max dimension and centering
    size = max(w, h)
    cx = (tile_left + tile_right) // 2
    cy = (tile_top + tile_bottom) // 2
    
    box = (cx - size // 2, cy - size // 2, cx + size // 2, cy + size // 2)
    tile = img.crop(box)
    tw, th = tile.size
    
    # Create smooth rounded squircle mask with anti-aliased rounded corners
    # Corner radius approx 22.5% of tile size (Apple/Win11 squircle standard)
    corner_radius = int(size * 0.22)
    
    # Supersampled mask for silky smooth anti-aliased corners
    scale = 4
    mask_large = Image.new("L", (tw * scale, th * scale), 0)
    draw = ImageDraw.Draw(mask_large)
    draw.rounded_rectangle([(0, 0), (tw * scale - 1, th * scale - 1)], radius=corner_radius * scale, fill=255)
    mask = mask_large.resize((tw, th), Image.Resampling.LANCZOS)
    
    tile.putalpha(mask)
    
    os.makedirs("assets", exist_ok=True)
    png_path = os.path.join("assets", "icon.png")
    tile.save(png_path, "PNG")
    print(f"Successfully saved {png_path} ({tw}x{th})")
    
    # Also save standard ICO
    ico_path = os.path.join("assets", "app_icon.ico")
    icon_sizes = [(256, 256), (128, 128), (64, 64), (48, 48), (32, 32), (24, 24), (16, 16)]
    tile.save(ico_path, format="ICO", sizes=icon_sizes)
    print(f"Successfully saved {ico_path} with sizes: {icon_sizes}")
else:
    print("Could not detect tile cleanly!")
    sys.exit(1)
