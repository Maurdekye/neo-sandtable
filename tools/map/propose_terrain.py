"""Create local-only palette proposals; never auto-publish raster classifications.

Requires Pillow only when used on local images. Numeric palette statistics are
source measurements, not map artwork. Contours, labels and tiny shoreline
fragments deliberately cause abstentions or quarantined proposals.
"""
from __future__ import annotations
import argparse
from collections import Counter
import csv
import os
from pathlib import Path
from generate_grid import extract, IMAGE

PALETTE = {"clear": (251, 250, 239), "rough": (194, 185, 149),
           "mountain": (160, 146, 80), "sea": (138, 181, 207)}
ALGORITHM = "palette-v0.1"


def decide(fractions, rough_inner, mountain_inner):
    sea = fractions["sea"]
    land = sum(fractions[k] for k in ["clear", "rough", "mountain"])
    if sea > .92 and land < .01:
        return "sea"
    if rough_inner > .035:
        return "rough"
    if mountain_inner > .015:
        return "unclassified"
    if land > .25:
        return "clear"
    return "unclassified"


def measure(image, h):
    counts = Counter()
    for yy in range(-43, 44):
        maxx = 38 if abs(yy) <= 22 else 38 * (43 - abs(yy)) / 21
        for xx in range(-38, 39):
            if abs(xx) <= maxx:
                counts[image.getpixel((h.x + xx, h.y + yy))] += 1
    total = sum(counts.values())
    fractions = {k: counts[v] / total for k, v in PALETTE.items()}
    inner = Counter(image.crop((h.x-24, h.y-24, h.x+25, h.y+25)).getdata())
    rough_inner = inner[PALETTE["rough"]] / 2401
    mountain_inner = inner[PALETTE["mountain"]] / 2401
    proposed = decide(fractions, rough_inner, mountain_inner)
    return dict(hex_id=h.hex_id, proposed=proposed,
                **{k: round(v, 4) for k, v in fractions.items()},
                rough_inner=round(rough_inner, 4), mountain_inner=round(mountain_inner, 4))


def propose(sources, output, section, first_range, second_range):
    from PIL import Image, ImageDraw
    output = output.resolve()
    if output.is_relative_to(Path(__file__).resolve().parents[2]) or output.is_relative_to(sources.resolve()):
        raise ValueError("Raster inspection output must be outside repository and sources")
    records, *_ = extract(sources)
    selected = sorted([h for h in records if h.hex_id[0] == section
                       and first_range[0] <= h.first <= first_range[1]
                       and second_range[0] <= h.second <= second_range[1]],
                      key=lambda h: (-h.first, h.second))
    if not selected:
        raise ValueError("Empty selection")
    image = Image.open(sources / "vassal/extracted/images" / IMAGE).convert("RGB")
    rows = [measure(image, h) for h in selected]
    output.mkdir(parents=True, exist_ok=True)
    with (output / "proposals.csv").open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, rows[0], lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)
    # Each crop is larger than its hex; reviewers inspect the drawn cell boundary.
    width, height, columns = 108, 128, 10
    sheet = Image.new("RGB", (width * columns, height * ((len(rows)+columns-1)//columns)), "white")
    for index, h in enumerate(selected):
        crop = image.crop((h.x-54, h.y-54, h.x+54, h.y+54))
        cell = Image.new("RGB", (width, height), "white")
        cell.paste(crop, (0, 20))
        draw = ImageDraw.Draw(cell)
        draw.text((4, 4), h.hex_id, fill="black")
        draw.ellipse((52, 72, 56, 76), outline="#ff00ff")
        sheet.paste(cell, (index % columns * width, index // columns * height))
    sheet.save(output / "contact-sheet.png")
    print(f"{len(rows)} proposals: {dict(Counter(row['proposed'] for row in rows))}. No data published.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--section", choices=list("ABCDE"), required=True)
    parser.add_argument("--first", type=int, nargs=2, required=True, metavar=("MIN", "MAX"))
    parser.add_argument("--second", type=int, nargs=2, required=True, metavar=("MIN", "MAX"))
    args = parser.parse_args()
    if not os.environ.get("CNA_SOURCES"):
        parser.error("Set CNA_SOURCES")
    propose(Path(os.environ["CNA_SOURCES"]), args.output, args.section, args.first, args.second)
