
import os
from PIL import Image

img = Image.open("assets/icon_new.png").convert("RGBA")
w, h = img.size

# Let's inspect along center column x=512 from y=0 to y=120
print("Pixel inspection along x=512:")
for y in range(50, 105):
    print(f"y={y}: {img.getpixel((512, y))}")
