
import os
from PIL import Image

src_png = "assets/icon.png"
img = Image.open(src_png).convert("RGBA")

# 1. Save 256x256 standard PNG
img_256 = img.resize((256, 256), Image.Resampling.LANCZOS)
img_256.save("assets/icon_256.png", "PNG")

# 2. Save 64x64 standard PNG
img_64 = img.resize((64, 64), Image.Resampling.LANCZOS)
img_64.save("assets/icon_64.png", "PNG")

# 3. Export raw RGBA binary bytes for egui::IconData (64x64 * 4 = 16384 bytes)
rgba_bytes = img_64.tobytes()
with open("assets/icon_64.rgba", "wb") as f:
    f.write(rgba_bytes)

# 4. Also export 32x32 RGBA binary bytes (32x32 * 4 = 4096 bytes)
img_32 = img.resize((32, 32), Image.Resampling.LANCZOS)
img_32.save("assets/icon_32.png", "PNG")
with open("assets/icon_32.rgba", "wb") as f:
    f.write(img_32.tobytes())

print(f"Generated: icon_256.png, icon_64.png, icon_32.png, icon_64.rgba ({len(rgba_bytes)} bytes)")
