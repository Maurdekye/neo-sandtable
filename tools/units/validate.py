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
MODE_ALLOWED = {"range_hexes", "tacair", "tacair_paren", "maneuver", "maneuver_night", "bomb_capacity", "torpedo_capacity", "fuel_points",
                "missions", "transport", "rng_is_transfer_range"}
MISSION_VALUES = {"day", "night", "night_only", "strafe_only"}
UNIT_ALLOWED = {"id", "name", "counter", "class", "echelon", "toe", "arrives", "arrives_raw", "parent", "nationality",
                "note", "src", "reassign", "training", "morale_untrained", "shell", "garrison_of", "immobile",
                "toe_note", "arrives_note", "kind", "group", "engineer_hq", "never_arrived_parent", "stacking_points",
                "echelon_symbol", "engineer", "garrison", "basic_morale", "begins_attached_to_sheet", "immobile", "cpa", "vehicle", "commander", "infantry_kind", "infantry_kind_evidence", "engineering"}
SHEET_ALLOWED = {"id", "nation", "side", "nationality", "name", "basic_morale", "basic_morale_untrained", "src", "note"}
MENTION_ALLOWED = {"unit", "begins_attached_to", "note", "src"}

weapons, classes, aircraft, unit_ids = {}, {}, {}, {}
pending_refs = []  # (path, kind, ref)


def check_engineering(path, unit):
    metadata = unit.get("engineering")
    if metadata is None:
        return  # Omitted is Unknown, including units sharing an engineer's class.
    if not isinstance(metadata, dict):
        err(path, f"unit {unit.get('id')}: engineering must be a table")
        return
    check_fields(path, metadata, ["scope", "evidence", "src"],
                 {"scope", "role", "toe_requirement", "evidence", "src"}, "engineering")
    scope, role, gate = metadata.get("scope"), metadata.get("role"), metadata.get("toe_requirement")
    if not isinstance(scope, str) or scope not in {"general", "railroad_only", "road_only", "anti_mine_only", "none"}:
        err(path, f"unit {unit.get('id')}: unknown engineering scope")
    if scope == "none":
        if role is not None or gate is not None:
            err(path, f"unit {unit.get('id')}: non-engineer identity cannot have role or TOE gate")
    elif not isinstance(role, str) or role not in {"company", "battalion", "headquarters"}:
        err(path, f"unit {unit.get('id')}: positive engineering scope requires a sourced role")
    citations = metadata.get("src")
    if not isinstance(citations, list) or not citations or not all(isinstance(s, str) and s.strip() for s in citations):
        err(path, f"unit {unit.get('id')}: engineering requires source citations")
    evidence = metadata.get("evidence")
    if not isinstance(evidence, dict):
        err(path, f"unit {unit.get('id')}: engineering requires double-read evidence")
    else:
        check_fields(path, evidence, ["transcribed_from", "verification"],
                     {"transcribed_from", "verification"}, "engineering evidence")
        files = evidence.get("transcribed_from")
        if evidence.get("verification") != "double" or not isinstance(files, list) or len(files) < 2 or not all(isinstance(f, str) and f.strip() for f in files):
            err(path, f"unit {unit.get('id')}: engineering needs two source filenames and double verification")
    if gate is not None:
        if not isinstance(gate, dict):
            err(path, f"unit {unit.get('id')}: engineering TOE gate must be a table")
            return
        check_fields(path, gate, ["weapon", "min_points"], {"weapon", "min_points"}, "engineering TOE gate")
        if not isinstance(gate.get("weapon"), str):
            err(path, f"unit {unit.get('id')}: engineering TOE weapon must be an id")
            return
        if scope != "anti_mine_only" or gate.get("weapon") != "cw.scorpion" or type(gate.get("min_points")) is not int or gate["min_points"] != 6:
            err(path, f"unit {unit.get('id')}: engineering gate must use land:23.15 Scorpion six-point threshold")
        pending_refs.append((path, "weapon", gate.get("weapon"), unit.get("id")))

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
        check_engineering(p, u)
        kind, evidence = u.get("infantry_kind"), u.get("infantry_kind_evidence")
        if kind is not None or evidence is not None:
            if kind not in {"ordinary", "machine_gun", "heavy_weapons"}:
                err(p, f"unit {u.get('id')}: unknown infantry_kind")
            if not isinstance(evidence, dict):
                err(p, f"unit {u.get('id')}: infantry kind requires OA/counter evidence")
            else:
                check_fields(p, evidence, ["transcribed_from", "verification"],
                             {"transcribed_from", "verification"}, "infantry_kind_evidence")
                files = evidence.get("transcribed_from", [])
                if evidence.get("verification") != "double" or not isinstance(files, list) or len(files) < 2 or not all(isinstance(f, str) and f for f in files):
                    err(p, f"unit {u.get('id')}: infantry classification needs two source filenames and double verification")
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

# Schedule shapes and references (both land and air).
ROW_ALLOWED = {"gt", "opstage", "gt_from", "gt_to", "label", "nationality", "location", "units", "trucks",
               "planes", "distribution", "squadrons", "when", "note", "src", "transport"}
SELECTOR_ALLOWED = {"unit", "subtree", "hq_only", "less", "att", "assg"}
for p in sorted((units / "schedules").glob("*.toml")):
    d = load(p)
    covered = d.get("file", {}).get("covers_gt", [])
    if len(covered) != 2 or any(type(v) is not int or v < 1 for v in covered) or covered[0] > covered[1]:
        err(p, "file: covers_gt must be an ordered inclusive pair")
    weights = d.get("file", {}).get("truck_value_halves")
    if weights is not None:
        check_fields(p, weights, ["light", "medium", "heavy"], {"light", "medium", "heavy"}, "truck_value_halves")
        for k, v in weights.items():
            if type(v) is not int or v < 1:
                err(p, f"file: truck_value_halves.{k} must be a positive integer")
    if any(r.get("transport") for r in d.get("withdrawal", [])) and weights is None:
        err(p, "file: transport requires truck_value_halves")
    for kind in ("arrival", "withdrawal", "replacement"):
        for r in d.get(kind, []):
            check_fields(p, r, ["src"], ROW_ALLOWED, kind)
            single = "gt" in r
            interval = "gt_from" in r or "gt_to" in r
            if single == interval:
                err(p, f"{kind}: exactly one of gt or gt_from/gt_to is required")
            for k in ("gt", "gt_from", "gt_to"):
                if k in r and (type(r[k]) is not int or r[k] < 1):
                    err(p, f"{kind}: {k} must be a positive integer")
            if interval:
                if not {"gt_from", "gt_to"} <= r.keys() or r.get("gt_to", 0) < r.get("gt_from", 1):
                    err(p, f"{kind}: incomplete or reversed monthly interval")
                if r.get("distribution") != "even_per_game_turn":
                    err(p, f"{kind}: monthly interval needs even_per_game_turn distribution")
                if "units" in r:
                    err(p, f"{kind}: land selectors require an exact stage")
            if "opstage" in r and (type(r["opstage"]) is not int or r["opstage"] not in (1, 2, 3)):
                err(p, f"{kind}: invalid opstage")
            if "units" in r and "opstage" not in r:
                err(p, f"{kind}: land row needs opstage")
            if not any(r.get(k) for k in ("units", "planes", "squadrons")):
                err(p, f"{kind}: row has no scheduled entities")
            for u in r.get("units", []):
                check_fields(p, u, ["unit"], SELECTOR_ALLOWED, "unit selector")
                pending_refs.append((p, "unit", u.get("unit"), kind))
                if u.get("subtree") and u.get("hq_only"):
                    err(p, f"{kind}: subtree and hq_only conflict")
                for flag in ("subtree", "hq_only"):
                    if flag in u and type(u[flag]) is not bool:
                        err(p, f"{kind}: {flag} must be boolean")
                for field in ("less", "att", "assg"):
                    for ref in u.get(field, []):
                        pending_refs.append((p, "unit", ref, kind))
            for pl in r.get("planes", []):
                check_fields(p, pl, ["type", "n"], {"type", "n"}, "plane arrival")
                if type(pl.get("n")) is not int or pl["n"] < 1:
                    err(p, f"{kind}: plane count must be positive")
            for sq in r.get("squadrons", []):
                check_fields(p, sq, ["role", "count", "min_planes"],
                             {"role", "count", "min_planes", "min_bomb_points_each", "note"}, "squadron selector")
                if sq.get("role") not in {"fighter", "bomber", "reconnaissance"}:
                    err(p, f"{kind}: unknown squadron role")
                for field in ("count", "min_planes", "min_bomb_points_each"):
                    if field in sq and (type(sq[field]) is not int or sq[field] < 1):
                        err(p, f"{kind}: {field} must be positive")
            for field, allowed in (("trucks", {"light", "medium", "heavy"}),
                                   ("transport", {"truck_value_points", "motorization_points"})):
                if field in r:
                    check_fields(p, r[field], sorted(allowed) if field == "transport" else [], allowed, field)
                    for k, v in r[field].items():
                        if type(v) is not int or v < 0:
                            err(p, f"{kind}: {field}.{k} must be a nonnegative integer")

# formation charts (land:19.3x): unit kinds and composition rows
KIND_ALLOWED = {"id", "nation", "name", "echelon", "symbol_echelon", "match", "classes", "sp", "fill_by", "any_of_kinds",
                "note", "src"}
KIND_MATCH_ALLOWED = {"unit_type", "echelon", "tags_any", "tags_all", "tags_none"}
FORMATION_ALLOWED = {"id", "nation", "name", "kind", "echelon", "sp", "periods", "designation", "applies_to",
                     "shares_row_with", "members", "exceptions", "limits", "note", "parent_note", "src"}
MEMBER_ALLOWED = {"kind", "formation", "sp", "any_of", "style", "gap", "glyph_note", "echelon_mark", "note"}
ECHELONS = {"company", "battalion", "brigade", "super_brigade", "division", "battle_group"}
SYMBOL_ECHELONS = {"I", "II", "III", "X", "XX"}
kinds, formations, parents = {}, {}, {}
form_refs = []  # (path, owner, field, ref)
for p in sorted((units / "formations").glob("*.toml")) if (units / "formations").exists() else []:
    d = load(p)
    nation = d.get("file", {}).get("nation")
    unit_types = {c.get("unit_type") for c in classes.values()}
    for k in d.get("kind", []):
        check_fields(p, k, ["id", "nation", "name", "src"], KIND_ALLOWED, "kind")
        kid = k.get("id")
        if kid in kinds:
            err(p, f"duplicate kind id {kid}")
        kinds[kid] = k
        if not str(kid).startswith(f"{nation}."):
            err(p, f"kind {kid}: id does not start with file nation {nation!r}")
        if "echelon" in k and k["echelon"] not in ECHELONS:
            err(p, f"kind {kid}: bad echelon {k['echelon']!r}")
        if "symbol_echelon" in k and k["symbol_echelon"] not in SYMBOL_ECHELONS:
            err(p, f"kind {kid}: bad symbol_echelon {k['symbol_echelon']!r}")
        m = k.get("match", {})
        for f in m:
            if f not in KIND_MATCH_ALLOWED:
                err(p, f"kind {kid}: unknown match field {f!r}")
        if "unit_type" in m and m["unit_type"] not in unit_types:
            err(p, f"kind {kid}: match.unit_type {m['unit_type']!r} is not a class unit_type")
        if "echelon" in m and m["echelon"] not in ECHELONS:
            err(p, f"kind {kid}: bad match.echelon {m['echelon']!r}")
        for c in k.get("classes", []):
            form_refs.append((p, kid, "class", c))
        for r in k.get("any_of_kinds", []):
            form_refs.append((p, kid, "kind", r))
        for r in k.get("fill_by", []):
            form_refs.append((p, kid, "kind", r.get("kind")))
    for f in d.get("formation", []):
        check_fields(p, f, ["id", "nation", "name", "echelon", "sp", "members", "src"], FORMATION_ALLOWED, "formation")
        fid = f.get("id")
        if fid in formations:
            err(p, f"duplicate formation id {fid}")
        formations[fid] = f
        if not str(fid).startswith(f"{nation}."):
            err(p, f"formation {fid}: id does not start with file nation {nation!r}")
        if f.get("echelon") not in ECHELONS:
            err(p, f"formation {fid}: bad echelon {f.get('echelon')!r}")
        for pr in f.get("periods", []):
            if set(pr) - {"gt_from", "gt_to"} or pr.get("gt_to", 10**6) < pr.get("gt_from", 1):
                err(p, f"formation {fid}: bad period {pr}")
        if "kind" in f:
            form_refs.append((p, fid, "kind", f["kind"]))
        for u in f.get("applies_to", []):
            form_refs.append((p, fid, "unit", u))
        if "shares_row_with" in f:
            form_refs.append((p, fid, "formation", f["shares_row_with"]))
        for e in f.get("exceptions", []):
            form_refs.append((p, fid, "sheet", e.get("holder_sheet")))
            if "kind" in e.get("add", {}):
                form_refs.append((p, fid, "kind", e["add"]["kind"]))
        if not f.get("members"):
            err(p, f"formation {fid}: no members")

        def check_member(m, top):
            check_fields(p, {**m, "id": fid}, [], MEMBER_ALLOWED | {"id"}, "member")
            if "any_of" in m:
                if not top:
                    err(p, f"formation {fid}: nested any_of")
                if len(m["any_of"]) < 2 or set(m) - {"any_of", "note"}:
                    err(p, f"formation {fid}: any_of needs >=2 options and nothing else")
                for o in m["any_of"]:
                    check_member(o, False)
                return
            if "kind" not in m and "formation" not in m:
                err(p, f"formation {fid}: member has neither kind nor formation")
            if "kind" in m:
                form_refs.append((p, fid, "kind", m["kind"]))
            if "formation" in m:
                form_refs.append((p, fid, "formation", m["formation"]))
            if "sp" not in m and "gap" not in m and "kind" in m:
                pass  # no number printed
            if "sp" in m and not isinstance(m["sp"], int):
                err(p, f"formation {fid}: member sp must be an integer")
            form_refs.append((p, fid, "member_sp", m))

        for m in f.get("members", []):
            check_member(m, True)

    for row in d.get("parent", []):
        check_fields(p, row, ["unit", "evidence", "src"],
                     {"unit", "profiles", "oa_slots", "evidence", "src", "attachment_maximum"}, "parent")
        uid = row.get("unit")
        if uid in parents:
            err(p, f"duplicate organization parent {uid}")
        parents[uid] = (p, row)
        if ("profiles" in row) == ("oa_slots" in row):
            err(p, f"{uid}: exactly one of profiles and oa_slots is required")
        ev = row.get("evidence", {})
        if ev.get("verification") != "double" or len(ev.get("transcribed_from", [])) < 2 or not all(ev.get("transcribed_from", [])):
            err(p, f"{uid}: paired double-read organization evidence required")
        form_refs.append((p, uid, "unit", uid))
        for fid in row.get("profiles", []):
            form_refs.append((p, uid, "formation", fid))
        for slot in row.get("oa_slots", []):
            form_refs.append((p, uid, "unit", slot))
        if "profiles" in row and not row["profiles"]:
            err(p, f"{uid}: empty profile list")
        limit = row.get("attachment_maximum")
        if limit is not None:
            check_fields(p, limit, ["units", "evidence", "src"], {"units", "evidence", "src"}, "attachment maximum")
            ev = limit.get("evidence", {})
            if not isinstance(limit.get("units"), int) or limit["units"] < 0 or ev.get("verification") != "double" or len(ev.get("transcribed_from", [])) < 2:
                err(p, f"{uid}: invalid attachment maximum evidence")

for uid, (p, row) in parents.items():
    dates = []
    for fid in row.get("profiles", []):
        f = formations.get(fid)
        if not f:
            continue
        for period in f.get("periods", [{"gt_from": 1}]):
            lo, hi = period.get("gt_from", 1), period.get("gt_to", 65535)
            if any(lo <= b and a <= hi for a, b in dates):
                err(p, f"{uid}: overlapping inclusive organization profiles")
            dates.append((lo, hi))

# sp consistency of member rows vs. the referenced kind / formation
for p, owner, what, ref in form_refs:
    if what == "member_sp":
        m = ref
        if "sp" in m and "kind" in m and m["kind"] in kinds and "sp" in kinds[m["kind"]] and m["sp"] != kinds[m["kind"]]["sp"]:
            err(p, f"{owner}: member {m['kind']} sp {m['sp']} differs from the kind's sp {kinds[m['kind']]['sp']}")
        if "formation" in m and m["formation"] in formations:
            tf = formations[m["formation"]]
            if "sp" in m and m["sp"] != tf.get("sp"):
                err(p, f"{owner}: member formation {m['formation']} sp {m['sp']} differs from its sp {tf.get('sp')}")
            if "kind" in m and tf.get("kind") not in (None, m["kind"]):
                err(p, f"{owner}: member kind {m['kind']} != kind {tf.get('kind')} of formation {m['formation']}")

formation_sheet_ids = {u.rsplit(".", 1)[0] for u in unit_ids}
for p, owner, what, ref in form_refs:
    if what == "member_sp":
        continue
    table = {"class": classes, "kind": kinds, "formation": formations, "unit": unit_ids,
             "sheet": dict.fromkeys(formation_sheet_ids)}[what]
    if ref not in table:
        err(p, f"{owner}: dangling {what} reference {ref!r}")

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

# Coastal rosters contain printed counter capacities, never procedural CP or cargo.
ship_ids, ship_rosters = set(), set()
for p in sorted((units / "ships").glob("*.toml")):
    d = load(p)
    ship_rosters.add(p.relative_to(units).as_posix())
    meta = d.get("file", {})
    check_fields(p, meta, ["src", "transcribed_from", "verification"],
                 {"src", "transcribed_from", "verification"}, "ship file")
    if not meta.get("src") or not meta.get("transcribed_from") or meta.get("verification") not in ("single", "double"):
        err(p, "ship file requires citations, source filenames and verification")
    if not d.get("ships"):
        err(p, "empty ship roster")
    for ship in d.get("ships", []):
        check_fields(p, ship, ["id", "designation", "src", "transcribed_from"],
                     {"id", "designation", "capacity_tons", "src", "transcribed_from"}, "ship")
        sid = ship.get("id")
        if not isinstance(sid, str) or not sid or sid in ship_ids:
            err(p, f"missing or duplicate ship counter id {sid!r}")
        ship_ids.add(sid)
        if not ship.get("designation") or "airlog:56.31" not in ship.get("src", []) or not ship.get("transcribed_from"):
            err(p, f"ship {sid!r} needs a printed designation and defining provenance")
        capacity = ship.get("capacity_tons")
        if capacity is not None and (type(capacity) is not int or capacity <= 0):
            err(p, f"ship {sid!r} capacity_tons must be a positive integer or absent")

for p in sorted(scen.glob("*/fleet.toml")):
    shipping = load(p).get("axis_coastal_shipping", {})
    if "roster" in shipping and shipping["roster"] not in ship_rosters:
        err(p, f"unknown ship roster under data/units: {shipping['roster']!r}")

# off-map location / area ids used by scenario files must exist in the cartographer's areas.toml
areas_file = root / "data" / "map" / "areas.toml"
if scen.exists() and areas_file.exists():
    ad = load(areas_file)
    loc_ids = {x.get("id") for x in ad.get("locations", [])}
    area_ids = {x.get("id") for x in ad.get("areas", [])}
    for p in sorted(scen.rglob("*.toml")):
        def walk_loc(x):
            if isinstance(x, dict):
                for k, v in x.items():
                    if k == "location" and isinstance(v, str) and v not in loc_ids:
                        err(p, f"unknown map location id {v!r}")
                    elif k in ("area", "location_area") and isinstance(v, str) and v not in area_ids and not v.startswith("unclear_"):
                        err(p, f"unknown map area id {v!r}")
                    else:
                        walk_loc(v)
            elif isinstance(x, list):
                for i in x:
                    walk_loc(i)
        walk_loc(load(p))

n = len(errors)
print(f"weapons={len(weapons)} classes={len(classes)} aircraft={len(aircraft)} units={len(unit_ids)} "
      f"kinds={len(kinds)} formations={len(formations)}  errors={n}")
for e in errors:
    print("ERROR", e)
sys.exit(1 if n else 0)
