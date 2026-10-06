#!/usr/bin/env python3
"""Validate data/units (and, when present, data/scenarios) against the schemas in their READMEs.

Needs Python >= 3.11 (tomllib). Usage:  python tools/units/validate.py [repo_root]
Exit status is non-zero if any check fails.

Checks: TOML parses; required fields; unknown field names; duplicate ids per kind;
dangling references (class / weapon / parent / mention unit / aircraft type); value sanity.
"""
import sys
import tomllib
from pathlib import Path

root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[2]
units = root / "data" / "units"
scen = root / "data" / "scenarios"
errors = []


def err(path, msg):
    errors.append(f"{path.relative_to(root)}: {msg}")


def load(path):
    try:
        with open(path, "rb") as f:
            return tomllib.load(f)
    except Exception as e:  # noqa: BLE001
        err(path, f"TOML error: {e}")
        return {}


def check_fields(path, rec, required, allowed, what):
    for k in required:
        if k not in rec:
            err(path, f"{what} {rec.get('id', '?')}: missing required field {k!r}")
    for k in rec:
        if k not in allowed:
            err(path, f"{what} {rec.get('id', '?')}: unknown field {k!r}")


RATING = ["aa", "barrage", "anti_armor", "vulnerability", "armor_prot"]
WEAPON_ALLOWED = (
    {"id", "nation", "kind", "name", "cpa", "cpa_sited", "ca_off", "ca_def", "fuel_rate", "bar", "note", "src",
     "airdroppable", "us_tank", "minesweeper", "emplaced"}
    | set(RATING) | {r + "_paren" for r in RATING} | {"ca_off_paren", "ca_def_paren"}
)
CLASS_ALLOWED = (
    {"id", "nation", "code", "unit_type", "echelon", "cpa", "cpa_fixed", "cpa_plus", "ca_off", "ca_def", "max_toe",
     "max_toe_paren", "max_toe_extra", "assigns", "src", "note", "emplaced", "max_toe_kind", "role",
     "armor_prot_only_when_truck_transported", "equipment_note", "max_toe_if_all_us_tanks"}
    | set(RATING) | {r + "_paren" for r in RATING} | {"ca_off_paren", "ca_def_paren"}
)
AIRCRAFT_ALLOWED = {"id", "nation", "name", "role", "manufacturer", "mode", "src", "note"}
MODE_ALLOWED = {"range_hexes", "tacair", "tacair_paren", "maneuver", "bomb_capacity", "torpedo_capacity", "fuel_points",
                "missions", "transport", "rng_is_transfer_range"}
MISSION_VALUES = {"day", "night", "night_only", "strafe_only"}
UNIT_ALLOWED = {"id", "name", "counter", "class", "echelon", "toe", "arrives", "arrives_raw", "parent", "nationality",
                "note", "src", "reassign", "training", "morale_untrained", "shell", "garrison_of", "immobile",
                "toe_note", "arrives_note", "kind", "group", "engineer_hq", "never_arrived_parent", "stacking_points",
                "echelon_symbol", "engineer", "garrison", "basic_morale", "begins_attached_to_sheet", "immobile"}
SHEET_ALLOWED = {"id", "nation", "side", "nationality", "name", "basic_morale", "basic_morale_untrained", "src", "note"}
MENTION_ALLOWED = {"unit", "begins_attached_to", "note", "src"}

weapons, classes, aircraft, unit_ids = {}, {}, {}, {}
pending_refs = []  # (path, kind, ref)

for p in sorted((units / "weapons").glob("*.toml")) if (units / "weapons").exists() else []:
    d = load(p)
    for w in d.get("weapon", []):
        check_fields(p, w, ["id", "nation", "kind", "name", "cpa", "src"] if "src" in w else ["id", "nation", "kind", "name", "cpa"], WEAPON_ALLOWED, "weapon")
        if w.get("id") in weapons:
            err(p, f"duplicate weapon id {w['id']}")
        weapons[w.get("id")] = w
        if not str(w.get("id", "")).startswith(w.get("nation", "?") + "."):
            err(p, f"weapon id {w.get('id')} does not start with nation prefix")

for p in sorted((units / "classes").glob("*.toml")) if (units / "classes").exists() else []:
    d = load(p)
    for c in d.get("class", []):
        check_fields(p, c, ["id", "nation", "code", "unit_type", "cpa"], CLASS_ALLOWED, "class")
        if c.get("id") in classes:
            err(p, f"duplicate class id {c['id']}")
        classes[c.get("id")] = c
        if c.get("id") != f"{c.get('nation')}.{c.get('code')}":
            err(p, f"class id {c.get('id')} != nation.code")
        for a in c.get("assigns", []):
            if set(a) - {"kind", "max", "note"}:
                err(p, f"class {c['id']}: unknown assigns field")

for p in sorted((units / "aircraft").glob("*.toml")) if (units / "aircraft").exists() else []:
    d = load(p)
    for a in d.get("aircraft", []):
        check_fields(p, a, ["id", "nation", "name", "role", "mode"], AIRCRAFT_ALLOWED, "aircraft")
        if a.get("id") in aircraft:
            err(p, f"duplicate aircraft id {a['id']}")
        aircraft[a.get("id")] = a
        for m in a.get("mode", []):
            check_fields(p, {**m, "id": a.get("id")}, ["range_hexes", "tacair", "maneuver", "fuel_points"], MODE_ALLOWED | {"id"}, "aircraft mode")
            for k, v in m.get("missions", {}).items():
                if v not in MISSION_VALUES:
                    err(p, f"aircraft {a.get('id')}: bad mission value {k}={v!r}")

for p in sorted((units / "oa").glob("*/*.toml")) if (units / "oa").exists() else []:
    d = load(p)
    sheet = d.get("sheet", {})
    check_fields(p, {**sheet, "id": sheet.get("id")}, ["id", "nation", "side", "nationality", "name"], SHEET_ALLOWED, "sheet")
    if sheet.get("side") not in ("axis", "commonwealth"):
        err(p, f"sheet {sheet.get('id')}: side must be axis|commonwealth")
    for u in d.get("unit", []):
        check_fields(p, u, ["id", "name", "arrives"], UNIT_ALLOWED, "unit")
        uid = u.get("id")
        if uid in unit_ids:
            err(p, f"duplicate unit id {uid} (also in {unit_ids[uid]})")
        unit_ids[uid] = p.relative_to(root)
        if "class" in u:
            pending_refs.append((p, "class", u["class"], uid))
        if "parent" in u:
            pending_refs.append((p, "unit", u["parent"], uid))
        if isinstance(u.get("toe"), list):
            for t in u["toe"]:
                pending_refs.append((p, "weapon", t.get("weapon"), uid))
    for m in d.get("mention", []):
        check_fields(p, {**m, "id": m.get("unit")}, ["unit"], MENTION_ALLOWED | {"id"}, "mention")
        pending_refs.append((p, "unit", m.get("unit"), "mention"))
        if "begins_attached_to" in m:
            pending_refs.append((p, "unit", m["begins_attached_to"], "mention"))

# schedules reference units/formations
for p in sorted((units / "schedules").glob("*.toml")) if (units / "schedules").exists() else []:
    d = load(p)
    for kind in ("arrival", "withdrawal", "replacement"):
        for r in d.get(kind, []):
            for u in r.get("units", []):
                if "unit" in u:
                    pending_refs.append((p, "unit", u["unit"], kind))

tables = {"class": classes, "weapon": weapons, "unit": unit_ids}
for p, kind, ref, owner in pending_refs:
    if ref not in tables[kind]:
        err(p, f"{owner}: dangling {kind} reference {ref!r}")

# scenarios: reference checks only for unit/aircraft ids when files exist
if scen.exists():
    for p in sorted(scen.rglob("*.toml")):
        d = load(p)

        def walk(x):
            if isinstance(x, dict):
                for k, v in x.items():
                    if k == "unit" and isinstance(v, str) and "." in v and v not in unit_ids:
                        err(p, f"dangling unit reference {v!r}")
                    elif k in ("att", "assg", "less", "det", "consists_of") and isinstance(v, list):
                        for r in v:
                            if r not in unit_ids:
                                err(p, f"dangling unit reference {r!r} in {k}")
                    elif k == "type" and isinstance(v, str) and v.split(".")[0] in ("it", "cw", "ge") and v not in aircraft:
                        err(p, f"dangling aircraft type {v!r}")
                    else:
                        walk(v)
            elif isinstance(x, list):
                for i in x:
                    walk(i)

        walk(d)

# sheet references and aircraft types in schedules
sheet_ids = {u.rsplit(".", 1)[0] for u in unit_ids}
if scen.exists():
    for p in sorted(scen.rglob("*.toml")):
        def walk_sheet(x):
            if isinstance(x, dict):
                for k, v in x.items():
                    if k == "sheet" and isinstance(v, str) and v not in sheet_ids:
                        err(p, f"dangling sheet reference {v!r}")
                    else:
                        walk_sheet(v)
            elif isinstance(x, list):
                for i in x:
                    walk_sheet(i)
        walk_sheet(load(p))
for p in sorted((units / "schedules").glob("*.toml")) if (units / "schedules").exists() else []:
    d = load(p)
    for kind in ("arrival", "withdrawal", "replacement"):
        for r in d.get(kind, []):
            for pl in r.get("planes", []):
                if pl.get("type") not in aircraft:
                    err(p, f"unknown aircraft type {pl.get('type')!r}")

n = len(errors)
print(f"weapons={len(weapons)} classes={len(classes)} aircraft={len(aircraft)} units={len(unit_ids)}  errors={n}")
for e in errors:
    print("ERROR", e)
sys.exit(1 if n else 0)
