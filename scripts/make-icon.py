"""Generates assets/logo.png (1024x1024) from the accent colour in branding.json.

Run `npx tauri icon assets/logo.png` afterwards to regenerate every platform icon.
"""
import json
import pathlib

from PIL import Image, ImageDraw, ImageFilter

root = pathlib.Path(__file__).resolve().parent.parent
brand = json.loads((root / "branding.json").read_text(encoding="utf-8"))
accent = brand["colors"]["accent"].lstrip("#")
base = tuple(int(accent[i : i + 2], 16) for i in (0, 2, 4))
S = 1024

def gradient() -> Image.Image:
    img = Image.new("RGBA", (S, S))
    px = img.load()
    for y in range(S):
        for x in range(S):
            t = (x + y) / (2 * S)
            px[x, y] = tuple(int(c * (1.08 - 0.42 * t)) for c in base) + (255,)
    return img

bg = gradient()
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).rounded_rectangle((32, 32, S - 32, S - 32), radius=230, fill=255)
icon = Image.new("RGBA", (S, S), (0, 0, 0, 0))
icon.paste(bg, (0, 0), mask)

# Monitor with a pointer: reads as "remote desktop" at every size.
d = ImageDraw.Draw(icon)
d.rounded_rectangle((212, 262, 812, 632), radius=46, fill=(255, 255, 255, 255))
d.rounded_rectangle((252, 302, 772, 592), radius=22, fill=tuple(int(c * 0.55) for c in base) + (255,))
d.rounded_rectangle((452, 632, 572, 712), radius=12, fill=(255, 255, 255, 235))
d.rounded_rectangle((372, 702, 652, 742), radius=20, fill=(255, 255, 255, 255))
pointer = [(520, 380), (520, 590), (575, 540), (622, 640), (668, 618), (622, 520), (700, 520)]
shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
ImageDraw.Draw(shadow).polygon([(x + 8, y + 12) for x, y in pointer], fill=(0, 0, 0, 90))
icon = Image.alpha_composite(icon, shadow.filter(ImageFilter.GaussianBlur(8)))
ImageDraw.Draw(icon).polygon(pointer, fill=(255, 255, 255, 255), outline=(20, 30, 50, 255))
(root / "assets").mkdir(exist_ok=True)
icon.save(root / "assets" / "logo.png")
print("wrote assets/logo.png")
