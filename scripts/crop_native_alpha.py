
import os
from PIL import Image

src_path = "assets/icon_new.png"
img = Image.open(src_path).convert("RGBA")
w, h = img.size

# Extract alpha channel
alpha = img.getchannel("A")
bbox = alpha.getbbox()
print(f"Native non-transparent bounding box from alpha channel: {bbox}")

if bbox:
    min_x, min_y, max_x, max_y = bbox
    bw = max_x - min_x
    bh = max_y - min_y
    print(f"Native content size: {bw}x{bh}")

    # Make it an exact symmetrical square
    size = max(bw, bh)
    cx = (min_x + max_x) // 2
    cy = (min_y + max_y) // 2

    half = size // 2
    # Ensure square crop fits inside image bounds with smooth 1:1 ratio
    crop_box = (cx - half, cy - half, cx + half, cy + half)
    cropped = img.crop(crop_box)
    cw, ch = cropped.size
    print(f"Symmetrically cropped to square: {cw}x{ch}")

    # 1. Save master icon.png using the model's pure native anti-aliased transparency
    master_png = "assets/icon.png"
    cropped.save(master_png, "PNG")
    print(f"Saved pristine native master: {master_png}")

    # 2. Save 256x256
    img_256 = cropped.resize((256, 256), Image.Resampling.LANCZOS)
    img_256.save("assets/icon_256.png", "PNG")

    # 3. Save 64x64 and export RGBA matrix
    img_64 = cropped.resize((64, 64), Image.Resampling.LANCZOS)
    img_64.save("assets/icon_64.png", "PNG")
    with open("assets/icon_64.rgba", "wb") as f:
        f.write(img_64.tobytes())

    # 4. Save 32x32 and export RGBA matrix
    img_32 = cropped.resize((32, 32), Image.Resampling.LANCZOS)
    img_32.save("assets/icon_32.png", "PNG")
    with open("assets/icon_32.rgba", "wb") as f:
        f.write(img_32.tobytes())

    # 5. Generate Windows multi-size ICO
    ico_path = "assets/app_icon.ico"
    icon_sizes = [(256, 256), (128, 128), (64, 64), (48, 48), (32, 32), (24, 24), (16, 16)]
    cropped.save(ico_path, format="ICO", sizes=icon_sizes)

    print("All assets updated with 100% native anti-aliased transparency!")
