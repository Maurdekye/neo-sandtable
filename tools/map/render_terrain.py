"""Our original flat-fill map art, built from data only (no source images)."""
import math
from pathlib import Path

# Deliberately new graphic colors; none are sampled from the source artwork.
COLORS = {"unclassified": "#eceff3", "clear": "#ead8ab", "rough": "#a77c57",
          "gravel": "#bdada3", "salt_marsh": "#c4aab7", "heavy_vegetation": "#63865b",
          "mountain": "#6f6256", "delta": "#86a79d", "desert": "#e6b24b",
          "major_city": "#7b759b", "swamp": "#849977", "sea": "#a9d3e9",
          "village_bir_oasis": "#ead8ab"}


def render(rows, path: Path):
    radius = 10
    centers = [(math.sqrt(3) * radius * (int(h["q"]) + int(h["r"]) / 2),
                1.5 * radius * int(h["r"])) for h in rows]
    xmin, xmax = min(x for x, y in centers)-12, max(x for x, y in centers)+12
    ymin, ymax = min(y for x, y in centers)-12, max(y for x, y in centers)+12
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{xmin:.2f} {ymin:.2f} {xmax-xmin:.2f} {ymax-ymin:.2f}">',
           '<title>CNA map: reviewed terrain, incomplete</title>',
           '<desc>Regular hexagons generated solely from published data. Gray cells are unclassified. No source artwork.</desc>']
    for h, (x, y) in zip(rows, centers):
        points = " ".join(f'{x+radius*math.cos(math.radians(a)):.2f},{y+radius*math.sin(math.radians(a)):.2f}'
                          for a in [30, 90, 150, 210, 270, 330])
        color = COLORS[h["terrain"]]
        out.append(f'<polygon points="{points}" fill="{color}" stroke="#657281" stroke-width="0.35"><title>{h["hex_id"]}: {h["terrain"]}</title></polygon>')
    out.append("</svg>")
    path.write_text("\n".join(out)+"\n", encoding="utf-8")
