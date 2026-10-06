"""Local-only source-image inspection; requires Pillow, outputs outside repo."""
import argparse
import os
from pathlib import Path
from PIL import Image, ImageDraw
from generate_grid import extract, IMAGE

LANDMARKS = ["A1816", "A2021", "A2629", "A4130", "A4829", "A4827", "B5504",
             "B5925", "B4921", "C4807", "C4321", "C4021", "C4131", "C4218",
             "C4120", "C1014", "C0127", "D3714", "E3613", "E3714", "E1430", "A2109", "E3815", "E4019", "B5810", "B5809"]


def inspect(sources: Path, output: Path):
    output = output.resolve()
    repo = Path(__file__).resolve().parents[2]
    if output.is_relative_to(repo) or output.is_relative_to(sources.resolve()):
        raise ValueError("Source-image outputs must be outside repository and sources")
    records, _, _, _, _ = extract(sources)
    by_id = {h.hex_id: h for h in records}
    image = Image.open(sources / "vassal/extracted/images" / IMAGE)
    width = 360
    sheet = Image.new("RGB", (width * 7, width * ((len(LANDMARKS) + 6) // 7)), "white")
    for idx, name in enumerate(LANDMARKS):
        h = by_id[name]
        crop = image.crop((h.x-180, h.y-180, h.x+180, h.y+180)).convert("RGB")
        draw = ImageDraw.Draw(crop)
        draw.ellipse((174, 174, 186, 186), outline="#ff00ff", width=3)
        draw.rectangle((0, 0, 120, 20), fill="white")
        draw.text((4, 4), name, fill="black")
        sheet.paste(crop, ((idx % 7) * width, (idx // 7) * width))
    output.mkdir(parents=True, exist_ok=True)
    sheet.save(output / "landmarks.png")
    seams = Image.new("RGB", (800 * 4, 500 * 2), "white")
    for idx, x in enumerate([2863, 5680, 8498, 11314]):
        for row, y in enumerate([2500, 4200]):
            crop = image.crop((x-400, y-250, x+400, y+250)).convert("RGB")
            draw = ImageDraw.Draw(crop)
            for h in records:
                if x-400 < h.x < x+400 and y-250 < h.y < y+250:
                    xx, yy = h.x-x+400, h.y-y+250
                    draw.ellipse((xx-2, yy-2, xx+2, yy+2), fill="#ff00ff")
                    draw.text((xx-20, yy-18), h.hex_id, fill="#cc00cc")
            seams.paste(crop, (idx * 800, row * 500))
    seams.save(output / "seams.png")
    print(f"Local inspection images written to {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not os.environ.get("CNA_SOURCES"):
        parser.error("Set CNA_SOURCES")
    inspect(Path(os.environ["CNA_SOURCES"]), args.output)
