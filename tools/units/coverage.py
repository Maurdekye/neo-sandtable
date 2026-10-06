#!/usr/bin/env python3
"""Report which 'D' (deployed-at-start) OA units are placed by the Graziani set-up files, and which are not.
Usage: python tools/units/coverage.py [repo_root]   (needs Python >= 3.11)"""
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[2]
units, children, sheet_units = {}, defaultdict(list), defaultdict(list)
for f in sorted((root / "data/units/oa").glob("*/*.toml")):
    d = tomllib.load(open(f, "rb"))
    sid = d["sheet"]["id"]
    for u in d["unit"]:
        u["sheet"] = sid
        units[u["id"]] = u
        sheet_units[sid].append(u["id"])
        if "parent" in u:
            children[u["parent"]].append(u["id"])


def subtree(uid, skip):
    out = []
    for c in children[uid]:
        if c in skip or not is_d(c):
            continue
        out.append(c)
        out += subtree(c, skip)
    return out


def is_d(uid):
    return units[uid].get("arrives") == "D"


placed = defaultdict(list)  # unit -> groups
for side, fn in (("axis", "land_axis.toml"), ("commonwealth", "land_cw.toml")):
    d = tomllib.load(open(root / "data/scenarios/graziani" / fn, "rb"))
    for g in d["group"]:
        for e in g.get("unit", []):
            skip = set(e.get("less", [])) | set(e.get("det", []))
            ids = []
            if "sheet" in e:
                ids = [u for u in sheet_units[e["sheet"]]]
            else:
                ids = [e["unit"]] + ([] if e.get("hq_only") else subtree(e["unit"], skip))
            ids += e.get("att", [])
            for u in ids:
                if u in skip:
                    continue
                placed[u].append(g["id"])

side_of = {u: ("axis" if u.startswith("it.") else "commonwealth") for u in units}
bad = 0
for u, gs in placed.items():
    if not is_d(u):
        print("PLACED BUT NOT D:", u, units[u].get("arrives"), gs)
        bad += 1
    if len(gs) > 1:
        print("PLACED TWICE:", u, gs)
        bad += 1
for side in ("axis", "commonwealth"):
    tot = [u for u in units if side_of[u] == side and is_d(u)]
    got = [u for u in tot if u in placed]
    print(f"{side}: placed {len(got)} of {len(tot)} D-arrival units on the OA sheets in the data")
    print("  total placed (any):", sum(1 for u in placed if side_of[u] == side))
sys.exit(1 if bad else 0)
