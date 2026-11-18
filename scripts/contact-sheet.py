"""Builds a labelled contact sheet from the corpus renders.

    python scripts/contact-sheet.py .tmp-render

Reads every PNG in the directory (except the sheet itself), writes
`<dir>/corpus.png`. Kept dependency-free apart from Pillow so it also works with
any Python that has it installed.
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

COLUMNS = 4
PADDING = 12
LABEL_HEIGHT = 22


def font(size: int) -> ImageFont.ImageFont:
    for candidate in ("segoeui.ttf", "arial.ttf", "DejaVuSans.ttf"):
        try:
            return ImageFont.truetype(candidate, size)
        except OSError:
            continue
    return ImageFont.load_default()


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2

    directory = Path(sys.argv[1])
    paths = sorted(p for p in directory.glob("*.png") if p.name != "corpus.png")

    if not paths:
        print(f"no PNGs in {directory}")
        return 1

    images = [(path.stem, Image.open(path).convert("RGB")) for path in paths]
    cell_width = max(image.width for _, image in images)
    cell_height = max(image.height for _, image in images) + LABEL_HEIGHT
    rows = (len(images) + COLUMNS - 1) // COLUMNS

    sheet = Image.new(
        "RGB",
        (
            COLUMNS * (cell_width + PADDING) + PADDING,
            rows * (cell_height + PADDING) + PADDING,
        ),
        (255, 255, 255),
    )
    draw = ImageDraw.Draw(sheet)
    label_font = font(14)

    for index, (name, image) in enumerate(images):
        column = index % COLUMNS
        row = index // COLUMNS
        left = PADDING + column * (cell_width + PADDING)
        top = PADDING + row * (cell_height + PADDING)

        sheet.paste(image, (left, top + LABEL_HEIGHT))
        draw.text((left, top), name, fill=(0, 0, 0), font=label_font)

    target = directory / "corpus.png"
    sheet.save(target)
    print(f"wrote {target} ({sheet.width}x{sheet.height}, {len(images)} models)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
