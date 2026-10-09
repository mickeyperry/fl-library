"""Generate the ten coloured folder icons (same palette as the panel) into icons\\folder_N.ico."""
import os

from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
PALETTE = [
    (255, 179, 128), (128, 222, 160), (140, 190, 255), (240, 160, 255), (255, 230, 120),
    (130, 230, 230), (255, 150, 170), (190, 255, 140), (255, 200, 90), (180, 170, 255),
]


def folder(size, rgb):
    s = size
    img = Image.new('RGBA', (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    dark = tuple(int(c * 0.62) for c in rgb)
    r = max(1, s // 16)
    # back + tab
    d.rounded_rectangle((s * 0.06, s * 0.18, s * 0.94, s * 0.84), radius=r, fill=dark)
    d.rounded_rectangle((s * 0.06, s * 0.12, s * 0.42, s * 0.30), radius=r, fill=dark)
    # front
    d.rounded_rectangle((s * 0.06, s * 0.34, s * 0.94, s * 0.88), radius=r, fill=rgb)
    return img


def main():
    out = os.path.join(HERE, 'icons')
    os.makedirs(out, exist_ok=True)
    for i, rgb in enumerate(PALETTE):
        big = folder(256, rgb)
        big.save(os.path.join(out, f'folder_{i}.ico'), sizes=[(256, 256), (64, 64), (48, 48), (32, 32), (16, 16)])
    folder(256, (255, 140, 0)).save(os.path.join(out, 'fl_on.ico'), sizes=[(64, 64), (32, 32), (16, 16)])
    folder(256, (120, 120, 128)).save(os.path.join(out, 'fl_off.ico'), sizes=[(64, 64), (32, 32), (16, 16)])
    print('wrote', len(PALETTE) + 2, 'icons to', out)


if __name__ == '__main__':
    main()
