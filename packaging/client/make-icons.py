#!/usr/bin/env python3
"""Generate the Kinseat mark at reproducible sizes (Pillow required)."""
from pathlib import Path
from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parents[2] / 'app' / 'kinseat-client' / 'assets'
OUT.mkdir(parents=True, exist_ok=True)
size = 1024
image = Image.new('RGBA', (size, size), '#151413')
d = ImageDraw.Draw(image)
d.rounded_rectangle((157, 170, 427, 854), radius=115, fill='#E8A33D')
d.rounded_rectangle((597, 170, 867, 854), radius=115, fill='#EDE8E1')
icons = []
for n in (16, 24, 32, 48, 64, 128, 256):
    icon = image.resize((n, n), Image.Resampling.LANCZOS)
    icon.save(OUT / f'kinseat-{n}.png', optimize=True)
    icons.append(icon)
image.save(OUT / 'kinseat.ico', format='ICO', sizes=[(n, n) for n in (16, 24, 32, 48, 64, 128, 256)])
print(OUT)
