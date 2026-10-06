# Scenarios

Starting-state data for each playable scenario: who is where, with which organization, trucks,
planes, supplies and construction, plus the scenario's clock, initiative, victory conditions and the
reinforcements/withdrawals that fall inside it. Owner: `oob`. Rules for *reading* a set-up are in
`scen:59.2`–`59.6`; the scenario text is `scen:60.x` for Graziani's.

**Status: DRAFT v0 for lead review. Example values below are real values read from the booklet.**

## Layout

```
data/scenarios/
  README.md  GAPS.md
  graziani/
    scenario.toml          # id, clock, rules-profile requirements, initiative, victory, abstractions register
    land_axis.toml         # Italian initial deployment               (scen:60.31)
    land_cw.toml           # Commonwealth initial deployment          (scen:60.41)
    air_axis.toml          # planes, pilots, SGSUs                    (scen:60.32)
    air_cw.toml            # CW N. African air + Malta                (scen:60.42, 60.46)
    facilities.toml        # air facilities, ports, repair            (scen:60.5, 60.33, 60.43, 60.44)
    supply.toml            # trucks (2nd–3rd line), dumps, air supply (scen:60.33–60.34, 60.43–60.44)
    construction.toml      # construction at start                    (scen:60.7)
    fleet.toml             # fleets, coastal shipping, convoys        (scen:60.35, 60.37, 60.45)
    arrivals.toml          # GT1–6 reinforcements, replacements, withdrawals, pulled from data/units/schedules
```

Hex ids are exactly the printed ids given by the cartographer's coordinate system (`data/map/README.md`),
`C4218` style; off-map locations (Tripoli/Tunisia boxes, `E(3433)`-style Cairo-area hexes printed
with a section letter in parentheses) use the map owner's ids for those and are logged in `GAPS.md`
where the map data does not yet define them. Unit and weapon ids come from `data/units/`.

## Placement forms

The set-ups mix exact hexes with freedom for the player. Every deployment record has a
`placement`:

| `placement.kind` | Meaning | Fields |
|---|---|---|
| `hex` | Exactly this hex | `hex` |
| `hexes_any` | Listed units may be split among these hexes freely (stacking limits apply) | `hexes = [...]` |
| `area` | Anywhere in a named area | `area` (`libya`, `egypt`, `map_a_or_b`, `map_d_or_e`, `tripoli`, `tripolitania`, …) |
| `within` | Within N hexes of a hex | `hex`, `n` |
| `city` | In a named city/box | `city` |

Area names are scenario-local vocabulary; the engine maps them to hex sets via `data/map`.

## Area vocabulary (scenario-local; hex sets belong to `data/map/areas.toml`)

| Area name | One-line definition | Used by |
|---|---|---|
| `libya` | Any land hex of Libya on the game-maps (Axis-held territory at the start) | `scen:60.31` Anywhere in Libya; `scen:60.33` trucks |
| `egypt` | Any land hex of Egypt on the game-maps | `scen:60.44` dump |
| `map_a_or_b` | Any hex on game-map sections A or B | `scen:60.31` |
| `map_c_libya` | Hexes of map section C that lie in Libya | `scen:60.34` Dumps 1, 2 and dummies |
| `map_c_or_d_egypt` | Egyptian hexes of map sections C or D | `scen:60.44` Dump 1 and dummy |
| `map_d_or_e` | Any hex on game-map sections D or E | `scen:60.41` |
| `tripoli` | Tripoli (the off-map Tripoli box; Tunisia boxes are separate) | `scen:60.31`, `60.33` |
| `tripolitania` | Tripolitania (west Libya; the Axis off-map Tripolitania holding area) | `scen:60.31` 4/10 Army |
| `cairo`, `alexandria`, `helwan` | The city hexes of these places (Cairo area `E(1430)`; Alexandria `E3613`/`E3714`) | `scen:60.41`, `60.43` |
| `any_air_facility` | Any friendly air facility | `scen:60.33`, `60.43` |

Exclusion qualifiers: `exclusion = { enemy_unit_within_hexes = N }` (no Commonwealth unit within N hexes).
Off-map and boxed ids (Tripoli/Tunisia boxes, `E(3433)`, `E(1833)` style) are the cartographer's; this
folder does not mint them.

## Land deployment records (`land_*.toml`)

One `[[group]]` per set-up line. A group has a placement and a list of `unit` entries. A unit entry
refers to an OA unit id (`data/units/oa/…`) and states the *deviations from that OA sheet* using the
booklet's vocabulary (`scen:59.2`):

| Field | Booklet notation | Meaning |
|---|---|---|
| `unit` | the listed unit | OA unit id; the entry includes every assigned unit of its subtree that arrives `D`, minus `less` and `det` |
| `sheet` | a whole garrison or "unassigned" group listed by name | every `D` unit of that OA sheet |
| `hq_only = true` | "HQ:" | only the HQ counter |
| `att = [unit ids]` | "Att:" | attached (not assigned) to this parent in the same hex |
| `assg = [unit ids]` | "Assg:" | additionally assigned (not originally on the OA sheet) |
| `less = [unit ids]` | "Less:" | assigned units that are not with the parent |
| `det = [unit ids]` | "Det:" | assigned units detached (they are placed in their own groups); a detached unit whose arrival is after the start is simply not on the map yet |
| `consists_of = [unit ids]` | "Consists of" | a wholly new roster (not used in Graziani's) |
| `note` | | paraphrased clarification |
| `trucks = { light, medium, heavy }` (group level) | "Trucks:" | first-line truck points, distributed by the player among the hex's units (`scen:59.42`) |
| `state = "in_training"` (group level) | "In Training" | the group starts in training (`land:17`) |
| `src` | | `["scen:60.31"]` |

The printed unit-type tags in brackets (`[T]`, `(I)`, `(Ar)`) and parent-counter hints (`Ar/LTC`, `7Spt/7`) are display aids only; they are derived from the OA sheets and are not stored.

The booklet also uses placement of whole sets: a *parent* listed as "Det: all …" means all its
assigned units are on the map elsewhere in the list. Both ends are recorded: the parent has
`det = [...]`; each detached unit appears in its own hex group.

### Example 1 — Italian infantry division with attachment (`scen:60.31`)

```toml
[[group]]
id = "it_c4218_1ccnn"
placement = { kind = "hex", hex = "C4218" }
trucks = { light = 10, medium = 25, heavy = 10 }
src = ["scen:60.31"]

  [[group.unit]]
  unit = "it.1ccnn_div.hq"          # 1 CCNN Div (I)
  weapon_type = "I"
  att = [ "it.libyan_tank_command.i_m" ]   # I(M) [T; Ar/LTC]
```

### Example 2 — Commonwealth stack with assigned-to-another-parent units and trucks (`scen:60.41`)

```toml
[[group]]
id = "cw_c3922"
placement = { kind = "hex", hex = "C3922" }
trucks = { light = 0, medium = 15, heavy = 5 }
src = ["scen:60.41"]

  [[group.unit]]
  unit = "cw.3_coldstream_gds"       # 3rd Coldstream Gds (I)
  weapon_type = "I"
  [[group.unit]]
  unit = "cw.1_krrc"                 # 1st KRRC (I; 7Spt/7)
  weapon_type = "I"
  parent_tag = "7Spt/7"
  [[group.unit]]
  unit = "cw.4_rha"                  # 4th RHA (7Spt/7)
  parent_tag = "7Spt/7"
  [[group.unit]]
  unit = "cw.7_medium_arty_regt"
```

(The 7th Armoured Division itself is a separate group at D3612 with `det =` all its listed
subordinate units, as printed.)

## Air records (`air_*.toml`)

```toml
[force]
side = "axis"
src = ["scen:60.32"]
refit_not_before = { gt = 1, opstage = 2 }       # no refit attempts before GT1 OpStage 2

[[plane]]
type = "it.cr42"                    # data/units/aircraft
total = 65
ready = 25                          # "Total Refitted"
src = ["scen:60.32"]

[pilots]
three = 3
two = 10
one = 15

[sgsu]
available = 39
```

Further record kinds in the Graziani folder: `[[bonus_toe]]` (the two Autoblinda points), `[[broken_down_vehicles]]` (Alexandria), `[[dump]]`/`[[dummy_dump]]`/`[air_supply_pool.<side>]`/`[[second_third_line_trucks]]`/`[unlimited_supply]` (supply.toml), `[[facility]]`/`[[repair_facility]]` (facilities.toml), `[construction]`/`[[port_override]]`, `[axis_coastal_shipping]`/`[commonwealth_fleet]` (fleet.toml), `[malta]` in air_cw.toml.

Tools: `python tools/units/validate.py` (schema and reference checks) and `python tools/units/coverage.py` (every `D`-arrival OA unit is placed exactly once).

Planes may start at any friendly air facility within capacity (`scen:59.35`); pilots, planes and SGSUs
are assigned by the player. Malta (`scen:60.46`) uses the same shape with `theatre = "malta"` plus
`aa_points = 17`, `facility_capacity_sgsu = 5`.

## Supply, trucks, facilities

```toml
[[dump]]                            # supply.toml
id = "ax_tobruk"
location = { kind = "hex", hex = "C4807" }
ammo = 200
fuel = 2000
stores = 500
water = 0                           # a "–" in the printed table = omitted, recorded as 0 only if the booklet prints 0
active = true
src = ["scen:60.34"]

[[dump]]
id = "ax_dump_1"
location = { kind = "area", area = "map_c_libya", exclusion = { enemy_unit_within_hexes = 4 } }
ammo = 1000
fuel = 1500
stores = 1500
water = 200
active = true
src = ["scen:60.34"]

[[dummy_dump]]
count = 2
location = { kind = "area", area = "map_c_libya", exclusion = { enemy_unit_within_hexes = 4 } }
src = ["scen:60.34"]

[air_supply_pool]                   # freely distributable among that side's airfields
ammo = 1200
fuel = 850
stores = 100
water = 100
src = ["scen:60.34"]

[[second_third_line_trucks]]        # supply.toml
side = "axis"
placement = { kind = "city", city = "tripoli" }
light = 25
medium = 140
heavy = 40
src = ["scen:60.33"]
```

Air facilities (`facilities.toml`): one `[[facility]]` per row of `scen:60.5`, with `kind` (airfield,
landing_strip, flying_boat_basin, alighting_area), `name`, `hex` (or `location`: an off-map id from `data/map/areas.toml`, e.g. `offmap_deversoir`; `location_area` for a set such as `tripoli_tunisia_boxes`; `printed_location` keeps the printed token), `owner` at start,
`src`. Ports/repair facilities carry their efficiency level and repair class.

## Scenario record (`scenario.toml`)

```toml
[scenario]
id = "graziani"
name = "Graziani's Offensive"
src = ["scen:60.22"]
start = { gt = 1, opstage = 1 }
end = { gt = 6, opstage = 3 }
systems = ["land", "air", "logistics"]     # full systems; abstractions in scen:60.9 are not used
initiative = { gt1 = "axis", from_gt2 = "normal_rules", src = ["scen:60.6"] }

[[victory]]                                # scen:60.81 — one entry per level; conditions as structured clauses
side = "axis"
level = "strategic"
require = [ { occupy_any = ["alexandria", "cairo"] }, { qualifying_unit = "combat_unit" }, { supply_trace = "convoy", to = "map_d" } ]
src = ["scen:60.81"]

[[abstraction_note]]                       # scen:60.9, recorded but unused under full systems
case = "60.92"
note = "Land-game-only supply and motorization substitutions; not applicable."
```

## Victory clause vocabulary

`[[victory]]` entries (`side`, `level` ∈ strategic | decisive | tactical, `require = [clauses]`, `src`).
All clauses in `require` must hold at end of game. Finite clause types:

| Clause | Fields | Meaning |
|---|---|---|
| `occupy_all` | `places` | Hold (occupy) every listed place with a qualifying unit |
| `occupy_any` | `places` | Hold at least one listed place |
| `retain` | `places` | Possess it at the end (already held at start and not lost) |
| `qualifying_unit` | `kind = "combat_unit"` | The holder must be a combat unit (non-parenthesized close-assault rating, `scen:60.82` note) |
| `supply_trace` | `mode = "convoy"` or `"truck_convoy"`, `to` (place, map section, or `"home_base"`) | The holder must be supplyable by that route to that place (as stated in `scen:60.81`) |

Places are city/oasis names resolved by the map data. Both sides' levels are listed separately; a
side is "highest level achieved" as defined in `scen:60.81`; the evaluation order is the engine's.

## Arrivals (`arrivals.toml`)

References rows of `data/units/schedules/*` that fall within GT 1–6, so the scenario file is
self-contained for the engine without duplicating unit definitions.

## Verification and gaps

Every record carries `src`. Values read from tables that were checked twice carry
`verification = "double"` in the file header. Illegible or missing values are **omitted** and logged
in `data/scenarios/GAPS.md`; nothing is guessed. Struck-through text in the printed booklet
(for example the Alexandria fleet roster in `scen:60.45`) is recorded as superseded text in
`GAPS.md` rather than as data.
