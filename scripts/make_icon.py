#!/usr/bin/env python3
"""生成鸿蒙应用图标（1024x1024 RGBA）。

品牌来源：上游 `gridcraft/assets/app-icon/` 的 gridcraft 标志
（`gridcraft-1024.png` 及 `hicolor/512x512/apps/ai.storyteller.gridcraft.png`）。

两种风格：
  blue  （默认，用于应用图标）品牌蓝渐变底 + 白化标志，保留等距三面的明暗层次。
        启动器里图标只有 48~160px，浅底方案在这个尺寸会发灰，蓝底的高对比更容易辨认。
  light （备选，最贴近上游 macOS 图标）浅色底 + 极淡等距网格 + 原色标志。

几何沿用 photocraft 那张图标的实测参数：整幅 1024、圆角半径 210。鸿蒙启动器是
按图片实际形状显示的（不额外裁切），所以圆角必须自己烘进图片。

用法：
    python scripts/make_icon.py                 # 默认 copy：直接落地 gridcraft-1024.png 成品图标
    python scripts/make_icon.py --style light out.png
"""
import os
import sys

from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
LOGO = os.path.join(ROOT, "..", "gridcraft", "assets", "app-icon", "gridcraft-1024.png")
TARGETS = [
    os.path.join(ROOT, "AppScope", "resources", "base", "media", "app_icon.png"),
    os.path.join(ROOT, "entry", "src", "main", "resources", "base", "media", "app_icon.png"),
]

SIZE = 1024
RADIUS = 210
SS = 4  # 超采样倍数，保证圆角与线条平滑
MARK_SCALE = 0.78

BLUE_TOP = (64, 142, 238)
BLUE_BOTTOM = (20, 72, 158)
LIGHT_TOP = (250, 252, 255)
LIGHT_BOTTOM = (223, 233, 249)
GRID = (80, 126, 170, 46)  # 与上游 02-grid.svg 的 #507eaa 同色，透明度压低


def rounded_mask(size: int, radius: int) -> Image.Image:
    big = Image.new("L", (size * SS, size * SS), 0)
    ImageDraw.Draw(big).rounded_rectangle(
        (0, 0, size * SS - 1, size * SS - 1), radius=radius * SS, fill=255
    )
    return big.resize((size, size), Image.LANCZOS)


def gradient(size: int, top: tuple, bottom: tuple) -> Image.Image:
    strip = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / (size - 1)
        strip.putpixel((0, y), tuple(round(top[i] + (bottom[i] - top[i]) * t) for i in range(3)))
    return strip.resize((size, size), Image.BICUBIC).convert("RGBA")


def logo_layer(size: int) -> Image.Image:
    side = round(size * MARK_SCALE)
    logo = Image.open(LOGO).convert("RGBA").resize((side, side), Image.LANCZOS)
    layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    off = (size - side) // 2
    layer.alpha_composite(logo, (off, off))
    return layer


def whiten(layer: Image.Image) -> Image.Image:
    """把标志重染成白→浅蓝的等距三面，保留面与面的明暗差别（否则 S 会糊成一片）。"""
    lum = layer.convert("L")
    alpha = layer.split()[3]
    # 亮度映射到 0.78..1.0 的白度
    grey = lum.point(lambda v: round(199 + 56 * v / 255))
    out = Image.merge("RGB", (grey, grey, grey.point(lambda v: min(255, round(v * 1.02)))))
    out = out.convert("RGBA")
    out.putalpha(alpha)
    return out


def build_blue() -> Image.Image:
    canvas = gradient(SIZE, BLUE_TOP, BLUE_BOTTOM)
    canvas.alpha_composite(whiten(logo_layer(SIZE)))
    canvas.putalpha(rounded_mask(SIZE, RADIUS))
    return canvas


def build_light() -> Image.Image:
    canvas = gradient(SIZE, LIGHT_TOP, LIGHT_BOTTOM)
    # 等距网格：45° 双斜线，中心向外淡出（对应 02-grid.svg 的 radial 渐变遮罩）
    grid = Image.new("RGBA", (SIZE * SS, SIZE * SS), (0, 0, 0, 0))
    draw = ImageDraw.Draw(grid)
    step, span = 114 * SS, SIZE * SS
    for offset in range(-span, span * 2, step):
        draw.line([(offset, 0), (offset + span, span)], fill=GRID, width=3 * SS)
        draw.line([(offset, 0), (offset - span, span)], fill=GRID, width=3 * SS)
    grid = grid.resize((SIZE, SIZE), Image.LANCZOS)
    fade = Image.new("L", (SIZE, SIZE), 0)
    fd = ImageDraw.Draw(fade)
    for r in range(SIZE // 2, 0, -1):
        alpha = max(0, round(255 * (1 - r / (SIZE * 0.52)) ** 1.2))
        fd.ellipse((SIZE // 2 - r, SIZE // 2 - r, SIZE // 2 + r, SIZE // 2 + r), fill=alpha)
    r, g, b, a = grid.split()
    grid.putalpha(Image.composite(a, Image.new("L", (SIZE, SIZE), 0), fade))
    canvas.alpha_composite(grid)
    canvas.alpha_composite(logo_layer(SIZE))
    canvas.putalpha(rounded_mask(SIZE, RADIUS))
    return canvas


def build_copy() -> Image.Image:
    # gridcraft 上游的 gridcraft-1024.png 已是完整圆角图标（蓝底 + 表格图形），
    # 直接作为鸿蒙应用图标落地，无需再合成底色或重染标志。
    return Image.open(LOGO).convert("RGBA").resize((SIZE, SIZE), Image.LANCZOS)


BUILDERS = {"copy": build_copy, "blue": build_blue, "light": build_light}


def main(argv: list) -> int:
    style = "copy"
    paths = list(TARGETS)
    args = list(argv)
    if "--style" in args:
        i = args.index("--style")
        style = args[i + 1]
        del args[i:i + 2]
    if args:
        paths = [os.path.abspath(args[0])]
    if style not in BUILDERS:
        print(f"unknown style {style!r}; pick one of {sorted(BUILDERS)}", file=sys.stderr)
        return 2
    if not os.path.exists(LOGO):
        print(f"missing upstream logo: {LOGO}", file=sys.stderr)
        return 1
    icon = BUILDERS[style]()
    for path in paths:
        icon.save(path, "PNG", optimize=True)
        print(f"wrote {path} {icon.size} {icon.mode} style={style}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
