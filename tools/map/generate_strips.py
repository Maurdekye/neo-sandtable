"""Build explicit route-completeness notes from source-reviewed layer data.

A complete map strip is not a verdict on unit allowance, fuel, control,
weather or game-action legality. No image-derived art is emitted.
"""
import json
import math
from pathlib import Path
import tomllib
from xml.sax.saxutils import escape
from layers import Layers, SIDE_KINDS
from render_terrain import COLORS

MOVEMENT_LINES = {"road", "track", "railroad", "unfinished_road", "unfinished_railroad"}


def generate(folder):
    layers = Layers(folder)
    strips = []
    for path in sorted((folder / "edge-reviews").glob("*.toml")):
        batch = tomllib.loads(path.read_text(encoding="utf-8"))["batch"]
        route = batch.get("route_hex_ids")
        if route is None:
            continue
        if (batch["source_image_sha256"] != layers.metadata["source_image_sha256"] or
                batch["build_file_sha256"] != layers.grid.metadata["build_file_sha256"]):
            raise ValueError("Strip review/layer source identity differs")
        if len(route) < 2 or len(set(route)) != len(route):
            raise ValueError("Strip route must contain distinct adjacent cells")
        for name in route:
            if layers.grid.canonical(name) != name:
                raise ValueError("Strip route needs canonical cells")
            for kind in ("terrain", "coastal"):
                if (kind, name, "") not in layers.coverage:
                    raise ValueError("Strip surface remains unknown")
        pairs = []
        for a,b in zip(route, route[1:]):
            # feature() also validates adjacency and canonicalizes query order.
            for kind in MOVEMENT_LINES:
                layers.feature("line", kind, a,b)
            for kind in SIDE_KINDS:
                layers.feature("side", kind, a,b)
            pairs.append(sorted((a,b)))
        strips.append(dict(id=batch["id"], coordinate_profile="vassal-2021",
                           source_image_sha256=batch["source_image_sha256"],
                           build_file_sha256=batch["build_file_sha256"],
                           review_batch=batch["id"], route_hex_ids=route,
                           surveyed_route_edges=pairs,
                           complete_line_kinds=sorted(MOVEMENT_LINES),
                           complete_side_kinds=sorted(SIDE_KINDS),
                           route_map_layers_complete=True,
                           control_halo_complete=False, pipeline_complete=False,
                           unit_action_legality_verified=False,
                           src=["land:8.33", "land:8.35", "land:8.37", "land:10.21"]))
    text = ['schema_version = 1', 'complete_map = false']
    for strip in strips:
        text += ["", "[[strips]]"] + [f"{k} = {json.dumps(v)}" for k,v in strip.items()]
    (folder / "strips.toml").write_text("\n".join(text)+"\n", encoding="utf-8", newline="\n")
    for strip in strips:
        render_strip(layers, strip, folder / (strip["id"] + "-preview.svg"))
    return strips


def render_strip(layers, strip, path):
    route = strip["route_hex_ids"]
    radius = 28
    centers = {}
    for name in route:
        h = layers.grid.hexes[name]
        centers[name] = (math.sqrt(3)*radius*(int(h["q"])+int(h["r"])/2),
                         1.5*radius*int(h["r"]))
    xs, ys = zip(*centers.values())
    xmin, ymin = min(xs)-radius-8, min(ys)-radius-8
    width, height = max(xs)-xmin+radius+8, max(ys)-ymin+radius+8
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{xmin:.2f} {ymin:.2f} {width:.2f} {height:.2f}">',
           '<title>Source-reviewed map strip: map-layer completeness only</title>',
           '<desc>Original regular hexagons and center links generated from canonical map data. No source artwork. Unit move legality and the neighboring control halo are unverified.</desc>']
    for name,(x,y) in centers.items():
        points = " ".join(f"{x+radius*math.cos(math.radians(a)):.2f},{y+radius*math.sin(math.radians(a)):.2f}"
                          for a in (30,90,150,210,270,330))
        color = COLORS[layers.grid.hexes[name]["terrain"]]
        out.append(f'<polygon points="{points}" fill="{color}" stroke="#637683" stroke-width="1"/>')
    styles = {"road": ("#84522c", ""), "track": ("#676b70", "2 3"),
              "unfinished_road": ("#84522c", "4 3"), "railroad": ("#437e91", ""),
              "unfinished_railroad": ("#437e91", "2 3"), "plain": ("#8f98a0", "1 4")}
    for a,b in zip(route, route[1:]):
        kind = next((k for k in ("road", "track", "unfinished_road", "railroad", "unfinished_railroad")
                     if layers.feature("line", k, a,b) is not None), "plain")
        color, dash = styles[kind]
        x1,y1 = centers[a]
        x2,y2 = centers[b]
        out.append(f'<line x1="{x1:.2f}" y1="{y1:.2f}" x2="{x2:.2f}" y2="{y2:.2f}" stroke="{color}" stroke-width="2.5" stroke-dasharray="{dash}"><title>{kind}</title></line>')
    for a,b in zip(route, route[1:]):
        x1,y1 = centers[a]
        x2,y2 = centers[b]
        mx,my = (x1+x2)/2,(y1+y2)/2
        length = math.hypot(x2-x1,y2-y1)
        tx,ty = -(y2-y1)/length,(x2-x1)/length
        for kind in sorted(SIDE_KINDS):
            feature = layers.feature("side",kind,a,b)
            if feature is None:
                continue
            label = kind + ("; high " + feature["high_side"] if feature["high_side"] else "")
            out.append(f'<line x1="{mx-tx*radius/2:.2f}" y1="{my-ty*radius/2:.2f}" x2="{mx+tx*radius/2:.2f}" y2="{my+ty*radius/2:.2f}" stroke="#b63831" stroke-width="3"><title>{escape(label)}</title></line>')
            out.append(f'<text x="{mx:.2f}" y="{my-20:.2f}" text-anchor="middle" font-family="sans-serif" font-size="5" fill="#9d2521">{escape(label)}</text>')
    for name,(x,y) in centers.items():
        out.append(f'<text x="{x:.2f}" y="{y+15:.2f}" text-anchor="middle" font-family="sans-serif" font-size="8">{escape(name)}</text>')
    path.write_text("\n".join(out)+"\n</svg>\n", encoding="utf-8", newline="\n")

if __name__ == "__main__":
    generate(Path(__file__).resolve().parents[2] / "data/map")
